//! Minimal PowerPC (Broadway) encoder + two-pass assembler for the helper routines EX injects into main.dol.

use std::collections::HashMap;

fn d(op: u32, rt: i32, ra: i32, imm: i32) -> u32 {
    (op << 26) | ((rt as u32) << 21) | ((ra as u32) << 16) | (imm as u32 & 0xFFFF)
}
fn x(rt: i32, ra: i32, rb: i32, xo: u32) -> u32 {
    (31 << 26) | ((rt as u32) << 21) | ((ra as u32) << 16) | ((rb as u32) << 11) | (xo << 1)
}
pub fn addi(rt: i32, ra: i32, si: i32) -> u32 { d(14, rt, ra, si) }
pub fn li(rt: i32, si: i32) -> u32 { addi(rt, 0, si) }
pub fn addis(rt: i32, ra: i32, si: i32) -> u32 { d(15, rt, ra, si) }
pub fn lis(rt: i32, si: i32) -> u32 { addis(rt, 0, si) }
pub fn ori(ra: i32, rs: i32, ui: i32) -> u32 { d(24, rs, ra, ui) }
pub fn lwz(rt: i32, dd: i32, ra: i32) -> u32 { d(32, rt, ra, dd) }
pub fn lbz(rt: i32, dd: i32, ra: i32) -> u32 { d(34, rt, ra, dd) }
pub fn stw(rs: i32, dd: i32, ra: i32) -> u32 { d(36, rs, ra, dd) }
pub fn stb(rs: i32, dd: i32, ra: i32) -> u32 { d(38, rs, ra, dd) }
pub fn stwu(rs: i32, dd: i32, ra: i32) -> u32 { d(37, rs, ra, dd) }
pub fn lmw(rt: i32, dd: i32, ra: i32) -> u32 { d(46, rt, ra, dd) }
pub fn stmw(rs: i32, dd: i32, ra: i32) -> u32 { d(47, rs, ra, dd) }
pub fn cmpwi(ra: i32, si: i32) -> u32 { d(11, 0, ra, si) }
pub fn cmplwi(ra: i32, ui: i32) -> u32 { d(10, 0, ra, ui) }
pub fn cmpw(ra: i32, rb: i32) -> u32 { x(0, ra, rb, 0) }
pub fn cmplw(ra: i32, rb: i32) -> u32 { x(0, ra, rb, 32) }
pub fn add(rt: i32, ra: i32, rb: i32) -> u32 { x(rt, ra, rb, 266) }
/// rt = rb - ra
pub fn subf(rt: i32, ra: i32, rb: i32) -> u32 { x(rt, ra, rb, 40) }
pub fn mr(ra: i32, rs: i32) -> u32 { x(rs, ra, rs, 444) }
pub fn mullw(rt: i32, ra: i32, rb: i32) -> u32 { x(rt, ra, rb, 235) }
pub fn divwu(rt: i32, ra: i32, rb: i32) -> u32 { x(rt, ra, rb, 459) }
pub fn xor(ra: i32, rs: i32, rb: i32) -> u32 { x(rs, ra, rb, 316) }
pub fn lwzx(rt: i32, ra: i32, rb: i32) -> u32 { x(rt, ra, rb, 23) }
pub fn rlwinm(ra: i32, rs: i32, sh: i32, mb: i32, me: i32) -> u32 {
    (21 << 26) | ((rs as u32) << 21) | ((ra as u32) << 16) | ((sh as u32) << 11) | ((mb as u32) << 6) | ((me as u32) << 1)
}
pub fn mflr(rt: i32) -> u32 { (31 << 26) | ((rt as u32) << 21) | (8 << 16) | (339 << 1) }
pub fn mtlr(rs: i32) -> u32 { (31 << 26) | ((rs as u32) << 21) | (8 << 16) | (467 << 1) }
pub fn mtctr(rs: i32) -> u32 { (31 << 26) | ((rs as u32) << 21) | (9 << 16) | (467 << 1) }
pub const BLR: u32 = 0x4E800020;
pub const NOP: u32 = 0x60000000;
pub fn b(from: u32, to: u32, link: bool) -> u32 {
    0x48000000 | (to.wrapping_sub(from) & 0x03FFFFFC) | link as u32
}
pub fn bdnz(from: u32, to: u32) -> u32 { (16 << 26) | (16 << 21) | (to.wrapping_sub(from) & 0xFFFC) }
pub fn bc(cond: &str, from: u32, to: u32) -> u32 {
    let (bo, bi) = match cond {
        "lt" => (12, 0),
        "gt" => (12, 1),
        "eq" => (12, 2),
        "ge" => (4, 0),
        "le" => (4, 1),
        "ne" => (4, 2),
        _ => panic!("bad condition {cond}"),
    };
    (16 << 26) | (bo << 21) | (bi << 16) | (to.wrapping_sub(from) & 0xFFFC)
}

pub type Labels = HashMap<String, u32>;
type ItemFn = Box<dyn Fn(u32, &Labels) -> u32>;

/// Two-pass assembler: items are fixed words or functions of (pc, labels).
pub struct Asm {
    pub base: u32,
    items: Vec<ItemFn>,
    pub labels: Labels,
}

impl Asm {
    pub fn new(base: u32) -> Self { Asm { base, items: Vec::new(), labels: HashMap::new() } }
    pub fn pc(&self) -> u32 { self.base + 4 * self.items.len() as u32 }
    pub fn label(&mut self, n: &str) { let pc = self.pc(); self.labels.insert(n.to_string(), pc); }
    pub fn w(&mut self, w: u32) { self.items.push(Box::new(move |_, _| w)); }
    pub fn wf(&mut self, f: impl Fn(u32, &Labels) -> u32 + 'static) { self.items.push(Box::new(f)); }
    pub fn la(&mut self, reg: i32, addr: u32) {
        self.w(lis(reg, ((addr.wrapping_add(0x8000) >> 16) & 0xFFFF) as i32));
        self.w(addi(reg, reg, (addr & 0xFFFF) as i32));
    }
    pub fn bl_addr(&mut self, target: u32) { self.wf(move |pc, _| b(pc, target, true)); }
    pub fn bl(&mut self, label: &str) { let l = label.to_string(); self.wf(move |pc, ls| b(pc, ls[&l], true)); }
    pub fn br(&mut self, label: &str) { let l = label.to_string(); self.wf(move |pc, ls| b(pc, ls[&l], false)); }
    pub fn bc(&mut self, cond: &'static str, label: &str) { let l = label.to_string(); self.wf(move |pc, ls| bc(cond, pc, ls[&l])); }
    pub fn bdnz(&mut self, label: &str) { let l = label.to_string(); self.wf(move |pc, ls| bdnz(pc, ls[&l])); }
    pub fn bytes(&self) -> Vec<u8> {
        let mut o = vec![0u8; self.items.len() * 4];
        for (i, f) in self.items.iter().enumerate() {
            o[4 * i..4 * i + 4].copy_from_slice(&f(self.base + 4 * i as u32, &self.labels).to_be_bytes());
        }
        o
    }
}
