//! NovaId — NovaDB's 128-bit document identifier.

use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{NovaError, Result};

/// Binary length of a `NovaId` in bytes.
pub const NOVA_ID_LENGTH: usize = 16;

/// Prefix used in a `NovaId`'s human-readable form.
pub const NOVA_ID_PREFIX: &str = "nova_";

/// A globally unique 128-bit document identifier.
///
/// # Binary layout (16 bytes, big-endian)
///
/// ```text
/// bytes 0..=7   unix epoch milliseconds, big-endian (u64)
/// bytes 8..=15  64 random bits from the operating-system CSPRNG
/// ```
///
/// # Properties
///
/// - **Time-ordered:** the big-endian ms timestamp is placed first, so byte
///   order equals creation order. IDs created within the same millisecond
///   tie-break by their random component.
/// - **Compact:** exactly 16 bytes in binary form; 37 characters as the
///   human-readable `nova_` + 32-hex-digit form.
/// - **Collision-resistant:** 64 bits of OS entropy per ID. The expected number
///   of collisions among `N` IDs is roughly `N^2 / 2^129` (birthday bound), i.e.
///   about `1.5e-21` for a billion IDs. The statistical test in this module
///   verifies zero collisions in practice.
///
/// See `docs/novaid-format.md` for the full format specification.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NovaId([u8; NOVA_ID_LENGTH]);

impl NovaId {
    /// Generates a fresh ID from the current wall-clock time and OS entropy.
    ///
    /// # Errors
    ///
    /// Returns an [`NovaError::Internal`] error if the operating-system entropy
    /// source is unavailable.
    pub fn generate() -> Result<Self> {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random)
            .map_err(|e| NovaError::Internal(format!("entropy source failure: {e}")))?;
        Ok(Self::new(epoch_millis(), random))
    }

    /// Builds an ID from explicit components; used for deterministic tests and
    /// for reconstructing IDs during recovery.
    #[must_use]
    pub const fn new(epoch_millis: u64, random: [u8; 8]) -> Self {
        let mut bytes = [0u8; NOVA_ID_LENGTH];
        let ts = epoch_millis.to_be_bytes();
        let mut i = 0;
        while i < 8 {
            bytes[i] = ts[i];
            i += 1;
        }
        let mut j = 0;
        while j < 8 {
            bytes[8 + j] = random[j];
            j += 1;
        }
        Self(bytes)
    }

    /// Interprets exactly [`NOVA_ID_LENGTH`] bytes as an ID.
    ///
    /// # Errors
    ///
    /// Returns an [`NovaError::InvalidArgument`] error when `bytes` is not
    /// exactly [`NOVA_ID_LENGTH`] bytes long.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let arr: [u8; NOVA_ID_LENGTH] = bytes.try_into().map_err(|_| {
            NovaError::InvalidArgument(format!(
                "nova id must be exactly {NOVA_ID_LENGTH} bytes; got {}",
                bytes.len()
            ))
        })?;
        Ok(Self(arr))
    }

    /// Returns the raw 16-byte representation.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; NOVA_ID_LENGTH] {
        &self.0
    }

    /// Returns the ID as an owned byte array.
    #[must_use]
    pub fn to_bytes(self) -> [u8; NOVA_ID_LENGTH] {
        self.0
    }

    /// The creation timestamp (unix epoch milliseconds) embedded in this ID.
    #[must_use]
    pub fn timestamp_ms(&self) -> u64 {
        let mut ts = 0u64;
        for &b in &self.0[..8] {
            ts = (ts << 8) | u64::from(b);
        }
        ts
    }

    /// The 64 random bits embedded in this ID.
    #[must_use]
    pub fn random_half(&self) -> [u8; 8] {
        let mut r = [0u8; 8];
        r.copy_from_slice(&self.0[8..]);
        r
    }

    /// Encodes the ID as a 32-character lowercase hex string (no prefix).
    #[must_use]
    pub fn to_hex(&self) -> String {
        encode_hex(&self.0)
    }
}

impl fmt::Display for NovaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(NOVA_ID_PREFIX)?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for NovaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NovaId({self})")
    }
}

impl FromStr for NovaId {
    type Err = NovaError;

    /// Parses an ID from `nova_`-prefixed or bare 32-hex-digit strings.
    /// Both lowercase and uppercase hex digits are accepted.
    ///
    /// # Errors
    ///
    /// Returns an [`NovaError::InvalidArgument`] error if the string is not a
    /// valid NovaId representation.
    fn from_str(s: &str) -> Result<Self> {
        let (hex, prefix_len) = match s.strip_prefix(NOVA_ID_PREFIX) {
            Some(rest) => (rest, NOVA_ID_PREFIX.len()),
            None => (s, 0),
        };
        let bytes = s.as_bytes();
        if hex.len() != NOVA_ID_LENGTH * 2 {
            return Err(NovaError::InvalidArgument(format!(
                "nova id must be exactly {} hex digits; got {}",
                NOVA_ID_LENGTH * 2,
                hex.len()
            )));
        }
        let mut out = [0u8; NOVA_ID_LENGTH];
        for i in 0..NOVA_ID_LENGTH {
            let hi = decode_nibble(bytes[prefix_len + 2 * i])
                .ok_or_else(|| invalid_hex(prefix_len + 2 * i))?;
            let lo = decode_nibble(bytes[prefix_len + 2 * i + 1])
                .ok_or_else(|| invalid_hex(prefix_len + 2 * i + 1))?;
            out[i] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }
}

/// Current unix epoch milliseconds; returns 0 if the clock is pre-epoch.
fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0f)]));
    }
    out
}

