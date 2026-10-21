//! The mod's SD folder as a few big files on the disc (ex0.pak, ex1.pak, ... each under 2 GiB: DVD read offsets are signed 32-bit),
//! so the FST (which lives at the top of MEM1) stays about vanilla-sized. ex0.pak starts with the index:
//!   0x00  "EXPK", count, records offset, pack count
//!   0x20  h1[count]                               sorted; this part is loaded into memory (Shims ex_init)
//!   R     {offset, size, h2, pack}[count]          R 32-aligned, same order as h1; one record is read per open
//! then file data, every file 32-aligned, in path order (files of one folder stay together on the disc).
//! Key: the path below the SD root's parent ("ex_remix/pf/menu/boot/boot.pac"), ASCII lower case, as the shims hash it.

use std::fs::{self, File};
use std::io::{self, BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use crate::bin::{put32, require, Result};
use crate::fsutil;

pub const MAX_PACK: u64 = 0x7FF00000;

/// where the packs are, where their hash table goes
#[derive(Clone, Debug)]
pub struct PackInfo {
    pub paths: Vec<String>,
    pub count: u32,
    pub entries_off: u32,
    pub index_load: u32,
}

pub fn hash(key: &str) -> (u32, u32) {
    let (mut h1, mut h2) = (0x811C9DC5u32, 5381u32);
    for ch in key.chars() {
        let c = if ch.is_ascii_uppercase() { ch as u32 + 0x20 } else { ch as u32 };
        h1 = (h1 ^ c).wrapping_mul(0x01000193);
        h2 = h2.wrapping_mul(33) ^ c;
    }
    (h1, h2)
}

pub fn index_load(count: usize) -> u32 {
    ((0x20 + 4 * count + 0x1F) & !0x1F) as u32
}

pub fn pack_name(i: usize) -> String {
    format!("ex{i}.pak")
}

struct Item {
    file: PathBuf,
    key: String,
    h1: u32,
    h2: u32,
    len: u64,
    off: u64,
    pack: usize,
}

fn plan(files: &Path, root: &str) -> Result<(Vec<Item>, usize, u32)> {
    let dir = files.join(root);
    let mut items: Vec<Item> = fsutil::files_recursive(&dir).into_iter().map(|f| {
        let key = format!("{root}/{}", fsutil::rel(&dir, &f));
        let (h1, h2) = hash(&key);
        let len = fsutil::file_len(&f);
        Item { file: f, key, h1, h2, len, off: 0, pack: 0 }
    }).collect();
    items.sort_by(|a, b| fsutil::cmp_ignore_case(&a.key, &b.key));
    let records = index_load(items.len());
    let mut off = (records as u64 + 16 * items.len() as u64 + 0x1F) & !0x1F;
    let mut p = 0;
    for it in items.iter_mut() {
        require(it.len < MAX_PACK, format!("{} is too big for a disc file", it.key))?;
        if off + it.len > MAX_PACK {
            p += 1;
            off = 0;
        }
        it.pack = p;
        it.off = off;
        off = (off + it.len + 0x1F) & !0x1F;
    }
    Ok((items, p + 1, records))
}

/// file count and pack file names the folder will need (for the FST size, before anything is written)
pub fn layout(files: &Path, root: &str) -> Result<(usize, Vec<String>)> {
    let (items, packs, _) = plan(files, root)?;
    Ok((items.len(), (0..packs).map(pack_name).collect()))
}

/// packs files/<root>/ into files/ex*.pak and deletes the folder
pub fn write(files: &Path, root: &str) -> Result<PackInfo> {
    let (items, packs, records) = plan(files, root)?;
    let mut by_hash: Vec<&Item> = items.iter().collect();
    by_hash.sort_by(|a, b| a.h1.cmp(&b.h1).then(a.h2.cmp(&b.h2)));
    for w in by_hash.windows(2) {
        require(w[1].h1 != w[0].h1 || w[1].h2 != w[0].h2, format!("hash collision: {} / {}", w[1].key, w[0].key))?;
    }
    let n = items.len() as u32;
    let mut head = vec![0u8; ((records + 16 * n + 0x1F) & !0x1F) as usize];
    head[0..4].copy_from_slice(b"EXPK");
    put32(&mut head, 4, n);
    put32(&mut head, 8, records);
    put32(&mut head, 12, packs as u32);
    for (i, it) in by_hash.iter().enumerate() {
        let r = (records + 16 * i as u32) as usize;
        put32(&mut head, 0x20 + 4 * i, it.h1);
        put32(&mut head, r, it.off as u32);
        put32(&mut head, r + 4, it.len as u32);
        put32(&mut head, r + 8, it.h2);
        put32(&mut head, r + 12, it.pack as u32);
    }
    let pad = [0u8; 0x20];
    for p in 0..packs {
        let mut o = BufWriter::with_capacity(1 << 20, File::create(files.join(pack_name(p)))?);
        if p == 0 {
            o.write_all(&head)?;
        }
        for it in items.iter().filter(|x| x.pack == p) {
            let pos = o.stream_position()?;
            require(pos == it.off, "pack layout mismatch")?;
            let mut s = File::open(&it.file)?;
            io::copy(&mut s, &mut o)?;
            let r = (o.stream_position()? & 0x1F) as usize;
            if r != 0 {
                o.write_all(&pad[..0x20 - r])?;
            }
        }
        o.flush()?;
    }
    fs::remove_dir_all(files.join(root))?;
    Ok(PackInfo { paths: (0..packs).map(|i| format!("/{}", pack_name(i))).collect(), count: n, entries_off: records, index_load: index_load(items.len()) })
}
