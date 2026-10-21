//! Code EX adds to main.dol (text right after the Gecko handler):
//!  * "SD card on the disc": replacements for Brawl's FA (SD/FAT) file API that read the mod's SD folder from pack files on the DVD
//!    (see `sdpack`). Every SD reader of the Project+ family (File Patch Code, sawnd/brstm streamers, readSDFile/checkFileSD
//!    users, Syringe, BrawlEx, THP replacement, stage/music loaders) ends up in FAFopen/FAFread/FAFseek/FAFclose/FAFstat, so the mod runs
//!    unmodified. Paths resolve through a hash table loaded on first use - a few bytes per file instead of the ~32 an FST entry costs.
//!  * a CRT .bss clear that leaves the code list and the baked data alone (the launcher instead skipped the clear entirely), and that
//!    places the EX RAM block before OSInit reads the arena bounds.

use std::collections::HashMap;

use crate::bin::{get32, require, Result};
use crate::ppc::{self, Asm};
use crate::sdpack::PackInfo;

pub const DVD_OPEN: u32 = 0x801F6278;
pub const DVD_READ_PRIO: u32 = 0x801F67F0;
pub const OS_LOCK_MUTEX: u32 = 0x801DEBB4;
pub const OS_UNLOCK_MUTEX: u32 = 0x801DEC90;
pub const MEMSET: u32 = 0x8000443C;

pub const FA_ENTRIES: [(&str, u32, u32); 10] = [
    ("open", 0x803EBEB8, 0x9421FFF0), ("read", 0x803EBEE4, 0x4BFFF7FC), ("seek", 0x803EBEE8, 0x9421FFF0),
    ("close", 0x803EBE8C, 0x9421FFF0), ("stat", 0x803EBF6C, 0x9421FFF0),
    // the prfile2 functions under them (same arguments; FAF* only normalize the result) - P+Ex's THP streamer calls pf2_fread directly
    ("open", 0x803EB644, 0x9421FFD0), ("read", 0x803EB6E0, 0x9421FFF0), ("seek", 0x803EB708, 0x9421FFF0),
    ("close", 0x803EB620, 0x9421FFF0), ("stat", 0x803EB7C4, 0x9421FFE0),
];

// file slot: +0 used, +4 position, +8 -> fake FA file object whose +0x264 is the size (RSBE reads the size that way), +0xC size,
// +0x10 offset in its pack, +0x14 -> the pack's DVDFileInfo
// RAM block (zeroed by the early hook): bounce buffer, mutex, init flag, one DVDFileInfo per pack, file slots
const NSLOT: i32 = 16;
const SLOT: i32 = 0x20;
const USED: i32 = 0;
const POS: i32 = 4;
const OBJ: i32 = 8;
const SIZE: i32 = 0xC;
const BASE: i32 = 0x10;
const FIP_SLOT: i32 = 0x14;
const BOUNCE: i32 = 0x400;
const BB: u32 = 0;
const MTX: u32 = BOUNCE as u32;
const INIT: u32 = MTX + 0x20;
const FIP: u32 = INIT + 0x20;

pub fn data_size(packs: usize) -> u32 {
    FIP + packs as u32 * 0x40 + (NSLOT * SLOT) as u32
}

fn prolog(a: &mut Asm, n: i32) {
    a.w(ppc::stwu(1, -0x40, 1));
    a.w(ppc::mflr(0));
    a.w(ppc::stw(0, 0x44, 1));
    a.w(ppc::stmw(32 - n, 0x08, 1));
}

fn epilog(a: &mut Asm, n: i32) {
    a.w(ppc::lmw(32 - n, 0x08, 1));
    a.w(ppc::lwz(0, 0x44, 1));
    a.w(ppc::mtlr(0));
    a.w(ppc::addi(1, 1, 0x40));
    a.w(ppc::BLR);
}