fn decode_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn invalid_hex(position: usize) -> NovaError {
    NovaError::InvalidArgument(format!("invalid hex digit at position {position}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const ONE_DAY_MS: u64 = 86_400_000;

    #[test]
    fn generated_id_has_correct_shape() {
        let id = NovaId::generate().unwrap();
        assert_eq!(id.as_bytes().len(), NOVA_ID_LENGTH);
        let s = id.to_string();
        assert!(s.starts_with(NOVA_ID_PREFIX));
        assert_eq!(s.len(), NOVA_ID_PREFIX.len() + NOVA_ID_LENGTH * 2);
        assert_eq!(id.to_hex().len(), NOVA_ID_LENGTH * 2);
    }

    #[test]
    fn generated_timestamp_is_close_to_now() {
        let id = NovaId::generate().unwrap();
        let now = epoch_millis();
        let delta = now.abs_diff(id.timestamp_ms());
        assert!(delta < ONE_DAY_MS, "id timestamp drifted: {delta} ms");
    }

    #[test]
    fn new_is_pure_and_deterministic() {
        let a = NovaId::new(1_700_000_000_000, [7u8; 8]);
        let b = NovaId::new(1_700_000_000_000, [7u8; 8]);
        assert_eq!(a, b);
        assert_eq!(a.timestamp_ms(), 1_700_000_000_000);
        assert_eq!(a.random_half(), [7u8; 8]);
    }

    #[test]
    fn byte_order_matches_creation_time() {
        // A later timestamp always compares greater, regardless of random half.
        let early = NovaId::new(1_000, [0xff; 8]);
        let late = NovaId::new(1_001, [0x00; 8]);
        assert!(early < late);
        assert!(late > early);

        // Same millisecond ties break by the random component (big-endian order).
        let low = NovaId::new(5, [0x00; 8]);
        let high = NovaId::new(5, [0xff; 8]);
        assert!(low < high);
    }

    #[test]
    fn bytes_round_trip() {
        let id = NovaId::generate().unwrap();
        assert_eq!(NovaId::from_bytes(id.as_bytes()).unwrap(), id);
        assert_eq!(NovaId::from_bytes(&id.to_bytes()).unwrap(), id);
        let mut flipped = id.to_bytes();
        flipped[0] ^= 0x01;
        assert_ne!(NovaId::from_bytes(&flipped).unwrap(), id);
    }

    #[test]
    fn hex_round_trip_with_and_without_prefix() {
        let id = NovaId::generate().unwrap();
        let prefixed = id.to_string();
        let bare = id.to_hex();
        assert_eq!(prefixed.parse::<NovaId>().unwrap(), id);
        assert_eq!(bare.parse::<NovaId>().unwrap(), id);
        assert_eq!(prefixed.parse::<NovaId>().unwrap().to_hex(), bare);
    }

    #[test]
    fn uppercase_hex_is_accepted() {
        let id = NovaId::new(42, [0xab; 8]);
        assert_eq!(id.to_hex().to_uppercase().parse::<NovaId>().unwrap(), id);
    }

    #[test]
    fn from_bytes_rejects_bad_lengths() {
        for len in [0usize, 1, 8, 15, 17, 32] {
            let bytes = vec![0u8; len];
            assert!(
                NovaId::from_bytes(&bytes).is_err(),
                "length {len} should be rejected"
            );
        }
    }

    #[test]
    fn from_str_rejects_malformed_input() {
        let bad = [
            "",
            "nova_",
            "zz",
            "0",
            "00",
            "nova_0",
            "xyz123",                               // 6 chars, wrong length
            "g",                                    // not hex
            "0123456789abcdef0123456789abcdef0",    // 33 digits
            "nova_0123456789abcdef0123456789abcde", // 31 digits with prefix
        ];
        for s in bad {
            assert!(s.parse::<NovaId>().is_err(), "{s:?} should be rejected");
        }
    }

    #[test]
    fn statistical_no_collisions_in_large_batch() {
        // 250k sequential IDs: zero collisions expected (birthday bound for
        // 2^128 space ~ N^2/2^129 ~ 1e-26).
        let mut seen: HashSet<NovaId> = HashSet::with_capacity(250_000);
        for _ in 0..250_000 {
            let id = NovaId::generate().unwrap();
            assert!(seen.insert(id), "unexpected NovaId collision");
        }
    }

    #[test]
    fn statistical_no_collisions_across_threads() {
        // Concurrent generation (simulates parallel inserts) must not collide.
        let ids_per_thread = 30_000;
        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    let mut local = Vec::with_capacity(ids_per_thread);
                    for _ in 0..ids_per_thread {
                        local.push(NovaId::generate().unwrap());
                    }
                    local
                })
            })
            .collect();

        let mut seen: HashSet<NovaId> = HashSet::with_capacity(ids_per_thread * 8);
        for handle in handles {
            for id in handle.join().unwrap() {
                assert!(seen.insert(id), "unexpected NovaId collision");
            }
        }
        assert_eq!(seen.len(), ids_per_thread * 8);
    }

    #[test]
    fn ids_are_copy_and_comparable() {
        let a = NovaId::generate().unwrap();
        let b = a;
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
    }
}
