use std::io::{Read, Write};

use nova_core::error::{NovaError, Result};

const REQUEST_MAGIC: [u8; 4] = *b"NVRQ";
const RESPONSE_MAGIC: [u8; 4] = *b"NVRS";
const VERSION: u16 = 1;
const HEADER_SIZE: usize = 24;
const CHECKSUM_OFFSET: usize = 20;

/// One client query request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub request_id: u64,
    pub query: String,
}

/// Stable wire error categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorCode {
    Protocol = 1,
    Parse = 2,
    Execution = 3,
    Busy = 4,
    Internal = 5,
}

/// One server response, correlated by request id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub request_id: u64,
    pub outcome: std::result::Result<String, (ErrorCode, String)>,
}

/// Encodes a request frame.
///
/// # Errors
/// Returns a typed error if the query length exceeds the wire representation.
pub fn encode_request(request: &Request) -> Result<Vec<u8>> {
    encode_frame(
        REQUEST_MAGIC,
        0,
        request.request_id,
        request.query.as_bytes(),
    )
}

/// Decodes one request from a reader with a configured payload limit.
///
/// # Errors
/// Returns a typed I/O, protocol, checksum, size, or UTF-8 error.
pub fn read_request(reader: &mut impl Read, maximum: usize) -> Result<Request> {
    let (status, request_id, payload) = read_frame(reader, REQUEST_MAGIC, maximum)?;
    if status != 0 {
        return corruption("request status byte must be zero");
    }
    let query = String::from_utf8(payload)
        .map_err(|_| NovaError::InvalidArgument("request query is not UTF-8".to_owned()))?;
    Ok(Request { request_id, query })
}

/// Writes one response frame.
///
/// # Errors
/// Returns a typed encoding or I/O error.
pub fn write_response(writer: &mut impl Write, response: &Response) -> Result<()> {
    let (status, payload) = match &response.outcome {
        Ok(value) => (0, value.as_bytes()),
        Err((code, message)) => (*code as u8, message.as_bytes()),
    };
    writer.write_all(&encode_frame(
        RESPONSE_MAGIC,
        status,
        response.request_id,
        payload,
    )?)?;
    writer.flush()?;
    Ok(())
}

/// Reads one response frame.
///
/// # Errors
/// Returns a typed I/O, protocol, checksum, size, or UTF-8 error.
pub fn read_response(reader: &mut impl Read, maximum: usize) -> Result<Response> {
    let (status, request_id, payload) = read_frame(reader, RESPONSE_MAGIC, maximum)?;
    let text = String::from_utf8(payload)
        .map_err(|_| NovaError::Corruption("response payload is not UTF-8".to_owned()))?;
    let outcome = if status == 0 {
        Ok(text)
    } else {
        let code = match status {
            1 => ErrorCode::Protocol,
            2 => ErrorCode::Parse,
            3 => ErrorCode::Execution,
            4 => ErrorCode::Busy,
            5 => ErrorCode::Internal,
            other => return corruption(&format!("unknown response status {other}")),
        };
        Err((code, text))
    };
    Ok(Response {
        request_id,
        outcome,
    })
}

fn encode_frame(magic: [u8; 4], status: u8, request_id: u64, payload: &[u8]) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(HEADER_SIZE + payload.len());
    bytes.extend_from_slice(&magic);
    bytes.extend_from_slice(&VERSION.to_be_bytes());
    bytes.push(status);
    bytes.push(0);
    bytes.extend_from_slice(&request_id.to_be_bytes());
    bytes.extend_from_slice(
        &u32::try_from(payload.len())
            .map_err(size_error)?
            .to_be_bytes(),
    );
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(payload);
    let checksum = crc32(&bytes);
    bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&checksum.to_be_bytes());
    Ok(bytes)
}

fn read_frame(
    reader: &mut impl Read,
    expected_magic: [u8; 4],
    maximum: usize,
) -> Result<(u8, u64, Vec<u8>)> {
    let mut header = [0_u8; HEADER_SIZE];
    reader.read_exact(&mut header)?;
    if header[..4] != expected_magic {
        return corruption("invalid protocol magic");
    }
    let version = u16::from_be_bytes(header[4..6].try_into().map_err(fixed_error)?);
    if version != VERSION {
        return Err(NovaError::Unsupported(format!(
            "protocol version {version}"
        )));
    }
    if header[7] != 0 {
        return corruption("protocol reserved byte is non-zero");
    }
    let request_id = u64::from_be_bytes(header[8..16].try_into().map_err(fixed_error)?);
    let length = usize::try_from(u32::from_be_bytes(
        header[16..20].try_into().map_err(fixed_error)?,
    ))
    .map_err(size_error)?;
    if length > maximum {
        return Err(NovaError::InvalidArgument(format!(
            "protocol payload length {length} exceeds limit {maximum}"
        )));
    }
    let stored = u32::from_be_bytes(header[20..24].try_into().map_err(fixed_error)?);
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    let mut encoded = header.to_vec();
    encoded.extend_from_slice(&payload);
    if stored != crc32(&encoded) {
        return corruption("protocol checksum mismatch");
    }
    Ok((header[6], request_id, payload))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for (index, byte) in bytes.iter().copied().enumerate() {
        crc ^= u32::from(if (CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4).contains(&index) {
            0
        } else {
            byte
        });
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn corruption<T>(message: &str) -> Result<T> {
    Err(NovaError::Corruption(message.to_owned()))
}

fn fixed_error<T>(_: T) -> NovaError {
    NovaError::Internal("fixed-width protocol decode failed".to_owned())
}

fn size_error(error: impl std::fmt::Display) -> NovaError {
    NovaError::InvalidArgument(format!("protocol size is not representable: {error}"))
}
