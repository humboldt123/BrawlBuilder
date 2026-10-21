//! Gecko code list helpers: parsing, splitting into codes, the static pre-pass (what a launcher applied before the game ran),
//! and the BrawlX-heap / vanilla-FST pointer fix-ups.

use crate::bin::{get32, put32, require, Result};
use crate::ppc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub a: u32,
    pub b: u32,
}

impl Line {
    pub fn new(a: u32, b: u32) -> Self { Line { a, b } }
}

#[derive(Clone, Debug)]
pub struct GeckoCode {
    pub name: String,
    pub lines: Vec<Line>,
}

pub const HEAP_OLD: u32 = 0x817BA5A0;
pub const HEAP_SIZE: u32 = 0x20000;
pub const VAN_ARENA_HI: u32 = 0x817DA5A0;
pub const MEM_TOP: u32 = 0x81800000;

pub fn parse(gct: &[u8]) -> Result<Vec<Line>> {
    require(gct.len() >= 16 && get32(gct, 0) == 0x00D0C0DE && get32(gct, 4) == 0x00D0C0DE, "not a GCT (missing 00D0C0DE header)")?;
    let mut l = Vec::new();
    let mut i = 8;
    while i + 8 <= gct.len() {
        let (a, b) = (get32(gct, i), get32(gct, i + 4));
        if a == 0xF0000000 && b == 0 {
            break;
        }
        l.push(Line::new(a, b));
        i += 8;
    }
    Ok(l)
}

pub fn build(codes: &[GeckoCode]) -> Vec<u8> {
    let lines: Vec<Line> = codes.iter().flat_map(|c| c.lines.iter().copied()).collect();
    let mut o = vec![0u8; 16 + 8 * lines.len()];
    put32(&mut o, 0, 0x00D0C0DE);
    put32(&mut o, 4, 0x00D0C0DE);
    for (i, l) in lines.iter().enumerate() {
        put32(&mut o, 8 + 8 * i, l.a);
        put32(&mut o, 12 + 8 * i, l.b);
    }
    let n = o.len();
    put32(&mut o, n - 8, 0xF0000000);
    o
}

/// number of lines the op starting at `i` occupies
pub fn op_length(l: &[Line], i: usize) -> usize {
    let t = (l[i].a >> 24) & 0xFE;
    let b = l[i].b;
    if t == 0x06 || t == 0x16 {
        return 1 + (b.wrapping_add(7) / 8) as usize;
    }
    if t == 0x08 {
        return 2;
    }
    if matches!(t, 0xC0 | 0xC2 | 0xD2 | 0xD0 | 0xD4 | 0xD6 | 0xF2 | 0xF4) && b < 3000 {
        return 1 + b as usize;
    }
    let a = l[i].a;
    if t == 0x66 && ((a >> 20) & 0xF) == 2 && (a & 0xFFFF) > 0 && (a & 0xFFFF) < 0x8000 {
        return 1 + (a & 0xFFFF) as usize; // unconditional goto over data
    }
    1
}

/// byte ranges the list may write in [lo, hi) with absolute (ba = 0x80000000) addresses - conservative, conditions ignored
pub fn writes_in(l: &[Line], lo: u32, hi: u32) -> Vec<(u32, u32)> {
    let mut o = Vec::new();
    let mut i = 0;
    while i < l.len() {
        let step = op_length(l, i).max(1);
        let t = (l[i].a >> 24) & 0xFE;
        let a = 0x80000000 | (l[i].a & 0x1FFFFFF);
        let n = match t {
            0x00 | 0x02 => Some(((l[i].b >> 16) + 1).wrapping_mul(4)),
            0x04 | 0xC6 | 0xC2 => Some(4),
            0x06 => Some(l[i].b),
            _ => None,
        };
        if let Some(n) = n {
            if a < hi && a.wrapping_add(n) > lo {
                o.push((a, a.wrapping_add(n)));
            }
        }
        i += step;
    }
    // ASM in the payloads addresses low memory itself: "lis rX,0x8000" + a load/store/addi/ori with a displacement in range
    // (Legacy TE's Tags Controls keeps a flag at 0x80002800), or a plain pointer word - conservative, the whole list as words
    let w: Vec<u32> = l.iter().flat_map(|x| [x.a, x.b]).collect();
    for i in 0..w.len() {
        if w[i] >= lo && w[i] < hi {
            o.push((w[i] & !3, (w[i] & !3) + 4));
        }
        if (w[i] & 0xFC1FFFFF) != 0x3C008000 {
            continue; // lis rX, 0x8000
        }
        let rx = (w[i] >> 21) & 31;
        for k in i + 1..w.len().min(i + 24) {
            let (op, rs, ra, d) = (w[k] >> 26, (w[k] >> 21) & 31, (w[k] >> 16) & 31, w[k] & 0xFFFF);
            let uses = if (32..=47).contains(&op) || op == 14 { ra == rx } else if op == 24 { rs == rx } else { false };
            if !uses || d >= 0x8000 {
                continue;
            }
            let a = 0x80000000 | d;
            if a >= lo && a < hi {
                o.push((a, a + if op == 46 || op == 47 { 0x80 } else { 0x20 }));
            }
        }
    }
    o
}

