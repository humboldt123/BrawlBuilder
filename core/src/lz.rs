//! Nintendo LZ77 types 0x10 and 0x11. Compress is a port of scripts/lz10c.c (greedy + 1-step lazy, hash chains),
//! so its output is byte-identical to the reference compressor (and to the C# BrawlBuilderEx).

use crate::bin::{require, Result};

const WIN: usize = 4096;
const MAXL: usize = 18;
const HB: u32 = 16;

pub fn is_compressed(d: &[u8]) -> bool {
    d.len() >= 4 && (d[0] == 0x10 || d[0] == 0x11)
}

/// LZ77 type 0x11 (extended lengths, used by e.g. PMEX's common2.pac)
fn decompress11(d: &[u8]) -> Result<Vec<u8>> {
    let mut size = d[1] as usize | (d[2] as usize) << 8 | (d[3] as usize) << 16;
    let mut i = 4;
    if size == 0 {
        size = d[4] as usize | (d[5] as usize) << 8 | (d[6] as usize) << 16 | (d[7] as usize) << 24;
        i = 8;
    }
    let mut o = vec![0u8; size];
    let mut op = 0;
    while op < size && i < d.len() {
        let flags = d[i];
        i += 1;
        for bit in 0..8 {
            if op >= size {
                break;
            }
            if flags & (0x80 >> bit) == 0 {
                o[op] = d[i];
                op += 1;
                i += 1;
                continue;
            }
            let b0 = d[i] as usize;
            let ind = b0 >> 4;
            let (ln, disp);
            if ind == 0 {
                let (b1, b2) = (d[i + 1] as usize, d[i + 2] as usize);
                i += 3;
                ln = (((b0 & 0xF) << 4) | (b1 >> 4)) + 0x11;
                disp = (((b1 & 0xF) << 8) | b2) + 1;
            } else if ind == 1 {
                let (b1, b2, b3) = (d[i + 1] as usize, d[i + 2] as usize, d[i + 3] as usize);
                i += 4;
                ln = (((b0 & 0xF) << 12) | (b1 << 4) | (b2 >> 4)) + 0x111;
                disp = (((b2 & 0xF) << 8) | b3) + 1;
            } else {
                let b1 = d[i + 1] as usize;
                i += 2;
                ln = ind + 1;
                disp = (((b0 & 0xF) << 8) | b1) + 1;
            }
            require(disp <= op, "corrupt LZ11 stream")?;
            for _ in 0..ln {
                if op >= size {
                    break;
                }
                o[op] = o[op - disp];
                op += 1;
            }
        }
    }
    require(op == size, "truncated LZ11 stream")?;
    Ok(o)
}

pub fn decompress(d: &[u8]) -> Result<Vec<u8>> {
    if d.len() >= 4 && d[0] == 0x11 {
        return decompress11(d);
    }
    require(d.len() >= 4 && d[0] == 0x10, "not LZ77 type 0x10/0x11")?;
    let size = d[1] as usize | (d[2] as usize) << 8 | (d[3] as usize) << 16;
    let mut o = vec![0u8; size];
    let (mut op, mut i) = (0, 4);
    while op < size && i < d.len() {
        let flags = d[i];
        i += 1;
        for bit in 0..8 {
            if op >= size {
                break;
            }
            if flags & (0x80 >> bit) != 0 {
                let (b0, b1) = (d[i] as usize, d[i + 1] as usize);
                i += 2;
                let ln = (b0 >> 4) + 3;
                let disp = (((b0 & 0xF) << 8) | b1) + 1;
                require(disp <= op, "corrupt LZ10 stream")?;
                for _ in 0..ln {
                    if op >= size {
                        break;
                    }
                    o[op] = o[op - disp];
                    op += 1;
                }
            } else {
                o[op] = d[i];
                op += 1;
                i += 1;
            }
        }
    }
    require(op == size, "truncated LZ10 stream")?;
    Ok(o)
}

fn hash(s: &[u8], p: usize) -> usize {
    (((s[p] as u32) << 8 ^ (s[p + 1] as u32) << 4 ^ s[p + 2] as u32) & ((1 << HB) - 1)) as usize
}

fn find(s: &[u8], p: usize, head: &[i32], prev: &[i32], max_len: usize) -> (usize, usize) {
    let n = s.len();
    if p + 3 > n {
        return (0, 0);
    }
    let h = hash(s, p);
    let (mut best, mut disp) = (0usize, 0usize);
    let mut c = head[h];
    let mut depth = 0;
    let maxl = (n - p).min(max_len);
    while c >= 0 && p - c as usize <= WIN && {
        depth += 1;
        depth <= 512
    } {
        let cu = c as usize;
        let mut l = 0;
        while l < maxl && s[cu + l] == s[p + l] {
            l += 1;
        }
        if l > best {
            best = l;
            disp = p - cu;
            if l == maxl {
                break;
            }
        }
        c = prev[cu];
    }
    if best >= 3 { (best, disp) } else { (0, disp) }
}

/// type 0x10, or 0x11 (`type11`: matches up to 0x10110 bytes) to keep a file in the format it came in
pub fn compress(s: &[u8], type11: bool) -> Vec<u8> {
    let n = s.len();
    let max_len = if type11 { 0x10110 } else { MAXL };
    let mut o = Vec::with_capacity(n + n / 8 + 64);
    o.push(if type11 { 0x11 } else { 0x10 });
    o.push(n as u8);
    o.push((n >> 8) as u8);
    o.push((n >> 16) as u8);
    let mut head = vec![-1i32; 1 << HB];
    let mut prev = vec![0i32; n.max(1)];
    let ins = |q: usize, head: &mut Vec<i32>, prev: &mut Vec<i32>| {
        if q + 3 <= n {
            let h = hash(s, q);
            prev[q] = head[h];
            head[h] = q as i32;
        }
    };
    let mut p = 0;
    while p < n {
        let fp = o.len();
        o.push(0);
        let mut flags = 0u8;
        let mut bit = 0;
        while bit < 8 && p < n {
            let (mut l, d) = find(s, p, &head, &prev, max_len);
            if l != 0 {
                ins(p, &mut head, &mut prev);
                let (l2, _) = find(s, p + 1, &head, &prev, max_len);
                if l2 > l + 1 {
                    l = 0;
                }
            } else {
                ins(p, &mut head, &mut prev);
            }
            if l != 0 {
                flags |= 0x80 >> bit;
                let dd = d - 1;
                if !type11 {
                    o.push((((l - 3) << 4) | (dd >> 8)) as u8);
                    o.push((dd & 255) as u8);
                } else if l <= 0x10 {
                    o.push((((l - 1) << 4) | (dd >> 8)) as u8);
                    o.push((dd & 255) as u8);
                } else if l <= 0x110 {
                    let m = l - 0x11;
                    o.push((m >> 4) as u8);
                    o.push((((m & 0xF) << 4) | (dd >> 8)) as u8);
                    o.push((dd & 255) as u8);
                } else {
                    let m = l - 0x111;
                    o.push((0x10 | (m >> 12)) as u8);
                    o.push(((m >> 4) & 255) as u8);
                    o.push((((m & 0xF) << 4) | (dd >> 8)) as u8);
                    o.push((dd & 255) as u8);
                }
                for k in 1..l {
                    ins(p + k, &mut head, &mut prev);
                }
                p += l;
            } else {
                o.push(s[p]);
                p += 1;
            }
            bit += 1;
        }
        o[fp] = flags;
    }
    while o.len() & 3 != 0 {
        o.push(0);
    }
    o
}