/// dst = src past "xx:" if a ':' occurs in the first 6 chars
fn strip(a: &mut Asm, dst: i32, src: i32, tag: &str) {
    a.w(ppc::mr(dst, src));
    a.w(ppc::li(0, 6));
    a.w(ppc::mtctr(0));
    a.w(ppc::mr(12, src));
    a.label(&format!("{tag}_l"));
    a.w(ppc::lbz(0, 0, 12));
    a.w(ppc::cmpwi(0, 0));
    a.bc("eq", &format!("{tag}_d"));
    a.w(ppc::cmpwi(0, 0x3A));
    a.bc("ne", &format!("{tag}_n"));
    a.w(ppc::addi(dst, 12, 1));
    a.br(&format!("{tag}_d"));
    a.label(&format!("{tag}_n"));
    a.w(ppc::addi(12, 12, 1));
    a.bdnz(&format!("{tag}_l"));
    a.label(&format!("{tag}_d"));
}

fn hi_adj(v: u32) -> i32 {
    ((v.wrapping_add(0x8000) >> 16) & 0xFFFF) as i32
}

fn lal(a: &mut Asm, reg: i32, label: &'static str, add: u32) {
    a.wf(move |_, l| ppc::lis(reg, hi_adj(l[label].wrapping_add(add))));
    a.wf(move |_, l| ppc::addi(reg, reg, (l[label].wrapping_add(add) & 0xFFFF) as i32));
}

/// reg = RAM block + off (the block's address is decided at boot and kept at label "ram")
fn lad(a: &mut Asm, reg: i32, off: u32) {
    a.wf(move |_, l| ppc::lis(reg, hi_adj(l["ram"])));
    a.wf(move |_, l| ppc::lwz(reg, (l["ram"] & 0xFFFF) as u16 as i16 as i32, reg));
    if off != 0 {
        a.w(ppc::addi(reg, reg, off as i32));
    }
}