/// split a code list into pseudo-codes after each full terminator (E0): no names needed, but boundaries for the pre-pass
pub fn split_at_terminators(l: &[Line]) -> Vec<GeckoCode> {
    let mut o = Vec::new();
    let mut cur = GeckoCode { name: "code 0".into(), lines: Vec::new() };
    let mut i = 0;
    while i < l.len() {
        let n = op_length(l, i);
        for k in 0..n {
            if i + k < l.len() {
                cur.lines.push(l[i + k]);
            }
        }
        let e0 = (l[i].a >> 24) == 0xE0;
        i += n;
        if e0 {
            o.push(cur);
            cur = GeckoCode { name: format!("code {}", o.len()), lines: Vec::new() };
        }
    }
    if !cur.lines.is_empty() {
        o.push(cur);
    }
    o
}

fn is_hex(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_hexdigit())
}

/// try to name codes from a GCTRealMate codeset.txt ("name" line, then "* XXXXXXXX XXXXXXXX" lines); None if it does not match
pub fn from_codeset(text: &str, gct: &[Line]) -> Option<Vec<GeckoCode>> {
    let mut codes: Vec<GeckoCode> = Vec::new();
    let mut have = false;
    for raw in text.replace('\r', "").split('\n') {
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        let p: Vec<&str> = s.trim_start_matches('*').trim().split([' ', '\t']).filter(|x| !x.is_empty()).collect();
        if have && p.len() == 2 && p[0].len() == 8 && p[1].len() == 8 && is_hex(p[0]) && is_hex(p[1]) {
            let line = Line::new(u32::from_str_radix(p[0], 16).ok()?, u32::from_str_radix(p[1], 16).ok()?);
            codes.last_mut().unwrap().lines.push(line);
            continue;
        }
        if !have && s == "RSBE01" {
            continue;
        }
        if s.starts_with('*') {
            continue;
        }
        codes.push(GeckoCode { name: s.to_string(), lines: Vec::new() });
        have = true;
    }
    let codes: Vec<GeckoCode> = codes.into_iter().filter(|c| !c.lines.is_empty()).collect();
    let flat: Vec<Line> = codes.iter().flat_map(|c| c.lines.iter().copied()).collect();
    if flat.len() != gct.len() || flat.iter().zip(gct).any(|(a, b)| a != b) {
        return None;
    }
    Some(codes)
}

/// ASM blocks (C2/D2 hooks with their address, C0 without) as (header index, address or 0, payload lines)
pub fn asm_blocks(l: &[Line]) -> Vec<(usize, u32, usize)> {
    let mut o = Vec::new();
    let mut i = 0;
    while i < l.len() {
        let t = (l[i].a >> 24) & 0xFE;
        let b = l[i].b;
        if (t == 0xC2 || t == 0xD2) && b > 0 && b < 3000 {
            o.push((i, 0x80000000 | (l[i].a & 0x1FFFFFF), b as usize));
            i += 1 + b as usize;
        } else if t == 0xC0 && b > 0 && b < 3000 {
            o.push((i, 0, b as usize));
            i += 1 + b as usize;
        } else {
            i += op_length(l, i);
        }
    }
    o
}

