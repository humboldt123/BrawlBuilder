//! Wii DOL image: 7 text + 11 data sections (offsets @0x00, addresses @0x48, sizes @0x90).

use crate::bin::{get32, put32, require, Result};

pub struct Dol {
    d: Vec<u8>,
    pub off: [u32; 18],
    pub addr: [u32; 18],
    pub size: [u32; 18],
}

impl Dol {
    pub fn new(b: &[u8]) -> Self {
        let mut dol = Dol { d: b.to_vec(), off: [0; 18], addr: [0; 18], size: [0; 18] };
        for i in 0..18 {
            dol.off[i] = get32(b, 4 * i);
            dol.addr[i] = get32(b, 0x48 + 4 * i);
            dol.size[i] = get32(b, 0x90 + 4 * i);
        }
        dol
    }

    pub fn section(&self, va: u32) -> Option<usize> {
        (0..18).find(|&i| self.size[i] != 0 && self.addr[i] <= va && (va as u64) < self.addr[i] as u64 + self.size[i] as u64)
    }

    fn file_off(&self, va: u32) -> Result<usize> {
        let i = self.section(va);
        require(i.is_some(), format!("address {va:08X} is not inside main.dol"))?;
        let i = i.unwrap();
        Ok((self.off[i] + va - self.addr[i]) as usize)
    }

    pub fn read(&self, va: u32) -> Result<u32> {
        let o = self.file_off(va)?;
        Ok(get32(&self.d, o))
    }

    /// read for callers that cannot fail (addresses outside main.dol read as 0)
    pub fn read_or0(&self, va: u32) -> u32 {
        self.read(va).unwrap_or(0)
    }

    pub fn write(&mut self, va: u32, w: u32, old: Option<u32>) -> Result<()> {
        if let Some(old) = old {
            let cur = self.read(va)?;
            require(cur == old, format!("main.dol @{va:08X}: expected {old:08X} found {cur:08X} (not Brawl USA Rev 2?)"))?;
        }
        let o = self.file_off(va)?;
        put32(&mut self.d, o, w);
        Ok(())
    }

    pub fn write_bytes(&mut self, va: u32, b: &[u8]) -> Result<()> {
        for (k, &v) in b.iter().enumerate() {
            let o = self.file_off(va + k as u32)?;
            self.d[o] = v;
        }
        Ok(())
    }

    pub fn covers(&self, va: u32, n: usize) -> bool {
        (0..n).all(|k| self.section(va.wrapping_add(k as u32)).is_some())
    }

    pub fn free(&self, va: u32, n: usize) -> bool {
        (0..n).all(|k| self.section(va.wrapping_add(k as u32)).is_none())
    }

    /// append a new section (data may also use a spare text slot)
    pub fn add(&mut self, va: u32, blob: &[u8], text: bool) -> Result<()> {
        let slots: Vec<usize> = if text { (0..7).collect() } else { (7..18).chain(0..7).collect() };
        let i = slots.iter().copied().find(|&k| self.size[k] == 0 && !(k == 0 && self.addr[0] != 0)).unwrap_or(0);
        require(self.size[i] == 0, "no free main.dol section left")?;
        while self.d.len() % 32 != 0 {
            self.d.push(0);
        }
        self.off[i] = self.d.len() as u32;
        self.addr[i] = va;
        self.size[i] = ((blob.len() + 31) & !31) as u32;
        self.d.extend_from_slice(blob);
        while self.d.len() % 32 != 0 {
            self.d.push(0);
        }
        Ok(())
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let mut b = self.d.clone();
        for i in 0..18 {
            put32(&mut b, 4 * i, self.off[i]);
            put32(&mut b, 0x48 + 4 * i, self.addr[i]);
            put32(&mut b, 0x90 + 4 * i, self.size[i]);
        }
        b
    }
}
