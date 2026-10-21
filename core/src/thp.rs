//! Port of scripts/thp_truncate.py: keep the first K frames of a THP movie as a valid, self-consistent THP.

use crate::bin::{get32, put32, require, Result};

struct Hdr {
    n: u32,
    first_sz: u32,
    data_sz: u32,
    offs_off: u32,
    first_off: u32,
    last_off: u32,
}

fn parse(d: &[u8]) -> Result<Hdr> {
    require(d.len() >= 0x30 && &d[0..4] == b"THP\0", "not a THP file")?;
    Ok(Hdr {
        n: get32(d, 0x14),
        first_sz: get32(d, 0x18),
        data_sz: get32(d, 0x1C),
        offs_off: get32(d, 0x24),
        first_off: get32(d, 0x28),
        last_off: get32(d, 0x2C),
    })
}

fn frames(d: &[u8], h: &Hdr) -> Result<Vec<(u64, u64)>> {
    let mut r = Vec::new();
    let (mut off, mut sz) = (h.first_off as u64, h.first_sz as u64);
    for _ in 0..h.n {
        r.push((off, sz));
        require((off as usize) + 4 <= d.len(), "THP frame offset out of range")?;
        let nxt = get32(d, off as usize) as u64;
        off += sz;
        sz = nxt;
    }
    Ok(r)
}

fn check(d: &[u8]) -> Result<(bool, Hdr, Vec<(u64, u64)>)> {
    let h = parse(d)?;
    let fr = frames(d, &h)?;
    let ok = fr.last().map(|f| f.0) == Some(h.last_off as u64) && h.data_sz as u64 == d.len() as u64 - h.first_off as u64 && h.offs_off == 0;
    Ok((ok, h, fr))
}

pub fn truncate(d: &[u8], k: usize) -> Result<Vec<u8>> {
    let (ok, h, fr) = check(d)?;
    require(ok, "input THP failed consistency check")?;
    let k = k.min(h.n as usize);
    let start = h.first_off as u64;
    let end = fr[k - 1].0 + fr[k - 1].1;
    let mut out = d[..end as usize].to_vec();
    let last_rel = fr[k - 1].0 - start;
    put32(&mut out, (start + last_rel) as usize, h.first_sz); // last.next = firstFrameSize
    put32(&mut out, start as usize + 4, fr[k - 1].1 as u32); // first.prev = size of last frame
    put32(&mut out, 0x14, k as u32);
    put32(&mut out, 0x1C, (end - start) as u32);
    put32(&mut out, 0x2C, (start + last_rel) as u32);
    let (ok2, h2, _) = check(&out)?;
    require(ok2 && h2.n as usize == k, "output THP failed consistency check")?;
    Ok(out)
}