fn sd_disc(a: &mut Asm, pk: &PackInfo) {
    let t = FIP + 0x40 * pk.paths.len() as u32;
    let idx = data_size(pk.paths.len());
    let hashes = idx + 0x20;

    // ex_init (mutex held): open the packs once and load the hash table from ex0.pak; r3 = 1 ok / 0 failed
    a.label("ex_init");
    prolog(a, 1);
    lad(a, 12, INIT);
    a.w(ppc::lwz(0, 0, 12));
    a.w(ppc::cmpwi(0, 0));
    a.bc("ne", "in_ok");
    for i in 0..pk.paths.len() {
        lal(a, 3, "paths", 0x10 * i as u32);
        lad(a, 4, FIP + 0x40 * i as u32);
        a.bl_addr(DVD_OPEN);
        a.w(ppc::cmpwi(3, 0));
        a.bc("eq", "in_fail");
    }
    lad(a, 3, FIP);
    lad(a, 4, idx);
    a.la(5, pk.index_load);
    a.w(ppc::li(6, 0));
    a.w(ppc::li(7, 2));
    a.bl_addr(DVD_READ_PRIO);
    a.w(ppc::cmpwi(3, 0));
    a.bc("lt", "in_fail");
    lad(a, 12, INIT);
    a.w(ppc::li(0, 1));
    a.w(ppc::stw(0, 0, 12));
    a.label("in_ok");
    a.w(ppc::li(3, 1));
    a.br("in_ret");
    a.label("in_fail");
    a.w(ppc::li(3, 0));
    a.label("in_ret");
    epilog(a, 1);

    // ex_find(r3 = path, mutex held): r3 = 1 and r4 = offset, r5 = size, r6 = pack DVDFileInfo if the file exists, else r3 = 0
    // key: path without leading '/', ASCII lower case; h1 = FNV-1a, h2 = djb2-xor (checked against the entry read from the disc)
    a.label("ex_find");
    prolog(a, 8);
    a.w(ppc::mr(31, 3));
    a.label("fd_sl");
    a.w(ppc::lbz(0, 0, 31));
    a.w(ppc::cmpwi(0, 0x2F));
    a.bc("ne", "fd_h");
    a.w(ppc::addi(31, 31, 1));
    a.br("fd_sl");
    a.label("fd_h");
    a.la(30, 0x811C9DC5);
    a.w(ppc::li(29, 5381));
    a.la(28, 0x01000193);
    a.w(ppc::li(27, 33));
    a.w(ppc::li(9, 1));
    a.label("fd_hl");
    a.w(ppc::lbz(10, 0, 31));
    a.w(ppc::cmpwi(10, 0));
    a.bc("eq", "fd_bs");
    // normalize like DVDOpen: at a segment start skip extra '/' and "./" ("/P+Ex/././pf/..." in Legacy's codes)
    a.w(ppc::cmpwi(9, 0));
    a.bc("eq", "fd_ch");
    a.w(ppc::cmpwi(10, 0x2F));
    a.bc("eq", "fd_sk1");
    a.w(ppc::cmpwi(10, 0x2E));
    a.bc("ne", "fd_ch");
    a.w(ppc::lbz(0, 1, 31));
    a.w(ppc::cmpwi(0, 0x2F));
    a.bc("ne", "fd_ch");
    a.w(ppc::addi(31, 31, 1));
    a.label("fd_sk1");
    a.w(ppc::addi(31, 31, 1));
    a.br("fd_hl");
    a.label("fd_ch");
    a.w(ppc::li(9, 0));
    a.w(ppc::cmpwi(10, 0x2F));
    a.bc("ne", "fd_cs");
    a.w(ppc::li(9, 1));
    a.label("fd_cs");
    a.w(ppc::addi(12, 10, -0x41));
    a.w(ppc::cmplwi(12, 25));
    a.bc("gt", "fd_lc");
    a.w(ppc::addi(10, 10, 0x20));
    a.label("fd_lc");
    a.w(ppc::xor(30, 30, 10));
    a.w(ppc::mullw(30, 30, 28));
    a.w(ppc::mullw(29, 29, 27));
    a.w(ppc::xor(29, 29, 10));
    a.w(ppc::addi(31, 31, 1));
    a.br("fd_hl");
    // lower bound of h1 in the sorted table: r26 lo, r25 hi, r24 table
    a.label("fd_bs");
    a.w(ppc::li(26, 0));
    a.la(25, pk.count);
    lad(a, 24, hashes);
    a.label("fd_bl");
    a.w(ppc::cmplw(26, 25));
    a.bc("ge", "fd_scan");
    a.w(ppc::add(12, 26, 25));
    a.w(ppc::rlwinm(12, 12, 31, 1, 31));
    a.w(ppc::rlwinm(11, 12, 2, 0, 29));
    a.w(ppc::lwzx(0, 24, 11));
    a.w(ppc::cmplw(0, 30));
    a.bc("lt", "fd_up");
    a.w(ppc::mr(25, 12));
    a.br("fd_bl");
    a.label("fd_up");
    a.w(ppc::addi(26, 12, 1));
    a.br("fd_bl");
    // every entry with this h1: read its 16-byte record (offset, size, h2) and compare h2
    a.label("fd_scan");
    a.la(9, pk.count);
    a.w(ppc::cmplw(26, 9));
    a.bc("ge", "fd_no");
    a.w(ppc::rlwinm(11, 26, 2, 0, 29));
    a.w(ppc::lwzx(0, 24, 11));
    a.w(ppc::cmplw(0, 30));
    a.bc("ne", "fd_no");
    a.w(ppc::rlwinm(25, 26, 4, 0, 27));
    a.la(9, pk.entries_off);
    a.w(ppc::add(25, 25, 9)); // r25 = record offset in the pack
    lad(a, 3, FIP);
    lad(a, 4, BB);
    a.w(ppc::li(5, 0x20));
    a.w(ppc::rlwinm(6, 25, 0, 0, 26));
    a.w(ppc::li(7, 2));
    a.bl_addr(DVD_READ_PRIO);
    a.w(ppc::cmpwi(3, 0));
    a.bc("lt", "fd_no");
    lad(a, 12, BB);
    a.w(ppc::rlwinm(0, 25, 0, 27, 31));
    a.w(ppc::add(12, 12, 0));
    a.w(ppc::lwz(0, 8, 12));
    a.w(ppc::cmplw(0, 29));
    a.bc("eq", "fd_hit");
    a.w(ppc::addi(26, 26, 1));
    a.br("fd_scan");
    a.label("fd_hit");
    a.w(ppc::lwz(4, 0, 12));
    a.w(ppc::lwz(5, 4, 12));
    a.w(ppc::lwz(6, 12, 12));
    a.w(ppc::rlwinm(6, 6, 6, 0, 25));
    lad(a, 7, FIP);
    a.w(ppc::add(6, 6, 7));
    a.w(ppc::li(3, 1));
    a.br("fd_ret");
    a.label("fd_no");
    a.w(ppc::li(3, 0));
    a.label("fd_ret");
    epilog(a, 8);

    // FILE* fopen(path, mode)
    a.label("fa_open");
    prolog(a, 5);
    strip(a, 31, 3, "op");
    lad(a, 3, MTX);
    a.bl_addr(OS_LOCK_MUTEX);
    a.bl("ex_init");
    a.w(ppc::cmpwi(3, 0));
    a.bc("eq", "op_fail");
    a.w(ppc::mr(3, 31));
    a.bl("ex_find");
    a.w(ppc::cmpwi(3, 0));
    a.bc("eq", "op_fail");
    a.w(ppc::mr(29, 4));
    a.w(ppc::mr(28, 5));
    a.w(ppc::mr(27, 6));
    lad(a, 30, t);
    a.w(ppc::li(0, NSLOT));
    a.w(ppc::mtctr(0));
    a.label("op_find");
    a.w(ppc::lwz(0, USED, 30));
    a.w(ppc::cmpwi(0, 0));
    a.bc("eq", "op_got");
    a.w(ppc::addi(30, 30, SLOT));
    a.bdnz("op_find");
    a.br("op_fail");
    a.label("op_got");
    a.w(ppc::li(0, 1));
    a.w(ppc::stw(0, USED, 30));
    a.w(ppc::li(0, 0));
    a.w(ppc::stw(0, POS, 30));
    a.w(ppc::stw(29, BASE, 30));
    a.w(ppc::stw(28, SIZE, 30));
    a.w(ppc::stw(27, FIP_SLOT, 30));
    a.w(ppc::addi(0, 30, SIZE - 0x264));
    a.w(ppc::stw(0, OBJ, 30));
    a.w(ppc::mr(31, 30));
    a.br("op_ret");
    a.label("op_fail");
    a.w(ppc::li(31, 0));
    a.label("op_ret");
    lad(a, 3, MTX);
    a.bl_addr(OS_UNLOCK_MUTEX);
    a.w(ppc::mr(3, 31));
    epilog(a, 5);
    // int fclose(FILE*)
    a.label("fa_close");
    a.w(ppc::cmpwi(3, 0));
    a.bc("eq", "cl_ret");
    a.w(ppc::li(0, 0));
    a.w(ppc::stw(0, USED, 3));
    a.label("cl_ret");
    a.w(ppc::li(3, 0));
    a.w(ppc::BLR);
    // int fseek(FILE*, long off, int whence)
    a.label("fa_seek");
    a.w(ppc::cmpwi(5, 1));
    a.bc("eq", "sk_cur");
    a.w(ppc::cmpwi(5, 2));
    a.bc("eq", "sk_end");
    a.w(ppc::stw(4, POS, 3));
    a.w(ppc::li(3, 0));
    a.w(ppc::BLR);
    a.label("sk_cur");
    a.w(ppc::lwz(0, POS, 3));
    a.w(ppc::add(0, 0, 4));
    a.w(ppc::stw(0, POS, 3));
    a.w(ppc::li(3, 0));
    a.w(ppc::BLR);
    a.label("sk_end");
    a.w(ppc::lwz(0, SIZE, 3));
    a.w(ppc::add(0, 0, 4));
    a.w(ppc::stw(0, POS, 3));
    a.w(ppc::li(3, 0));
    a.w(ppc::BLR);
    // size_t fread(buf, size, n, FILE*): r31 f, r30 buf, r29 size, r28 remaining, r27 done, r26 pos, r25 aligned off, r24 skip, r23 chunk, r22 base, r21 pack
    a.label("fa_read");
    prolog(a, 11);
    a.w(ppc::mr(31, 6));
    a.w(ppc::mr(30, 3));
    a.w(ppc::mr(29, 4));
    a.w(ppc::mullw(28, 4, 5));
    a.w(ppc::li(27, 0));
    a.w(ppc::cmpwi(29, 0));
    a.bc("eq", "rd_zero");
    lad(a, 3, MTX);
    a.bl_addr(OS_LOCK_MUTEX);
    a.w(ppc::lwz(22, BASE, 31));
    a.w(ppc::lwz(21, FIP_SLOT, 31));
    a.w(ppc::lwz(26, POS, 31));
    a.w(ppc::lwz(0, SIZE, 31));
    a.w(ppc::cmpwi(26, 0));
    a.bc("lt", "rd_done");
    a.w(ppc::cmpw(26, 0));
    a.bc("ge", "rd_done");
    a.w(ppc::subf(0, 26, 0));
    a.w(ppc::cmplw(28, 0));
    a.bc("le", "rd_loop");
    a.w(ppc::mr(28, 0));
    a.label("rd_loop");
    a.w(ppc::cmpwi(28, 0));
    a.bc("le", "rd_done");
    // fast path: destination 32-aligned, position 4-aligned (pack offsets are 32-aligned) -> read (remaining & ~31) straight into the caller's buffer
    a.w(ppc::add(4, 30, 27));
    a.w(ppc::rlwinm(0, 4, 0, 27, 31));
    a.w(ppc::cmpwi(0, 0));
    a.bc("ne", "rd_slow");
    a.w(ppc::rlwinm(0, 26, 0, 30, 31));
    a.w(ppc::cmpwi(0, 0));
    a.bc("ne", "rd_slow");
    a.w(ppc::rlwinm(23, 28, 0, 0, 26));
    a.w(ppc::cmpwi(23, 0));
    a.bc("eq", "rd_slow");
    a.w(ppc::mr(3, 21));
    a.w(ppc::mr(5, 23));
    a.w(ppc::add(6, 26, 22));
    a.w(ppc::li(7, 2));
    a.bl_addr(DVD_READ_PRIO);
    a.w(ppc::cmpwi(3, 0));
    a.bc("lt", "rd_done");
    a.w(ppc::add(26, 26, 23));
    a.w(ppc::add(27, 27, 23));
    a.w(ppc::subf(28, 23, 28));
    a.br("rd_loop");
    a.label("rd_slow");
    a.w(ppc::add(0, 26, 22));
    a.w(ppc::rlwinm(25, 0, 0, 0, 26));
    a.w(ppc::subf(24, 25, 0));
    a.w(ppc::li(23, BOUNCE));
    a.w(ppc::subf(23, 24, 23));
    a.w(ppc::cmplw(23, 28));
    a.bc("le", "rd_c");
    a.w(ppc::mr(23, 28));
    a.label("rd_c");
    a.w(ppc::add(5, 24, 23));
    a.w(ppc::addi(5, 5, 31));
    a.w(ppc::rlwinm(5, 5, 0, 0, 26));
    a.w(ppc::mr(3, 21));
    lad(a, 4, BB);
    a.w(ppc::mr(6, 25));
    a.w(ppc::li(7, 2));
    a.bl_addr(DVD_READ_PRIO);
    a.w(ppc::cmpwi(3, 0));
    a.bc("lt", "rd_done");
    lad(a, 4, BB);
    a.w(ppc::add(4, 4, 24));
    a.w(ppc::add(5, 30, 27));
    a.w(ppc::mtctr(23));
    a.label("rd_cp");
    a.w(ppc::lbz(0, 0, 4));
    a.w(ppc::stb(0, 0, 5));
    a.w(ppc::addi(4, 4, 1));
    a.w(ppc::addi(5, 5, 1));
    a.bdnz("rd_cp");
    a.w(ppc::add(26, 26, 23));
    a.w(ppc::add(27, 27, 23));
    a.w(ppc::subf(28, 23, 28));
    a.br("rd_loop");
    a.label("rd_done");
    a.w(ppc::stw(26, POS, 31));
    lad(a, 3, MTX);
    a.bl_addr(OS_UNLOCK_MUTEX);
    a.w(ppc::divwu(3, 27, 29));
    a.br("rd_ret");
    a.label("rd_zero");
    a.w(ppc::li(3, 0));
    a.label("rd_ret");
    epilog(a, 11);
    // int fstat(path, struct stat*): st[0] = size
    a.label("fa_stat");
    prolog(a, 3);
    a.w(ppc::mr(30, 4));
    a.w(ppc::mr(29, 3));
    lad(a, 3, MTX);
    a.bl_addr(OS_LOCK_MUTEX);
    a.w(ppc::li(31, -1));
    a.bl("ex_init");
    a.w(ppc::cmpwi(3, 0));
    a.bc("eq", "st_ret");
    strip(a, 3, 29, "st");
    a.bl("ex_find");
    a.w(ppc::cmpwi(3, 0));
    a.bc("eq", "st_ret");
    a.w(ppc::stw(5, 0, 30));
    a.w(ppc::li(31, 0));
    a.label("st_ret");
    lad(a, 3, MTX);
    a.bl_addr(OS_UNLOCK_MUTEX);
    a.w(ppc::mr(3, 31));
    epilog(a, 3);
}

