//! Global format version registry for persisted NovaDB data.
//!
//! Every persistent artifact NovaDB writes (data pages, `WAL` records, collection
//! catalogs, index files) is identified by [`MAGIC`] plus a format [`FORMAT_VERSION`].
//! Readers must reject unknown magic/version combinations cleanly instead of
//! interpreting data they do not understand.

/// Magic bytes prefixing every NovaDB persistent artifact.
pub const MAGIC: [u8; 4] = *b"NOVA";

/// Current NovaDB on-disk format version.
///
/// Bump this value only on a deliberate, documented format change. Files written
/// with a different `FORMAT_VERSION` must be rejected by readers.
pub const FORMAT_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_is_four_ascii_bytes() {
        assert_eq!(&MAGIC, b"NOVA");
    }

    #[test]
    fn version_matches_registry() {
        // Guard against unintentional version drift during development.
        assert_eq!(FORMAT_VERSION, 1);
    }
}