/// The launcher's pre-pass: plain RAM writes (00/02/04/06) and C2 hooks that are not under a condition, a pointer/register op or a
/// flow op of their code. A full terminator (E0) starts a new code (resets the state).
pub fn static_ops(l: &[Line], writes: &mut Vec<(u32, Vec<u8>)>, hooks: &mut Vec<(usize, u32, usize)>) {
    let mut depth: i64 = 0;
    let mut tainted = false;
    let mut i = 0;
    while i < l.len() {
        let step = op_length(l, i);
        let (a, b) = (l[i].a, l[i].b);
        let t = (a >> 24) & 0xFE;
        let full = 0x80000000 | (a & 0x1FFFFFF);
        let next = i + step;
        if (0x20..=0x3E).contains(&t) {
            depth += 1;
            i = next;
            continue;
        }
        if t == 0xE0 {
            depth = 0;
            tainted = false;
            i = next;
            continue;
        }
        if t == 0xE2 {
            // E2T000VV: close VV ifs (T=1: else)
            if ((a >> 20) & 0xF) != 0 {
                tainted = true;
            }
            depth = (depth - (a & 0xFF).max(1) as i64).max(0);
            i = next;
            continue;
        }
        if t == 0xF0 {
            break;
        }
        let plain = matches!(t, 0x00 | 0x02 | 0x04 | 0x06 | 0xC0 | 0xC2 | 0xD2 | 0xD0 | 0xD4 | 0xD6);
        // po writes, serial, Gecko registers, po/store ops, repeat: ba-relative writes unaffected
        let neutral = (0x10..=0x16).contains(&t) || t == 0x08 || (0x80..=0xAE).contains(&t) || matches!(t, 0x44 | 0x48 | 0x4A | 0x4C | 0x4E | 0x60 | 0x62);
        if neutral {
            i = next;
            continue;
        }
        if !plain {
            // base-address, flow, switch, checksum and unknown ops
            tainted = true;
            i = next;
            continue;
        }
        if depth > 0 || tainted {
            i = next;
            continue;
        }
        match t {
            0x00 => {
                for k in 0..=(b >> 16) {
                    writes.push((full + k, vec![b as u8]));
                }
            }
            0x02 => {
                for k in 0..=(b >> 16) {
                    writes.push((full + 2 * k, vec![(b >> 8) as u8, b as u8]));
                }
            }
            0x04 => writes.push((full, b.to_be_bytes().to_vec())),
            0x06 => {
                let mut d = vec![0u8; b as usize];
                for (k, byte) in d.iter_mut().enumerate() {
                    let Some(ln) = l.get(i + 1 + k / 8) else { break };
                    let w = if k % 8 < 4 { ln.a } else { ln.b };
                    *byte = (w >> (24 - 8 * (k % 4))) as u8;
                }
                writes.push((full, d));
            }
            0xC2 => {
                if b > 0 && b < 3000 {
                    hooks.push((i, full, b as usize));
                }
            }
            _ => {}
        }
        i = next;
    }
}

const D_FORM: [u32; 25] = [14, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55];
const STORE_LIKE: [u32; 11] = [36, 37, 38, 39, 44, 45, 47, 52, 53, 54, 55];

/// (index, ori, low half) of the instruction completing a lis into `reg`
fn find_pair(words: &[u32], reg: u32) -> Option<(usize, bool, u32)> {
    for (r, &u) in words.iter().enumerate() {
        let (op, ra, rs) = (u >> 26, (u >> 16) & 0x1F, (u >> 21) & 0x1F);
        if op == 24 && rs == reg {
            return Some((r, true, u & 0xFFFF));
        }
        if D_FORM.contains(&op) && ra == reg {
            return Some((r, false, u & 0xFFFF));
        }
        if rs == reg && !STORE_LIKE.contains(&op) {
            return None;
        }
    }
    None
}

