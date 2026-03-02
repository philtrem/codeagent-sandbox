use crate::error::P9Error;
use crate::qid::Qid;

/// Maximum message size the server will accept (16 MB safety limit).
/// This is an absolute upper bound; the negotiated `msize` is typically smaller.
pub const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024;

/// Minimum message size: 4 (size) + 1 (type) + 2 (tag) = 7 bytes.
pub const MIN_MESSAGE_SIZE: u32 = 7;

/// Size of the message header: 4 (size) + 1 (type) + 2 (tag).
pub const HEADER_SIZE: u32 = 7;

/// Cursor-based reader for 9P wire format data.
///
/// Reads little-endian integers, length-prefixed strings, QIDs, and raw data
/// from a byte slice. Tracks position and validates that sufficient bytes remain
/// before each read.
pub struct WireReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> WireReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Returns the number of unread bytes remaining.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// Returns the current read position.
    pub fn position(&self) -> usize {
        self.pos
    }

    fn ensure_remaining(&self, count: usize) -> Result<(), P9Error> {
        if self.remaining() < count {
            return Err(P9Error::MalformedMessage {
                reason: format!(
                    "unexpected end of message: need {count} bytes but only {} remain",
                    self.remaining()
                ),
            });
        }
        Ok(())
    }

    pub fn read_u8(&mut self) -> Result<u8, P9Error> {
        self.ensure_remaining(1)?;
        let value = self.buf[self.pos];
        self.pos += 1;
        Ok(value)
    }

    pub fn read_u16(&mut self) -> Result<u16, P9Error> {
        self.ensure_remaining(2)?;
        let value = u16::from_le_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Ok(value)
    }

    pub fn read_u32(&mut self) -> Result<u32, P9Error> {
        self.ensure_remaining(4)?;
        let value = u32::from_le_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(value)
    }

    pub fn read_u64(&mut self) -> Result<u64, P9Error> {
        self.ensure_remaining(8)?;
        let value = u64::from_le_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
            self.buf[self.pos + 4],
            self.buf[self.pos + 5],
            self.buf[self.pos + 6],
            self.buf[self.pos + 7],
        ]);
        self.pos += 8;
        Ok(value)
    }

    /// Read a 9P string: u16 length prefix followed by UTF-8 bytes.
    pub fn read_string(&mut self) -> Result<String, P9Error> {
        let len = self.read_u16()? as usize;
        self.ensure_remaining(len)?;
        let bytes = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        String::from_utf8(bytes.to_vec()).map_err(|_| P9Error::MalformedMessage {
            reason: "string is not valid UTF-8".to_string(),
        })
    }

    /// Read a QID (13 bytes: u8 type + u32 version + u64 path).
    pub fn read_qid(&mut self) -> Result<Qid, P9Error> {
        let ty = self.read_u8()?;
        let version = self.read_u32()?;
        let path = self.read_u64()?;
        Ok(Qid { ty, version, path })
    }

    /// Read a 9P data blob: u32 length prefix followed by raw bytes.
    pub fn read_data(&mut self) -> Result<Vec<u8>, P9Error> {
        let len = self.read_u32()? as usize;
        self.ensure_remaining(len)?;
        let data = self.buf[self.pos..self.pos + len].to_vec();
        self.pos += len;
        Ok(data)
    }

    /// Read raw bytes without a length prefix.
    pub fn read_raw(&mut self, count: usize) -> Result<Vec<u8>, P9Error> {
        self.ensure_remaining(count)?;
        let data = self.buf[self.pos..self.pos + count].to_vec();
        self.pos += count;
        Ok(data)
    }

    /// Read multiple QIDs (prefixed by u16 count).
    pub fn read_qids(&mut self) -> Result<Vec<Qid>, P9Error> {
        let count = self.read_u16()? as usize;
        let mut qids = Vec::with_capacity(count);
        for _ in 0..count {
            qids.push(self.read_qid()?);
        }
        Ok(qids)
    }

    /// Read multiple strings (prefixed by u16 count).
    pub fn read_strings(&mut self) -> Result<Vec<String>, P9Error> {
        let count = self.read_u16()? as usize;
        let mut strings = Vec::with_capacity(count);
        for _ in 0..count {
            strings.push(self.read_string()?);
        }
        Ok(strings)
    }
}

/// Cursor-based writer for 9P wire format data.
///
/// Writes little-endian integers, length-prefixed strings, QIDs, and raw data
/// to an internal buffer. The `finish()` method prepends the 4-byte size field
/// (including the size field itself).
pub struct WireWriter {
    buf: Vec<u8>,
}