/// runs from __init_data (once per .bss section), before OSInit. On the first call the EX RAM block + hash table are placed:
///  mem2: <below> bytes under the MEM2 arena top (0x80003128, lowered to the block); the MEM1 arena top is set to arena_hi if given
///  else: under the FST the apploader placed (0x80000038) and its BI2 block (0x800000F4); the MEM1 arena top OSInit uses
///        (0x80003110, else BI2 0x80000034) becomes min(arena_hi, block) (arena_hi 0: the block itself)
/// bss_clear(r3 = start, r4 = 0, r5 = size): memset every gap of [start, start+size) not covered by the protect table
fn bss_clear(a: &mut Asm, arena_hi: u32, below: u32, data_size: u32, mem2: bool) {
    a.label("bss");
    a.w(ppc::stwu(1, -0x20, 1));
    a.w(ppc::mflr(0));
    a.w(ppc::stw(0, 0x24, 1));
    a.w(ppc::stmw(27, 0x0C, 1));
    a.w(ppc::mr(31, 3));
    a.w(ppc::add(30, 3, 5));
    a.wf(|_, l| ppc::lis(10, hi_adj(l["ram"])));
    a.wf(|_, l| ppc::lwz(0, (l["ram"] & 0xFFFF) as u16 as i16 as i32, 10));
    a.w(ppc::cmpwi(0, 0));
    a.bc("ne", "placed");
    a.w(ppc::lis(11, 0x8000u32 as i32));
    if mem2 {
        a.w(ppc::lwz(12, 0x3128, 11));
        a.w(ppc::cmpwi(12, 0));
        a.bc("ne", "m2_ok");
        a.la(12, 0x935E0000);
        a.label("m2_ok");
        a.la(10, below);
        a.w(ppc::subf(12, 10, 12));
        a.w(ppc::rlwinm(12, 12, 0, 0, 26));
        a.w(ppc::stw(12, 0x3128, 11));
        if arena_hi != 0 {
            a.la(9, arena_hi);
            a.w(ppc::stw(9, 0x34, 11));
            a.w(ppc::stw(9, 0x3110, 11));
        }
    } else {
        // below the FST (0x80000038) and the BI2 block the apploader leaves under it (0x800000F4)
        a.w(ppc::lwz(12, 0x38, 11));
        a.w(ppc::lwz(10, 0xF4, 11));
        a.w(ppc::cmplw(10, 12));
        a.bc("ge", "bi2_ok");
        a.w(ppc::rlwinm(0, 10, 4, 28, 31));
        a.w(ppc::cmplwi(0, 8));
        a.bc("ne", "bi2_ok");
        a.w(ppc::mr(12, 10)); // only a MEM1 address
        a.label("bi2_ok");
        a.la(10, below);
        a.w(ppc::subf(12, 10, 12));
        a.w(ppc::rlwinm(12, 12, 0, 0, 26));
        a.w(ppc::mr(9, 12));
        if arena_hi != 0 {
            a.la(10, arena_hi);
            a.w(ppc::cmplw(10, 12));
            a.bc("ge", "ah_min");
            a.w(ppc::mr(9, 10));
            a.label("ah_min");
        }
        a.w(ppc::stw(9, 0x34, 11));
        a.w(ppc::stw(9, 0x3110, 11));
    }
    a.wf(|_, l| ppc::lis(10, hi_adj(l["ram"])));
    a.wf(|_, l| ppc::stw(12, (l["ram"] & 0xFFFF) as u16 as i16 as i32, 10));
    a.w(ppc::mr(3, 12));
    a.w(ppc::li(4, 0));
    a.la(5, data_size);
    a.bl_addr(MEMSET);
    a.label("placed");
    a.wf(|_, l| ppc::lis(29, hi_adj(l["table"])));
    a.wf(|_, l| ppc::addi(29, 29, (l["table"] & 0xFFFF) as i32));
    a.label("loop");
    a.w(ppc::lwz(28, 0, 29));
    a.w(ppc::cmpwi(28, 0));
    a.bc("eq", "tail");
    a.w(ppc::lwz(27, 4, 29));
    a.w(ppc::addi(29, 29, 8));
    a.w(ppc::cmplw(28, 30));
    a.bc("le", "lo_ok");
    a.w(ppc::mr(28, 30));
    a.label("lo_ok");
    a.w(ppc::cmplw(28, 31));
    a.bc("le", "skip");
    a.w(ppc::mr(3, 31));
    a.w(ppc::li(4, 0));
    a.w(ppc::subf(5, 31, 28));
    a.bl_addr(MEMSET);
    a.label("skip");
    a.w(ppc::cmplw(27, 31));
    a.bc("le", "loop");
    a.w(ppc::mr(31, 27));
    a.br("loop");
    a.label("tail");
    a.w(ppc::cmplw(30, 31));
    a.bc("le", "done");
    a.w(ppc::mr(3, 31));
    a.w(ppc::li(4, 0));
    a.w(ppc::subf(5, 31, 30));
    a.bl_addr(MEMSET);
    a.label("done");
    a.w(ppc::lmw(27, 0x0C, 1));
    a.w(ppc::lwz(0, 0x24, 1));
    a.w(ppc::mtlr(0));
    a.w(ppc::addi(1, 1, 0x20));
    a.w(ppc::BLR);
}

