//! With Subspace deleted, picking it in the Solo menu hangs (the adventure modules are gone). Brawl already hides a panel there: All-Star,
//! until it is unlocked. The Solo menu (muProc1p in module/sora_menu_main.rel) adds its six panels in a loop and skips All-Star (panel 2)
//! when GameGlobal::checkHideMenuAllstar says so; the cursor moves through a table of 6 panels x (up, down, left, right), each
//! {u16 panel, u8 wrap, u8 0}. Panels: 0 Classic, 1 Subspace Emissary, 2 All-Star, 3 Events, 4 Stadium, 5 Training:
//!     Classic   | Subspace
//!     All-Star  |
//!     Events | Stadium | Training
//! Hiding Subspace the same way: the loop skips panel 1 too, the cursor starts on Classic instead of Subspace, and nothing moves onto it.
//! Only the vanilla module is patched: a mod's own sora_menu_main.rel is left alone.

use crate::bin::get32;

const TEXT: usize = 1;
const DATA: usize = 5;
const NAV: u32 = 0x19D8;

/// (text offset, vanilla word, patched word)
const CODE: [(u32, u32, u32); 5] = [
    // the panel loop: x = i*2 + (All-Star shown), skip when 2 <= x <= 4 (Subspace always, All-Star while hidden)
    (0x130F4, 0x2C030000, 0x5343083C), // cmpwi r3,0       -> rlwimi r3,r26,1,0,30
    (0x130F8, 0x4082000C, 0x3803FFFE), // bne 0x13104      -> subi r0,r3,2
    (0x130FC, 0x281A0002, 0x28000002), // cmplwi r26,2     -> cmplwi r0,2
    (0x13100, 0x4182002C, 0x4081002C), // beq 0x1312c      -> ble 0x1312c
    // first visit: the cursor starts on Classic
    (0x131F4, 0x38000001, 0x38000000), // li r0,1          -> li r0,0
];

/// (panel, direction 0 up / 1 down / 2 left / 3 right, vanilla {panel, wrap}, new {panel, wrap})
const MOVES: [(u32, u32, (u16, u8), (u16, u8)); 8] = [
    (0, 2, (1, 1), (0, 0)), // Classic left/right: stay
    (0, 3, (1, 0), (0, 0)),
    (2, 2, (1, 1), (2, 0)), // All-Star left/right: stay
    (2, 3, (1, 0), (2, 0)),
    (4, 0, (1, 0), (0, 0)), // Stadium up: Classic, down: around to Classic
    (4, 1, (1, 1), (0, 1)),
    (5, 0, (1, 0), (0, 0)), // Training the same
    (5, 1, (1, 1), (0, 1)),
];

fn section(rel: &[u8], i: usize) -> Option<usize> {
    if rel.len() < 0x40 || get32(rel, 0) != 2 {
        return None; // not sora_menu_main (module id 2)
    }
    let table = get32(rel, 0x10) as usize;
    let count = get32(rel, 0x0C) as usize;
    if i >= count || table + 8 * i + 8 > rel.len() {
        return None;
    }
    Some((get32(rel, table + 8 * i) & !1) as usize)
}

/// hide the Subspace Emissary panel; false (and nothing changed) unless this is the vanilla module
pub fn hide_subspace(rel: &mut [u8]) -> bool {
    let (Some(text), Some(data)) = (section(rel, TEXT), section(rel, DATA)) else { return false };
    let nav = |panel: u32, dir: u32| data + (NAV + panel * 16 + dir * 4) as usize;
    if text + 0x13104 > rel.len() || nav(5, 4) > rel.len() {
        return false;
    }
    let vanilla = CODE.iter().all(|&(o, v, _)| get32(rel, text + o as usize) == v)
        && MOVES.iter().all(|&(p, d, (t, w), _)| {
            let o = nav(p, d);
            u16::from_be_bytes([rel[o], rel[o + 1]]) == t && rel[o + 2] == w && rel[o + 3] == 0
        });
    if !vanilla {
        return false;
    }
    for &(o, _, n) in &CODE {
        let o = text + o as usize;
        rel[o..o + 4].copy_from_slice(&n.to_be_bytes());
    }
    for &(p, d, _, (t, w)) in &MOVES {
        let o = nav(p, d);
        rel[o..o + 4].copy_from_slice(&[(t >> 8) as u8, t as u8, w, 0]);
    }
    true
}

/// hide_subspace on a file (written as a new file: the tree can hold hard links); Ok(false) if missing or not the vanilla module
pub fn hide_subspace_file(path: &std::path::Path) -> std::io::Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut rel = std::fs::read(path)?;
    if !hide_subspace(&mut rel) {
        return Ok(false);
    }
    crate::fsutil::put(path, &rel)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    /// BBEX_MENU_REL = a vanilla module/sora_menu_main.rel, BBEX_MENU_REL_HIDDEN = the same with the panel hidden (from a tested build)
    #[test]
    fn vanilla_module() {
        let (Ok(a), Ok(b)) = (std::env::var("BBEX_MENU_REL"), std::env::var("BBEX_MENU_REL_HIDDEN")) else { return };
        let mut rel = std::fs::read(a).unwrap();
        assert!(super::hide_subspace(&mut rel));
        assert!(rel == std::fs::read(b).unwrap());
        assert!(!super::hide_subspace(&mut rel)); // only once
    }
}
