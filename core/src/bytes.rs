//! Sequential little-endian reads, the port of `Reader` in `src/tzx/bytes.ts`.

/// Reader errors carry the same text as the TypeScript ones, because the parser
/// puts them in user-visible warnings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError(pub String);

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type ReadResult<T> = Result<T, ReadError>;

pub struct Reader<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub fn eof(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn need(&self, n: usize) -> ReadResult<()> {
        if self.pos + n > self.buf.len() {
            return Err(ReadError(format!("Unexpected end of file at offset {}", self.pos)));
        }
        Ok(())
    }

    pub fn u8(&mut self) -> ReadResult<u8> {
        self.need(1)?;
        let v = self.buf[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn u16(&mut self) -> ReadResult<u16> {
        self.need(2)?;
        let v = u16::from_le_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    pub fn i16(&mut self) -> ReadResult<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u24(&mut self) -> ReadResult<u32> {
        self.need(3)?;
        let b = &self.buf[self.pos..self.pos + 3];
        self.pos += 3;
        Ok(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
    }

    pub fn u32(&mut self) -> ReadResult<u32> {
        self.need(4)?;
        let b = &self.buf[self.pos..self.pos + 4];
        self.pos += 4;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn bytes(&mut self, n: usize) -> ReadResult<Vec<u8>> {
        self.need(n)?;
        let v = self.buf[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(v)
    }

    /// TZX text is Latin-1; one byte becomes one char, as in `bytesToLatin1`.
    pub fn str(&mut self, n: usize) -> ReadResult<String> {
        Ok(latin1_to_string(&self.bytes(n)?))
    }
}

pub fn latin1_to_string(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// Inverse of [`latin1_to_string`]; chars above 0xff are truncated, as in
/// `latin1ToBytes`.
pub fn string_to_latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

/// Sequential little-endian writes, the port of `Writer` in `src/tzx/bytes.ts`.
/// Widths wrap the way the TypeScript ones do, so an over-long block writes the
/// same truncated length here as it did there.
#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i16(&mut self, v: i16) {
        self.u16(v as u16);
    }

    /// Low three bytes, as a TZX 24-bit length.
    pub fn u24(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes()[..3]);
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn str(&mut self, s: &str) {
        let b = string_to_latin1(s);
        self.bytes(&b);
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}