/// assemble at `base_addr`. RAM block + hash table: `fst_margin` below the FST (or at the top of MEM2 (`mem2`));
/// `arena_hi`: fixed MEM1 arena top (0: leave / the block)
pub fn build(base_addr: u32, protect: &[(u32, u32)], pack: &PackInfo, arena_hi: u32, fst_margin: u32, mem2: bool) -> Result<(Vec<u8>, HashMap<String, u32>)> {
    let mut a = Asm::new(base_addr);
    let ds = data_size(pack.paths.len());
    bss_clear(&mut a, arena_hi, ((if mem2 { 0 } else { fst_margin }) + ds + pack.index_load + 0x1F) & !0x1F, ds, mem2);
    sd_disc(&mut a, pack);
    a.label("table");
    let mut prot = protect.to_vec();
    prot.sort_by_key(|r| r.0);
    for (lo, hi) in prot {
        a.w(lo);
        a.w(hi);
    }
    a.w(0);
    a.w(0);
    a.label("ram");
    a.w(0);
    a.label("paths");
    for p in &pack.paths {
        let mut b = [0u8; 0x10];
        let t = p.as_bytes();
        require(t.len() < 0x10, "pack path too long")?;
        b[..t.len()].copy_from_slice(t);
        for k in (0..0x10).step_by(4) {
            a.w(get32(&b, k));
        }
    }
    Ok((a.bytes(), a.labels.clone()))
}
