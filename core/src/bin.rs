//! Big-endian helpers and the error type.

use std::fmt;

#[derive(Debug, Clone)]
pub struct ExError(pub String);

impl fmt::Display for ExError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExError {}

impl From<std::io::Error> for ExError {
    fn from(e: std::io::Error) -> Self {
        ExError(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ExError>;

pub fn require(ok: bool, msg: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(ExError(msg.into()))
    }
}

pub fn get32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

pub fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

pub fn sha1_hex(d: &[u8]) -> String {
    sha1_smol::Sha1::from(d).digest().to_string()
}

pub fn index_of(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    if n.len() > h.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| &h[i..i + n.len()] == n)
}
