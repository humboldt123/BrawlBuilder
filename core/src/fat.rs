//! Minimal read-only FAT16/FAT32 reader for a raw filesystem image (no mounting). Supports long file names and an optional MBR.
//! Never writes: the image is opened read-only.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::bin::{require, ExError, Result};

#[derive(Clone, Debug)]
pub struct FatEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
    pub first_cluster: u32,
    pub path: String,
}

pub struct FatImage {
    fs: File,
    cluster_bytes: u64,
    root_cluster: u32,
    root_dir_start: u64,
    root_dir_entries: usize,
    data_start: u64,
    fat: Vec<u32>,
    cluster_count: u32,
    is_fat32: bool,
}

fn u16le(b: &[u8], o: usize) -> u32 { u16::from_le_bytes([b[o], b[o + 1]]) as u32 }
fn u32le(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }

fn looks_like_bpb(s: &[u8]) -> bool {
    if s[510] != 0x55 || s[511] != 0xAA {
        return false;
    }
    let bps = u16le(s, 11);
    let spc = s[13] as u32;
    if !matches!(bps, 512 | 1024 | 2048 | 4096) {
        return false;
    }
    if spc == 0 || spc & (spc - 1) != 0 {
        return false;
    }
    s[16] != 0 && u16le(s, 14) != 0
}

impl FatImage {
    pub fn open(path: &Path) -> Result<Self> {
        let mut fs = File::open(path)?;
        let mut s0 = read_at(&mut fs, 0, 512)?;
        let mut vol_start = 0u64;
        if !looks_like_bpb(&s0) {
            // MBR: first non-empty partition
            require(s0[510] == 0x55 && s0[511] == 0xAA, "sd image: neither a FAT boot sector nor an MBR")?;
            let mut found = false;
            for i in 0..4 {
                let o = 446 + 16 * i;
                let ty = s0[o + 4];
                let lba = u32le(&s0, o + 8);
                if ty != 0 && lba != 0 {
                    vol_start = lba as u64 * 512;
                    found = true;
                    break;
                }
            }
            require(found, "sd image: no partition found")?;
            s0 = read_at(&mut fs, vol_start, 512)?;
            require(looks_like_bpb(&s0), "sd image: partition is not FAT")?;
        }
        let bps = u16le(&s0, 11) as u64;
        let spc = s0[13] as u64;
        let reserved = u16le(&s0, 14) as u64;
        let nfats = s0[16] as u64;
        let root_ent = u16le(&s0, 17) as u64;
        let mut tot = u16le(&s0, 19) as u64;
        if tot == 0 {
            tot = u32le(&s0, 32) as u64;
        }
        let mut fat_sz = u16le(&s0, 22) as u64;
        if fat_sz == 0 {
            fat_sz = u32le(&s0, 36) as u64;
        }
        let root_sectors = (root_ent * 32 + bps - 1) / bps;
        let data_sectors = tot as i64 - reserved as i64 - (nfats * fat_sz) as i64 - root_sectors as i64;
        let cluster_count = (data_sectors / spc as i64) as u32;
        require(cluster_count >= 4085, "FAT12 is not supported")?;
        let is_fat32 = cluster_count >= 65525;
        let cluster_bytes = bps * spc;
        let fat_start = vol_start + reserved * bps;
        let root_dir_start = fat_start + nfats * fat_sz * bps;
        let data_start = root_dir_start + root_sectors * bps;
        let root_cluster = if is_fat32 { u32le(&s0, 44) } else { 0 };
        // load FAT 1
        let raw = read_at(&mut fs, fat_start, (fat_sz * bps) as usize)?;
        let fat: Vec<u32> = if is_fat32 {
            raw.chunks_exact(4).map(|c| u32le(c, 0) & 0x0FFFFFFF).collect()
        } else {
            raw.chunks_exact(2).map(|c| u16le(c, 0)).collect()
        };
        Ok(FatImage { fs, cluster_bytes, root_cluster, root_dir_start, root_dir_entries: root_ent as usize, data_start, fat, cluster_count, is_fat32 })
    }

    fn cluster_offset(&self, c: u32) -> u64 {
        self.data_start + (c as u64 - 2) * self.cluster_bytes
    }

    fn is_eoc(&self, v: u32) -> bool {
        if self.is_fat32 { v >= 0x0FFFFFF8 } else { v >= 0xFFF8 }
    }

    fn chain(&self, first: u32) -> Result<Vec<u32>> {
        let mut l = Vec::new();
        let mut c = first;
        while c >= 2 && !self.is_eoc(c) {
            require((c as usize) < self.fat.len() && c < self.cluster_count + 2, "corrupt FAT chain")?;
            l.push(c);
            require(l.len() <= self.cluster_count as usize, "FAT chain loop")?;
            c = self.fat[c as usize];
        }
        Ok(l)
    }

    fn read_dir_bytes(&mut self, cluster: u32, is_root: bool) -> Result<Vec<u8>> {
        if is_root && !self.is_fat32 {
            let (s, n) = (self.root_dir_start, self.root_dir_entries * 32);
            return read_at(&mut self.fs, s, n);
        }
        let cluster = if is_root { self.root_cluster } else { cluster };
        let mut o = Vec::new();
        for c in self.chain(cluster)? {
            let off = self.cluster_offset(c);
            let n = self.cluster_bytes as usize;
            o.extend(read_at(&mut self.fs, off, n)?);
        }
        Ok(o)
    }

