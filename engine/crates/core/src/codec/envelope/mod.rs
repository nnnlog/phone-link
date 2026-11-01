//! The binary inner envelope: one serializer for App and Platform messages; all integers
//! little-endian.
//!
//! ```text
//! 0  1  version            0x01 (reader ignores the value)
//! 1  4  headersBlobLength  int32 = 4 + Σ (4 + keyLen + 4 + valueLen)   ← the 4-byte count is included
//! 5  4  headerCount        int32
//! 9  …  headers            headerCount × { int32 keyLen, key, int32 valueLen, value }  (UTF-8)
//! …  8  payloadLength      int64
//! …  …  payload
//! ```

use crate::types::HeaderMap;

/// Envelope version byte written by the PC.
pub const ENVELOPE_VERSION: u8 = 0x01;

/// A decoded envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub headers: HeaderMap,
    pub payload: Vec<u8>,
}

/// Why an envelope is malformed under the reader rules. Detected only after reassembly, i.e. after
/// every fragment was acked; the message is logged and discarded, no reply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    #[error("envelope truncated at {0}")]
    Truncated(&'static str),
    #[error("headersBlobLength {0} is not positive")]
    NonPositiveHeadersBlobLength(i32),
    #[error("headersBlobLength {0} exceeds the message")]
    HeadersBlobBeyondMessage(i32),
    #[error("headerCount {0} is not positive")]
    NonPositiveHeaderCount(i32),
    #[error("negative header string length {0}")]
    NegativeStringLength(i32),
    #[error("duplicate header key {0:?}")]
    DuplicateHeader(String),
    #[error("negative payloadLength {0}")]
    NegativePayloadLength(i64),
    /// Writer precondition: every message the PC sends has at least one header.
    #[error("an envelope must carry at least one header")]
    NoHeaders,
}

/// Serializes an envelope (writer rules: headers in map order; `headersBlobLength` includes the count).
pub fn encode(headers: &HeaderMap, payload: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
    if headers.is_empty() {
        return Err(EnvelopeError::NoHeaders);
    }

    let blob: usize = 4 + headers.iter().map(|(k, v)| 8 + k.len() + v.len()).sum::<usize>();
    let blob = i32::try_from(blob).expect("header blob larger than 2 GiB");
    let count = i32::try_from(headers.len()).expect("too many headers");

    let mut out = Vec::with_capacity(1 + 4 + blob as usize + 8 + payload.len());
    out.push(ENVELOPE_VERSION);
    out.extend_from_slice(&blob.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());

    for (k, v) in headers.iter() {
        out.extend_from_slice(&(k.len() as i32).to_le_bytes());
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(&(v.len() as i32).to_le_bytes());
        out.extend_from_slice(v.as_bytes());
    }

    out.extend_from_slice(&(payload.len() as i64).to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], EnvelopeError> {
        let end = self.pos.checked_add(n).ok_or(EnvelopeError::Truncated(what))?;
        let slice = self.buf.get(self.pos..end).ok_or(EnvelopeError::Truncated(what))?;
        self.pos = end;
        Ok(slice)
    }
    fn i32(&mut self, what: &'static str) -> Result<i32, EnvelopeError> {
        Ok(i32::from_le_bytes(self.take(4, what)?.try_into().expect("4 bytes")))
    }
    fn i64(&mut self, what: &'static str) -> Result<i64, EnvelopeError> {
        Ok(i64::from_le_bytes(self.take(8, what)?.try_into().expect("8 bytes")))
    }
    fn string(&mut self, what: &'static str) -> Result<String, EnvelopeError> {
        let len = self.i32(what)?;
        if len < 0 {
            return Err(EnvelopeError::NegativeStringLength(len));
        }
        // Lossy: invalid sequences become U+FFFD.
        Ok(String::from_utf8_lossy(self.take(len as usize, what)?).into_owned())
    }
}

/// Parses a reassembled envelope.
///
/// The version byte is ignored; `headersBlobLength` must be positive and only bounds how many bytes
/// must be present (so both the include-the-count and the exclude-the-count conventions parse);
/// zero headers and duplicate keys are malformed; the payload is the next `payloadLength` bytes.
///
/// Bytes after the payload are ignored.
pub fn decode(bytes: &[u8]) -> Result<Envelope, EnvelopeError> {
    let mut r = Reader { buf: bytes, pos: 0 };
    let _version = r.take(1, "version")?;

    let blob = r.i32("headersBlobLength")?;
    if blob <= 0 {
        return Err(EnvelopeError::NonPositiveHeadersBlobLength(blob));
    }
    if (blob as usize) > bytes.len() - r.pos {
        return Err(EnvelopeError::HeadersBlobBeyondMessage(blob));
    }

    let count = r.i32("headerCount")?;
    if count <= 0 {
        return Err(EnvelopeError::NonPositiveHeaderCount(count));
    }

    let mut headers = HeaderMap::new();
    for _ in 0..count {
        let key = r.string("header key")?;
        let value = r.string("header value")?;
        if headers.contains(&key) {
            return Err(EnvelopeError::DuplicateHeader(key));
        }
        headers.insert(key, value);
    }

    let len = r.i64("payloadLength")?;
    if len < 0 {
        return Err(EnvelopeError::NegativePayloadLength(len));
    }
    let len = usize::try_from(len).map_err(|_| EnvelopeError::Truncated("payload"))?;
    let payload = r.take(len, "payload")?.to_vec();
    Ok(Envelope { headers, payload })
}

#[cfg(test)]
mod tests;
