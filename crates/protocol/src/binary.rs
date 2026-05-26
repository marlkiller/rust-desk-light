use crate::error::ProtocolError;

#[derive(Default)]
pub(crate) struct BinaryWriter {
    buffer: Vec<u8>,
}

impl BinaryWriter {
    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.buffer
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.buffer.push(value);
    }

    pub(crate) fn bool(&mut self, value: bool) {
        self.u8(if value { 1 } else { 0 });
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn i32(&mut self, value: i32) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn u128(&mut self, value: u128) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn string(&mut self, value: &str) {
        self.u32(value.len() as u32);
        self.buffer.extend_from_slice(value.as_bytes());
    }

    pub(crate) fn byte_vec(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.buffer.extend_from_slice(value);
    }
}

pub(crate) struct BinaryReader<'a> {
    buf: &'a [u8],
}

impl<'a> BinaryReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { buf: data }
    }

    pub(crate) fn finish(&self) -> Result<(), ProtocolError> {
        let trailing = self.buf.len();
        if trailing == 0 {
            Ok(())
        } else {
            Err(ProtocolError::TrailingBytes(trailing))
        }
    }

    pub(crate) fn bytes(&mut self, len: usize) -> Result<&'a [u8], ProtocolError> {
        if self.buf.len() < len {
            return Err(ProtocolError::UnexpectedEof);
        }
        let (data, rest) = self.buf.split_at(len);
        self.buf = rest;
        Ok(data)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, ProtocolError> {
        if self.buf.is_empty() {
            return Err(ProtocolError::UnexpectedEof);
        }
        let v = self.buf[0];
        self.buf = &self.buf[1..];
        Ok(v)
    }

    pub(crate) fn bool(&mut self) -> Result<bool, ProtocolError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(ProtocolError::InvalidBool(value)),
        }
    }

    pub(crate) fn u16(&mut self) -> Result<u16, ProtocolError> {
        if self.buf.len() < 2 {
            return Err(ProtocolError::UnexpectedEof);
        }
        let v = u16::from_be_bytes([self.buf[0], self.buf[1]]);
        self.buf = &self.buf[2..];
        Ok(v)
    }

    pub(crate) fn i32(&mut self) -> Result<i32, ProtocolError> {
        if self.buf.len() < 4 {
            return Err(ProtocolError::UnexpectedEof);
        }
        let v = i32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        self.buf = &self.buf[4..];
        Ok(v)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, ProtocolError> {
        if self.buf.len() < 4 {
            return Err(ProtocolError::UnexpectedEof);
        }
        let v = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        self.buf = &self.buf[4..];
        Ok(v)
    }

    pub(crate) fn u64(&mut self) -> Result<u64, ProtocolError> {
        if self.buf.len() < 8 {
            return Err(ProtocolError::UnexpectedEof);
        }
        let v = u64::from_be_bytes([
            self.buf[0], self.buf[1], self.buf[2], self.buf[3],
            self.buf[4], self.buf[5], self.buf[6], self.buf[7],
        ]);
        self.buf = &self.buf[8..];
        Ok(v)
    }

    pub(crate) fn u128(&mut self) -> Result<u128, ProtocolError> {
        if self.buf.len() < 16 {
            return Err(ProtocolError::UnexpectedEof);
        }
        let v = u128::from_be_bytes([
            self.buf[0], self.buf[1], self.buf[2], self.buf[3],
            self.buf[4], self.buf[5], self.buf[6], self.buf[7],
            self.buf[8], self.buf[9], self.buf[10], self.buf[11],
            self.buf[12], self.buf[13], self.buf[14], self.buf[15],
        ]);
        self.buf = &self.buf[16..];
        Ok(v)
    }

    pub(crate) fn string(&mut self) -> Result<String, ProtocolError> {
        let len = self.u32()? as usize;
        let bytes = self.bytes(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtocolError::InvalidUtf8)
    }

    pub(crate) fn byte_vec(&mut self) -> Result<Vec<u8>, ProtocolError> {
        let len = self.u32()? as usize;
        let data = self.bytes(len)?;
        Ok(data.to_vec())
    }
}