    fn parse(&self, d: &[u8], parent: &str) -> Vec<FatEntry> {
        let mut res = Vec::new();
        let mut lfn = vec![0u16; 20 * 13];
        let (mut lfn_count, mut lfn_sum) = (0usize, 0u8);
        let offs = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
        let mut o = 0;
        while o + 32 <= d.len() {
            let b0 = d[o];
            if b0 == 0 {
                break;
            }
            if b0 == 0xE5 {
                lfn_count = 0;
                o += 32;
                continue;
            }
            let attr = d[o + 11];
            if attr == 0x0F {
                let seq = (b0 & 0x1F) as usize;
                if b0 & 0x40 != 0 {
                    lfn_count = seq;
                    lfn_sum = d[o + 13];
                    lfn.iter_mut().for_each(|x| *x = 0);
                }
                if (1..=20).contains(&seq) {
                    for k in 0..13 {
                        lfn[(seq - 1) * 13 + k] = u16le(d, o + offs[k]) as u16;
                    }
                }
                o += 32;
                continue;
            }
            if attr & 0x08 != 0 {
                lfn_count = 0; // volume label
                o += 32;
                continue;
            }
            let mut sum = 0u8;
            for k in 0..11 {
                sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(d[o + k]);
            }
            let name = if lfn_count > 0 && sum == lfn_sum {
                let units: Vec<u16> = lfn[..lfn_count * 13].iter().copied().take_while(|&c| c != 0 && c != 0xFFFF).collect();
                String::from_utf16_lossy(&units)
            } else {
                let mut b = String::from_utf8_lossy(&d[o..o + 8]).trim_end_matches(' ').to_string();
                let mut e = String::from_utf8_lossy(&d[o + 8..o + 11]).trim_end_matches(' ').to_string();
                if d[o + 12] & 0x08 != 0 {
                    b = b.to_lowercase();
                }
                if d[o + 12] & 0x10 != 0 {
                    e = e.to_lowercase();
                }
                if d[o] == 0x05 {
                    b = format!("{}{}", char::from_u32(0xE5).unwrap(), &b[1..]);
                }
                if e.is_empty() { b } else { format!("{b}.{e}") }
            };
            lfn_count = 0;
            if name != "." && name != ".." {
                let mut fc = (u16le(d, o + 20) << 16) | u16le(d, o + 26);
                if !self.is_fat32 {
                    fc &= 0xFFFF;
                }
                let path = if parent.is_empty() { name.clone() } else { format!("{parent}/{name}") };
                res.push(FatEntry { name, is_dir: attr & 0x10 != 0, size: u32le(d, o + 28), first_cluster: fc, path });
            }
            o += 32;
        }
        res
    }

    /// case-insensitive path lookup ("Minusery/module/x.rel")
    pub fn find(&mut self, path: &str) -> Result<Option<FatEntry>> {
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        let (mut cl, mut root, mut cur, mut acc) = (0u32, true, None, String::new());
        for (pi, p) in parts.iter().enumerate() {
            let bytes = self.read_dir_bytes(cl, root)?;
            let hit = self.parse(&bytes, &acc).into_iter().find(|e| e.name.eq_ignore_ascii_case(p));
            let Some(hit) = hit else { return Ok(None) };
            acc = hit.path.clone();
            cl = hit.first_cluster;
            root = false;
            let last = pi == parts.len() - 1;
            let is_dir = hit.is_dir;
            cur = Some(hit);
            if !is_dir && !last {
                return Ok(None);
            }
        }
        Ok(cur)
    }

    /// all files below the image root
    pub fn walk_all(&mut self) -> Result<Vec<FatEntry>> {
        let mut out = Vec::new();
        let root = self.read_dir_bytes(0, true)?;
        let mut stack: Vec<FatEntry> = Vec::new();
        for e in self.parse(&root, "") {
            if e.is_dir { stack.push(e) } else { out.push(e) }
        }
        while let Some(dir) = stack.pop() {
            let bytes = self.read_dir_bytes(dir.first_cluster, false)?;
            for e in self.parse(&bytes, &dir.path) {
                if e.is_dir { stack.push(e) } else { out.push(e) }
            }
        }
        Ok(out)
    }

    pub fn copy_to(&mut self, e: &FatEntry, dst: &mut impl Write) -> Result<()> {
        let mut remaining = e.size as u64;
        if remaining == 0 {
            return Ok(());
        }
        let chain = self.chain(e.first_cluster)?;
        require(chain.len() as u64 * self.cluster_bytes >= remaining, format!("FAT chain shorter than file size: {}", e.path))?;
        let mut buf = vec![0u8; 1 << 20];
        let mut i = 0;
        while i < chain.len() && remaining > 0 {
            let mut run = 1;
            while i + run < chain.len() && chain[i + run] == chain[i + run - 1] + 1 && (run as u64 + 1) * self.cluster_bytes <= buf.len() as u64 {
                run += 1;
            }
            let want = remaining.min(run as u64 * self.cluster_bytes) as usize;
            self.fs.seek(SeekFrom::Start(self.cluster_offset(chain[i])))?;
            self.fs.read_exact(&mut buf[..want])?;
            dst.write_all(&buf[..want])?;
            remaining -= want as u64;
            i += run;
        }
        Ok(())
    }

    /// copy every file of the image (skipping 'System Volume Information') into `dir`
    pub fn extract_all(&mut self, dir: &Path) -> Result<()> {
        for e in self.walk_all()? {
            if e.path.to_ascii_lowercase().starts_with("system volume information") {
                continue;
            }
            let dst = crate::fsutil::join_disc(dir, &e.path);
            fs::create_dir_all(dst.parent().unwrap())?;
            let mut f = File::create(&dst)?;
            self.copy_to(&e, &mut f)?;
        }
        Ok(())
    }
}

fn read_at(f: &mut File, off: u64, len: usize) -> Result<Vec<u8>> {
    let mut b = vec![0u8; len];
    f.seek(SeekFrom::Start(off))?;
    f.read_exact(&mut b).map_err(|_| ExError("image truncated".into()))?;
    Ok(b)
}