/// Move every hard-coded address into the vanilla BrawlX heap by `delta` and point every hard-coded address into the
/// vanilla FST name table at our own copy of the string. Returns notes (empty: nothing found).
pub fn fix_heap_and_fst(
    c: &mut GeckoCode,
    delta: i32,
    fst_name: &mut dyn FnMut(u32) -> Option<Vec<u8>>,
    string_addr: &mut dyn FnMut(&[u8]) -> u32,
    dol_word: &dyn Fn(u32) -> u32,
) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    let l = &mut c.lines;
    // flatten ASM payload words with their position: (line, slot, hook address, header)
    let mut wv: Vec<(usize, usize, u32, usize)> = Vec::new();
    for (hdr, addr, n) in asm_blocks(l) {
        for j in 0..n {
            for s in 0..2 {
                wv.push((hdr + 1 + j, s, addr, hdr));
            }
        }
    }
    let word = |l: &Vec<Line>, q: usize| -> u32 {
        let (li, s, _, _) = wv[q];
        if s == 0 { l[li].a } else { l[li].b }
    };
    let set = |l: &mut Vec<Line>, q: usize, v: u32| {
        let (li, s, _, _) = wv[q];
        if s == 0 { l[li].a = v } else { l[li].b = v }
    };
    for q in 0..wv.len() {
        let w = word(l, q);
        if w >> 26 != 15 || ((w >> 16) & 0x1F) != 0 {
            continue; // lis rX, hi
        }
        let hi = w & 0xFFFF;
        if !(0x817B..=0x817F).contains(&hi) {
            continue;
        }
        let reg = (w >> 21) & 0x1F;
        let mut rest = Vec::new();
        let mut k = q + 1;
        while k < wv.len() && rest.len() < 7 && wv[k].3 == wv[q].3 {
            rest.push(k);
            k += 1;
        }
        let rest_words: Vec<u32> = rest.iter().map(|&k| word(l, k)).collect();
        let mut f = find_pair(&rest_words, reg);
        let mut in_dol = false;
        if f.is_none() && wv[q].2 != 0 && rest_words.iter().all(|&x| x == 0 || x == ppc::NOP) {
            let dw: Vec<u32> = (1..=7u32).map(|k| dol_word(wv[q].2.wrapping_add(4 * k))).collect();
            f = find_pair(&dw, reg);
            in_dol = true;
        }
        let Some((fi, ori, lo)) = f else {
            notes.push(format!("lis {hi:x} without a pair (left)"));
            continue;
        };
        let addr = (hi << 16).wrapping_add(if ori { lo } else { lo as u16 as i16 as i32 as u32 });
        let neu = if addr >= HEAP_OLD && addr < HEAP_OLD + HEAP_SIZE {
            addr.wrapping_add(delta as u32)
        } else if addr >= VAN_ARENA_HI && addr < MEM_TOP {
            match fst_name(addr) {
                Some(s) => string_addr(&s),
                None => {
                    notes.push(format!("FST pointer {addr:x} not on a name (left)"));
                    continue;
                }
            }
        } else {
            continue;
        };
        let nh = if ori { neu >> 16 } else { (neu.wrapping_add(0x8000) >> 16) & 0xFFFF };
        let nl = neu & 0xFFFF;
        if neu == addr {
            continue;
        }
        if in_dol {
            require(nl == lo, format!("a lis completed in main.dol needs a different low half ({addr:08X})"))?;
            set(l, q, (w & 0xFFFF0000) | nh);
            notes.push(format!("{addr:x} -> {neu:x} (pair in main.dol)"));
            continue;
        }
        let r = rest[fi];
        set(l, q, (w & 0xFFFF0000) | nh);
        let wr = word(l, r);
        set(l, r, (wr & 0xFFFF0000) | nl);
        notes.push(format!("{addr:x} -> {neu:x}"));
    }
    if delta == 0 {
        return Ok(notes);
    }
    // 32-byte-aligned data words inside ASM payloads that hold a heap address (e.g. Syringe's 'word 0x817ba5a0')
    for q in 0..wv.len() {
        let w = word(l, q);
        if w >= HEAP_OLD && w <= VAN_ARENA_HI && (w & 0x1F) == 0 {
            set(l, q, w.wrapping_add(delta as u32));
            notes.push(format!("data word {w:x} moved"));
        }
    }
    // plain writes into the heap, and the prefab heap header blob (pointers inside it move too)
    let mut i = 0;
    while i < l.len() {
        let step = op_length(l, i);
        let (a, b) = (l[i].a, l[i].b);
        let t = (a >> 24) & 0xFE;
        let ad = 0x80000000 | (a & 0x1FFFFFF);
        if ad < HEAP_OLD || ad >= HEAP_OLD + HEAP_SIZE || (a & 0x10000000) != 0 {
            i += step;
            continue;
        }
        if t == 0 || t == 2 || t == 4 {
            l[i] = Line::new((a & 0xFE000000) | (ad.wrapping_add(delta as u32) & 0x01FFFFFF), b);
            notes.push(format!("write {ad:x} moved"));
        } else if t == 6 {
            let mut k = 0u32;
            while k + 4 <= b {
                let li = i + 1 + (k / 8) as usize;
                let hiw = k % 8 == 0;
                if li < l.len() {
                    let v = if hiw { l[li].a } else { l[li].b };
                    if v >= HEAP_OLD && v <= HEAP_OLD + HEAP_SIZE {
                        if hiw { l[li].a = v.wrapping_add(delta as u32) } else { l[li].b = v.wrapping_add(delta as u32) }
                    }
                }
                k += 4;
            }
            l[i] = Line::new((a & 0xFE000000) | (ad.wrapping_add(delta as u32) & 0x01FFFFFF), b);
            notes.push(format!("heap blob {ad:x} moved"));
        }
        i += step;
    }
    Ok(notes)
}