impl WireWriter {
    /// Create a new writer. The first 4 bytes are reserved for the size field
    /// and will be filled in by `finish()`.
    pub fn new() -> Self {
        Self {
            buf: vec![0, 0, 0, 0], // placeholder for size
        }
    }

    pub fn write_u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    pub fn write_u16(&mut self, value: u16) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Write a 9P string: u16 length prefix followed by UTF-8 bytes.
    pub fn write_string(&mut self, value: &str) {
        let bytes = value.as_bytes();
        self.write_u16(bytes.len() as u16);
        self.buf.extend_from_slice(bytes);
    }

    /// Write a QID (13 bytes: u8 type + u32 version + u64 path).
    pub fn write_qid(&mut self, qid: &Qid) {
        self.write_u8(qid.ty);
        self.write_u32(qid.version);
        self.write_u64(qid.path);
    }

    /// Write a 9P data blob: u32 length prefix followed by raw bytes.
    pub fn write_data(&mut self, data: &[u8]) {
        self.write_u32(data.len() as u32);
        self.buf.extend_from_slice(data);
    }

    /// Write raw bytes without a length prefix.
    pub fn write_raw(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Write multiple QIDs (prefixed by u16 count).
    pub fn write_qids(&mut self, qids: &[Qid]) {
        self.write_u16(qids.len() as u16);
        for qid in qids {
            self.write_qid(qid);
        }
    }

    /// Write multiple strings (prefixed by u16 count).
    pub fn write_strings(&mut self, strings: &[String]) {
        self.write_u16(strings.len() as u16);
        for s in strings {
            self.write_string(s);
        }
    }

    /// Finalize the message: fill in the 4-byte size prefix and return the buffer.
    /// The size includes the 4 size bytes themselves.
    pub fn finish(mut self) -> Vec<u8> {
        let size = self.buf.len() as u32;
        self.buf[0..4].copy_from_slice(&size.to_le_bytes());
        self.buf
    }

    /// Returns the current buffer length (including the 4-byte size placeholder).
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Returns true if the writer is empty (only the size placeholder).
    pub fn is_empty(&self) -> bool {
        self.buf.len() <= 4
    }
}

impl Default for WireWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate an incoming message size field.
///
/// Checks that the size is at least `MIN_MESSAGE_SIZE` and does not exceed
/// the given maximum. Returns an error before any allocation occurs.
pub fn validate_message_size(size: u32, max_size: u32) -> Result<(), P9Error> {
    if size < MIN_MESSAGE_SIZE {
        return Err(P9Error::MalformedMessage {
            reason: format!(
                "message size {size} is below minimum ({MIN_MESSAGE_SIZE})"
            ),
        });
    }
    if size > max_size {
        return Err(P9Error::OversizedMessage {
            size,
            max_size,
        });
    }
    Ok(())
}

/// Parse the common message header (type + tag) from a buffer that has
/// already had its 4-byte size prefix removed.
pub fn parse_header(buf: &[u8]) -> Result<(u8, u16), P9Error> {
    if buf.len() < 3 {
        return Err(P9Error::MalformedMessage {
            reason: "message too short for header".to_string(),
        });
    }
    let msg_type = buf[0];
    let tag = u16::from_le_bytes([buf[1], buf[2]]);
    Ok((msg_type, tag))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qid::qid_type;

    #[test]
    fn write_and_read_u8() {
        let mut writer = WireWriter::new();
        writer.write_u8(42);
        let buf = writer.finish();

        let mut reader = WireReader::new(&buf[4..]); // skip size
        assert_eq!(reader.read_u8().unwrap(), 42);
    }

    #[test]
    fn write_and_read_u16() {
        let mut writer = WireWriter::new();
        writer.write_u16(0xBEEF);
        let buf = writer.finish();

        let mut reader = WireReader::new(&buf[4..]);
        assert_eq!(reader.read_u16().unwrap(), 0xBEEF);
    }

    #[test]
    fn write_and_read_u32() {
        let mut writer = WireWriter::new();
        writer.write_u32(0xDEADBEEF);
        let buf = writer.finish();

        let mut reader = WireReader::new(&buf[4..]);
        assert_eq!(reader.read_u32().unwrap(), 0xDEADBEEF);
    }

    #[test]
    fn write_and_read_u64() {
        let mut writer = WireWriter::new();
        writer.write_u64(0x0102030405060708);
        let buf = writer.finish();

        let mut reader = WireReader::new(&buf[4..]);
        assert_eq!(reader.read_u64().unwrap(), 0x0102030405060708);
    }

    #[test]
    fn write_and_read_string() {
        let mut writer = WireWriter::new();
        writer.write_string("hello");
        let buf = writer.finish();

        let mut reader = WireReader::new(&buf[4..]);
        assert_eq!(reader.read_string().unwrap(), "hello");
    }

    #[test]
    fn write_and_read_empty_string() {
        let mut writer = WireWriter::new();
        writer.write_string("");
        let buf = writer.finish();

        let mut reader = WireReader::new(&buf[4..]);
        assert_eq!(reader.read_string().unwrap(), "");
    }

    #[test]
    fn write_and_read_qid() {
        let qid = Qid::new(qid_type::QTDIR, 7, 12345);
        let mut writer = WireWriter::new();
        writer.write_qid(&qid);
        let buf = writer.finish();

        let mut reader = WireReader::new(&buf[4..]);
        assert_eq!(reader.read_qid().unwrap(), qid);
    }

    #[test]
    fn write_and_read_data() {
        let data = b"payload bytes";
        let mut writer = WireWriter::new();
        writer.write_data(data);
        let buf = writer.finish();

        let mut reader = WireReader::new(&buf[4..]);
        assert_eq!(reader.read_data().unwrap(), data);
    }

    #[test]
    fn write_and_read_qids() {
        let qids = vec![
            Qid::file(1, 10),
            Qid::directory(2, 20),
            Qid::symlink(3, 30),
        ];
        let mut writer = WireWriter::new();
        writer.write_qids(&qids);
        let buf = writer.finish();

        let mut reader = WireReader::new(&buf[4..]);
        assert_eq!(reader.read_qids().unwrap(), qids);
    }

    #[test]
    fn write_and_read_strings() {
        let strings: Vec<String> = vec!["foo".into(), "bar".into(), "baz".into()];
        let mut writer = WireWriter::new();
        writer.write_strings(&strings);
        let buf = writer.finish();

        let mut reader = WireReader::new(&buf[4..]);
        assert_eq!(reader.read_strings().unwrap(), strings);
    }

    #[test]
    fn finish_fills_size_field() {
        let mut writer = WireWriter::new();
        writer.write_u8(100); // type
        writer.write_u16(0); // tag
        let buf = writer.finish();

        let size = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        assert_eq!(size, buf.len() as u32);
        assert_eq!(size, 7); // 4 (size) + 1 (type) + 2 (tag)
    }

    #[test]
    fn reader_insufficient_data_returns_error() {
        let buf = [0u8; 1];
        let mut reader = WireReader::new(&buf);
        assert!(reader.read_u32().is_err());
    }

    #[test]
    fn reader_string_truncated_data_returns_error() {
        // Claim string is 100 bytes but only provide 2 (the length prefix)
        let buf = [100u8, 0];
        let mut reader = WireReader::new(&buf);
        assert!(reader.read_string().is_err());
    }

    #[test]
    fn reader_invalid_utf8_string_returns_error() {
        // Length = 2, then invalid UTF-8 bytes
        let buf = [2u8, 0, 0xFF, 0xFE];
        let mut reader = WireReader::new(&buf);
        assert!(reader.read_string().is_err());
    }

    #[test]
    fn validate_message_size_too_small() {
        let result = validate_message_size(3, MAX_MESSAGE_SIZE);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, P9Error::MalformedMessage { .. }));
    }

    #[test]
    fn validate_message_size_too_large() {
        let result = validate_message_size(MAX_MESSAGE_SIZE + 1, MAX_MESSAGE_SIZE);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, P9Error::OversizedMessage { .. }));
    }

    #[test]
    fn validate_message_size_minimum_ok() {
        assert!(validate_message_size(MIN_MESSAGE_SIZE, MAX_MESSAGE_SIZE).is_ok());
    }

    #[test]
    fn validate_message_size_maximum_ok() {
        assert!(validate_message_size(MAX_MESSAGE_SIZE, MAX_MESSAGE_SIZE).is_ok());
    }

    #[test]
    fn parse_header_extracts_type_and_tag() {
        let buf = [100u8, 0xFF, 0xFF]; // type=100, tag=0xFFFF (NOTAG)
        let (msg_type, tag) = parse_header(&buf).unwrap();
        assert_eq!(msg_type, 100);
        assert_eq!(tag, 0xFFFF);
    }

    #[test]
    fn parse_header_too_short() {
        let buf = [100u8, 0xFF];
        assert!(parse_header(&buf).is_err());
    }
}
