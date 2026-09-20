use nova_core::error::{NovaError, Result};

pub(crate) const MAX_VARINT_BYTES: usize = 10;

pub(crate) fn write_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value.to_le_bytes()[0] & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value.to_le_bytes()[0]);
}

pub(crate) fn read_varint(input: &[u8]) -> Result<(u64, usize)> {
    let mut value = 0_u64;
    for index in 0..MAX_VARINT_BYTES {
        let byte = *input
            .get(index)
            .ok_or_else(|| NovaError::Corruption("truncated varint".to_owned()))?;
        let payload = u64::from(byte & 0x7f);

        if index == MAX_VARINT_BYTES - 1 && payload > 1 {
            return Err(NovaError::Corruption("varint overflows u64".to_owned()));
        }
        value |= payload << (index * 7);

        if byte & 0x80 == 0 {
            if index > 0 && payload == 0 {
                return Err(NovaError::Corruption("non-minimal varint".to_owned()));
            }
            return Ok((value, index + 1));
        }
    }
    Err(NovaError::Corruption("varint exceeds 10 bytes".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_values_round_trip_minimally() {
        for value in [0, 1, 127, 128, 255, 16_384, u64::from(u32::MAX), u64::MAX] {
            let mut bytes = Vec::new();
            write_varint(value, &mut bytes);
            assert_eq!(read_varint(&bytes).unwrap(), (value, bytes.len()));
            assert!(bytes.len() <= MAX_VARINT_BYTES);
            if bytes.len() > 1 {
                assert_ne!(bytes.last(), Some(&0));
            }
        }
    }
}
