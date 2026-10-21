//! BrawlBuilderEX: turn an extracted vanilla Brawl USA Rev 2 tree (wit layout: sys/, files/) plus a launcher-based mod into a
//! self-contained disc tree - no SD card, no launcher, no cheat file. Nothing in here is specific to one mod:
//!   1. SD card on the disc  : the mod folder goes onto the disc unchanged as pack files (sdpack) and Brawl's FA (SD) file API is
//!                              redirected to them (shims), so every SD reader of the mod keeps working
//!   2. launcher pre-pass    : the code handler (from the mod's own launcher) and the code list are embedded in main.dol at the address
//!                              the launcher used (gc.txt codeliststart - lists built with GCTRealMate -b point into themselves),
//!                              hooked like the launcher (VBI), and the unconditional writes and C2 hooks are installed statically
//!   3. memory layout        : the pack's hash table and the EX RAM go at the top of the MEM2 arena (MEM1 stays vanilla); only if the FST
//!                              outgrows the vanilla one do they go under it, with heaps the mod hard-codes there moved down

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::bin::{get32, put32, require, sha1_hex, ExError, Result};
use crate::dol::Dol;
use crate::exmod::ExMod;
use crate::fsutil::{self, ends_with_ignore_case, put, starts_with_ignore_case};
use crate::gecko::{self, GeckoCode, Line, MEM_TOP, VAN_ARENA_HI};
use crate::{lz, ppc, sdpack, shims, thp};

pub const VANILLA_MAIN_DOL_SHA1: &str = "a09cb9af20ae8356396374c27e31883d893f9ebe";
const HANDLER_ADDR: u32 = 0x80001800;
const HANDLER_ENTRY: u32 = 0x800018A8;
const VBI_HOOK: u32 = 0x80200984;
const BSS_CLEAR_CALL: u32 = 0x800042B8;
const GAME_ID_ADDR: u32 = 0x805A14B8;
const SD_MOUNTED: u32 = 0x805664EC;
const BSS_LO: u32 = 0x80494880;
const BSS_HI: u32 = 0x805A5154;
const LOW_LO: u32 = 0x800022C0;
const LOW_HI: u32 = 0x80003000;
// the EX RAM block + hash table are placed at boot FST_MARGIN below the FST (MEM1 layout); the arena/heap move is sized for an FST up to
// FST_SLACK bigger than this builder computes (Dolphin lays out folder-boot FSTs ~8 KiB bigger than wit images)
const FST_MARGIN: u32 = 0x100;
const FST_SLACK: u32 = 0x6000;

/// file data a dual-layer Wii disc holds (31 of every 32 KiB; the rest is hashes), minus headroom
pub const DISC_MAX: u64 = 8511160320 * 31 / 32 - 96 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitAnswer {
    Yes,
    YesAlways,
    No,
}

pub struct ExBuild<'a> {
    pub log: Box<dyn Fn(&str) + 'a>,
    pub report: Vec<String>,
    /// diagnosis: keep vanilla files the mod replaces
    pub keep_shadowed: bool,
    /// empty shadowed vanilla files / fit the disc (off: diagnosis, folder boot)
    pub free_disc_space: bool,
    /// diagnosis: force the MEM1 placement (heap move)
    pub mem1_index: bool,
    /// install unconditional writes / C2 hooks into main.dol (off: all at runtime, for diagnosis)
    pub bake: bool,
    /// delete the Subspace Emissary files
    pub remove_subspace: bool,
    /// allow shortening movies / removing Subspace / trophies to fit (CLI --fit)
    pub fit: bool,
    /// GUI: asked before each lossy step (overrides `fit`)
    pub ask_fit: Option<Box<dyn FnMut(&str) -> FitAnswer + 'a>>,
    /// "everything's okay": do the needed steps without asking
    pub fit_no_ask: bool,
    /// what the lossy steps took out, for the end-of-build summary
    pub removed: Vec<String>,
    /// Resources/SubspaceEmissaryFiles.txt
    pub subspace_list: Option<PathBuf>,
}

impl<'a> Default for ExBuild<'a> {
    fn default() -> Self {
        ExBuild {
            log: Box::new(|_| {}),
            report: Vec::new(),
            keep_shadowed: false,
            free_disc_space: true,
            mem1_index: false,
            bake: true,
            remove_subspace: false,
            fit: false,
            ask_fit: None,
            fit_no_ask: false,
            removed: Vec::new(),
            subspace_list: None,
        }
    }
}

pub fn arena_hi(fst: u32) -> u32 {
    MEM_TOP.wrapping_sub(fst) & !0x1F
}

/// FST bytes for the tree under files/, optionally without one folder
pub fn fst_size(files: &Path, without: Option<&Path>) -> u32 {
    let wo = without.map(|w| fsutil::full(w).to_string_lossy().to_lowercase());
    let (mut n, mut names) = (1u64, 0u64);
    for e in fsutil::entries_recursive(files) {
        if let Some(wo) = &wo {
            let ef = fsutil::full(&e).to_string_lossy().to_lowercase();
            if &ef == wo || ef.starts_with(&format!("{wo}{}", std::path::MAIN_SEPARATOR)) {
                continue;
            }
        }
        n += 1;
        names += fsutil::name(&e).len() as u64 + 1;
    }
    ((n * 12 + names + 3) & !3) as u32
}

fn common2_re() -> Regex {
    Regex::new(r"(?i)/system/common2(_en)?\.pac$").unwrap()
}

impl<'a> ExBuild<'a> {
    pub fn run(&mut self, tree: &Path, m: &ExMod, game_id: Option<&str>) -> Result<()> {
        let files = tree.join("files");
        let sys = tree.join("sys");
        let van_dol = fs::read(sys.join("main.dol"))?;
        let sha = sha1_hex(&van_dol);
        require(sha == VANILLA_MAIN_DOL_SHA1, format!("sys/main.dol is not the vanilla Brawl USA Rev 2 DOL (sha1 {sha})"))?;
        let van_fst = fs::read(sys.join("fst.bin"))?;
        require(arena_hi(van_fst.len() as u32) == VAN_ARENA_HI, "sys/fst.bin is not the vanilla FST")?;

        // 1. the mod's SD folder onto the disc
        let (sd, skipped) = m.sd_files();
        for (src, disc) in &sd {
            let dst = fsutil::join_disc(&files, disc);
            fs::create_dir_all(dst.parent().unwrap())?;
            if dst.exists() {
                fs::remove_file(&dst)?;
            }
            fs::copy(src, &dst)?;
        }
        (self.log)(&format!("SD folder /{}: {} files copied to the disc{}", m.root, sd.len(), if skipped > 0 { format!(" ({skipped} non-ASCII names skipped)") } else { String::new() }));
        if self.free_disc_space {
            self.free_space(tree, m, &sd)?;
        }

        // 2. code list
        let gct = gecko::parse(&fs::read(&m.gct_path)?)?;
        let mut codes: Option<Vec<GeckoCode>> = None;
        if let Some(cs) = &m.codeset_path {
            codes = gecko::from_codeset(&String::from_utf8_lossy(&fs::read(cs)?), &gct);
        }
        let mut codes = codes.unwrap_or_else(|| gecko::split_at_terminators(&gct));
        // diagnosis: extra Gecko lines ("XXXXXXXX YYYYYYYY") appended (lists point into themselves: never insert)
        if let Ok(extra) = std::env::var("EXCODES") {
            if !extra.is_empty() && Path::new(&extra).is_file() {
                let lines = fs::read_to_string(&extra)?
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| l.len() >= 17)
                    .map(|l| Line::new(u32::from_str_radix(&l[0..8], 16).unwrap_or(0), u32::from_str_radix(&l[9..17], 16).unwrap_or(0)))
                    .collect();
                codes.push(GeckoCode { name: "diag".into(), lines });
            }
        }
        (self.log)(&format!(
            "code list: {} lines, {} codes{}",
            gct.len(),
            codes.len(),
            if m.codeset_path.is_some() && !codes.is_empty() && codes[0].name != "code 0" { " (names from codeset)" } else { "" }
        ));

        // 3. memory layout
        let extra_gcts: Vec<(PathBuf, String)> =
            sd.iter().filter(|x| ends_with_ignore_case(&x.1, ".gct") && !fsutil::same_path(&x.0, &m.gct_path)).cloned().collect();
        let uses_heap = uses_heap(&codes, &extra_gcts, &sd)?;
        // the SD folder becomes pack files (sdpack), so the FST stays about vanilla-sized. Their hash table (4 bytes a file) and the EX RAM
        // go at the top of the MEM2 arena, leaving MEM1 exactly vanilla. Only if the FST still outgrows the vanilla one (which would push
        // the arena top below a heap the mod hard-codes there) do they go under the FST in MEM1 instead, with that heap and every pointer
        // to it moved down by whole 64 KiB steps (that path is fragile: a mod's own data can hold pointers we cannot see - with the
        // BrawlEx heap moved, P+Ex crashed creating the Ice Climbers' fighter instance mid-match; the MEM2 layout does not).
        let root_dir = files.join(&m.root);
        let (n_files, pack_names) = sdpack::layout(&files, &m.root)?;
        let fst_after = fst_size(&files, Some(&root_dir)) + pack_names.iter().map(|x| 12 + x.len() as u32 + 1).sum::<u32>();
        let data_size = shims::data_size(pack_names.len());
        let fst_start = arena_hi(fst_after);
        let mem2 = !self.mem1_index && (!uses_heap || fst_start >= VAN_ARENA_HI);
        let mut delta: i32 = 0;
        let target_hi;
        if mem2 {
            target_hi = if uses_heap { VAN_ARENA_HI } else { 0 };
            (self.log)(&format!(
                "SD pack: {} files in {} pack file(s); hash table 0x{:X} bytes + EX RAM at the top of MEM2 (MEM1 untouched)",
                n_files, pack_names.len(), sdpack::index_load(n_files)
            ));
        } else {
            let data_addr = fst_start.wrapping_sub(FST_SLACK + FST_MARGIN + sdpack::index_load(n_files) + data_size) & !0x1F;
            let n = if !uses_heap || data_addr >= VAN_ARENA_HI { 0 } else { (VAN_ARENA_HI - data_addr + 0xFFFF) / 0x10000 };
            delta = -0x10000 * n as i32;
            target_hi = if uses_heap { VAN_ARENA_HI.wrapping_add(delta as u32) } else { 0 };
            (self.log)(&format!(
                "SD pack: {} files in {} pack file(s); hash table 0x{:X} bytes + EX RAM under the FST (~0x{:08X}){}",
                n_files,
                pack_names.len(),
                sdpack::index_load(n_files),
                data_addr,
                if uses_heap { format!(", arena top 0x{:08X}, BrawlX/Syringe heap moved by -0x{:X}", target_hi, -delta) } else { String::new() }
            ));
        }
        if delta != 0 {
            let re = common2_re();
            for (src, disc) in sd.iter().filter(|x| re.is_match(&x.1)) {
                let raw = fs::read(src)?;
                if !lz::is_compressed(&raw) {
                    continue;
                }
                let mut dec = lz::decompress(&raw)?;
                let mut hits = 0;
                for i in 0..dec.len().saturating_sub(3) {
                    if get32(&dec, i) == gecko::HEAP_OLD {
                        put32(&mut dec, i, gecko::HEAP_OLD.wrapping_add(delta as u32));
                        hits += 1;
                    }
                }
                if hits > 0 {
                    put(&fsutil::join_disc(&files, disc), &lz::compress(&dec, raw[0] == 0x11))?;
                    self.report.push(format!("{disc}: {hits} heap-base relocation addends moved"));
                }
            }
        }

        // code fix-ups (heap pointers, FST-name pointers)
        let cl = m.code_list_start;
        let cl_sec = cl & !0x1F;
        let glen = 16 + 8 * codes.iter().map(|c| c.lines.len() as u32).sum::<u32>();
        let str_base = (cl + glen + 0x1F) & !0x1F;
        let mut str_blob: Vec<u8> = Vec::new();
        let mut strings: HashMap<Vec<u8>, u32> = HashMap::new();
        let mut fst_name = |a: u32| -> Option<Vec<u8>> {
            let o = a as i64 - VAN_ARENA_HI as i64;
            if o < 0 || o >= van_fst.len() as i64 {
                return None;
            }
            let o = o as usize;
            match van_fst[o..].iter().position(|&b| b == 0) {
                Some(e) => Some(van_fst[o..=o + e].to_vec()),
                None => Some(Vec::new()),
            }
        };
        let mut string_addr = |s: &[u8]| -> u32 {
            if let Some(&a) = strings.get(s) {
                return a;
            }
            let a = str_base + str_blob.len() as u32;
            strings.insert(s.to_vec(), a);
            str_blob.extend_from_slice(s);
            a
        };
        let van = Dol::new(&van_dol);
        let dol_word = |a: u32| van.read_or0(a);
        for c in codes.iter_mut() {
            let notes = gecko::fix_heap_and_fst(c, delta, &mut fst_name, &mut string_addr, &dol_word)?;
            if !notes.is_empty() {
                self.report.push(format!("{}: {}", c.name, notes.join("; ")));
            }
        }
        for (src, disc) in &extra_gcts {
            let raw = fs::read(src)?;
            if raw.len() < 16 || get32(&raw, 0) != 0x00D0C0DE {
                continue;
            }
            let mut parts = gecko::split_at_terminators(&gecko::parse(&raw)?);
            let mut notes = Vec::new();
            for c in parts.iter_mut() {
                notes.extend(gecko::fix_heap_and_fst(c, delta, &mut fst_name, &mut string_addr, &dol_word)?);
            }
            if !notes.is_empty() {
                put(&fsutil::join_disc(&files, disc), &gecko::build(&parts))?;
                self.report.push(format!("{disc}: {}", notes.join("; ")));
            }
        }
        drop(string_addr);
        // every edit of the SD copy is done: pack it
        let pk = sdpack::write(&files, &m.root)?;
        require(arena_hi(fst_size(&files, None)) == fst_start, "FST size after packing differs from the estimate")?;
        let mut g = gecko::build(&codes);
        let data_end = (str_base + str_blob.len() as u32 + 0x1F) & !0x1F;
        require(data_end <= m.code_list_end, format!("the code list does not fit below codelistend (0x{:08X})", m.code_list_end))?;

        // 4. main.dol
        let mut dol = Dol::new(&van_dol);
        dol.write(VBI_HOOK, ppc::b(VBI_HOOK, HANDLER_ENTRY, false), Some(ppc::BLR))?;
        for p in &m.pokes {
            if p.write_addr == BSS_CLEAR_CALL {
                continue; // replaced by our protecting .bss clear
            }
            let ok = match p.if_equal {
                None => true,
                Some(v) => dol.section(p.addr).is_some() && dol.read(p.addr)? == v,
            };
            if ok {
                dol.write(p.write_addr, p.value, None)?;
            }
        }
        if let Some(id) = game_id.filter(|id| !id.is_empty() && *id != "RSBE01") {
            require(id.len() == 6 && id.starts_with("RSBE"), "game id must be RSBExx")?;
            dol.write_bytes(GAME_ID_ADDR, &id.as_bytes()[4..])?;
        }
        // pre-pass: unconditional writes and C2 hooks
        let mut extra: BTreeMap<u32, u8> = BTreeMap::new();
        let (mut nin, mut nbss, mut nrt, mut nhook, mut gline) = (0, 0, 0, 0, 0usize);
        for c in &codes {
            let base_line = gline;
            gline += c.lines.len();
            let mut writes = Vec::new();
            let mut hooks = Vec::new();
            if self.bake {
                gecko::static_ops(&c.lines, &mut writes, &mut hooks);
            }
            for (hdr, hk, nl) in hooks {
                match dol.section(hk) {
                    Some(si) if si < 7 => {}
                    _ => continue,
                }
                let pay = cl + 8 + 8 * (base_line + hdr + 1) as u32;
                let last = pay + 8 * nl as u32 - 4;
                let o = (last - cl) as usize;
                if get32(&g, o) != 0 {
                    self.report.push(format!("C2 {hk:08X} left to the runtime handler"));
                    continue;
                }
                dol.write(hk, ppc::b(hk, pay, false), None)?;
                put32(&mut g, o, ppc::b(last, hk + 4, false));
                nhook += 1;
            }
            for (va, b) in writes {
                if dol.covers(va, b.len()) {
                    dol.write_bytes(va, &b)?;
                    nin += 1;
                } else if va >= BSS_LO && va as u64 + b.len() as u64 <= BSS_HI as u64 && dol.free(va, b.len()) {
                    for (k, &v) in b.iter().enumerate() {
                        extra.insert(va + k as u32, v);
                    }
                    nbss += 1;
                } else {
                    nrt += 1;
                }
            }
        }
        let mut blobs: Vec<(u32, Vec<u8>)> = Vec::new();
        for (&va, &v) in &extra {
            let lo = va & !0x1F;
            let merge = blobs.last().map(|b| lo <= b.0 + b.1.len() as u32 + 0x4000).unwrap_or(false);
            if merge {
                let bl = blobs.last_mut().unwrap();
                let hi = (bl.0 + bl.1.len() as u32).max(lo + 0x20);
                if hi > bl.0 + bl.1.len() as u32 {
                    bl.1.resize((hi - bl.0) as usize, 0);
                }
            } else {
                blobs.push((lo, vec![0u8; 0x20]));
            }
            let cur = blobs.last_mut().unwrap();
            cur.1[(va - cur.0) as usize] = v;
        }
        for bl in &blobs {
            require(!(bl.0 < data_end && cl_sec < bl.0 + bl.1.len() as u32), "a baked write lands in the code list area")?;
        }
        // Brawl's own "SD card mounted" flag: our shims are the SD card, and the File Patch Codes only run while it is set
        {
            let lo = SD_MOUNTED & !0x1F;
            match blobs.iter_mut().find(|b| b.0 <= SD_MOUNTED && SD_MOUNTED < b.0 + b.1.len() as u32) {
                Some(b) => b.1[(SD_MOUNTED - b.0 + 3) as usize] = 1,
                None => {
                    let mut fb = vec![0u8; 0x20];
                    fb[(SD_MOUNTED - lo + 3) as usize] = 1;
                    blobs.push((lo, fb));
                }
            }
        }
        (self.log)(&format!("launcher pre-pass: {nin} writes into main.dol, {nbss} into .bss, {nrt} runtime-only, {nhook} C2 hooks installed"));
        // handler, EX helpers (text), code list + strings + helper data (data), baked .bss blocks
        let mut protect: Vec<(u32, u32)> = vec![(cl_sec, data_end)];
        protect.extend(blobs.iter().map(|b| (b.0, b.0 + b.1.len() as u32)));
        // the EX helpers go in the low-memory space after the handler where no code of the mod touches (P+Ex's VBI hook switch writes
        // 0x80002778, Legacy TE's Tags Controls keeps a flag at 0x80002800 that redirects boot to the Controls menu), else after the code list
        let mut est = protect.clone();
        est.push((0, 0));
        let shim_len = shims::build(0x80002000, &est, &pk, target_hi, FST_MARGIN, mem2)?.0.len() as u32;
        let mut used = gecko::writes_in(&gct, LOW_LO, LOW_HI);
        for (src, _) in &extra_gcts {
            let raw = fs::read(src)?;
            if raw.len() >= 16 && get32(&raw, 0) == 0x00D0C0DE {
                used.extend(gecko::writes_in(&gecko::parse(&raw)?, LOW_LO, LOW_HI));
            }
        }
        let mut shim_base = 0u32;
        let mut at = LOW_LO;
        while at + shim_len <= LOW_HI {
            let hit: Vec<&(u32, u32)> = used.iter().filter(|u| u.0 < at + shim_len && u.1 > at).collect();
            if hit.is_empty() {
                shim_base = at;
                break;
            }
            at = (hit.iter().map(|u| u.1).max().unwrap() + 0x1F) & !0x1F;
        }
        if shim_base == 0 {
            shim_base = data_end;
            protect.push((shim_base, (shim_base + shim_len + 0x1F) & !0x1F));
            require(shim_base + shim_len <= m.code_list_end, "no room for the EX helpers (low memory is used by the mod's codes and the code list area is full)")?;
        }
        let (shim, labels) = shims::build(shim_base, &protect, &pk, target_hi, FST_MARGIN, mem2)?;
        require(shim.len() as u32 <= shim_len, "EX helpers grew")?;
        let mut h = m.handler.clone();
        require(get32(&h, 0x104) == 0x3DE08000 && get32(&h, 0x108) == 0x61EF22A8, "unexpected handler template")?;
        put32(&mut h, 0x104, 0x3DE00000 | (cl >> 16));
        put32(&mut h, 0x108, 0x61EF0000 | (cl & 0xFFFF));
        dol.add(HANDLER_ADDR, &h, true)?;
        dol.add(shim_base, &shim, true)?;
        dol.write(BSS_CLEAR_CALL, ppc::b(BSS_CLEAR_CALL, labels["bss"], true), Some(0x48000185))?;
        for (name, addr, old) in shims::FA_ENTRIES {
            dol.write(addr, ppc::b(addr, labels[&format!("fa_{name}")], false), Some(old))?;
        }
        let mut list_sec = vec![0u8; (data_end - cl_sec) as usize];
        list_sec[(cl - cl_sec) as usize..(cl - cl_sec) as usize + g.len()].copy_from_slice(&g);
        list_sec[(str_base - cl_sec) as usize..(str_base - cl_sec) as usize + str_blob.len()].copy_from_slice(&str_blob);
        dol.add(cl_sec, &list_sec, false)?;
        for bl in &blobs {
            dol.add(bl.0, &bl.1, false)?;
        }
        put(&sys.join("main.dol"), &dol.to_vec())?;
        // the game compares its own id with the disc's
        if let Some(id) = game_id.filter(|id| !id.is_empty()) {
            for hp in [sys.join("boot.bin"), tree.join("disc").join("header.bin")] {
                if hp.is_file() {
                    let mut hb = fs::read(&hp)?;
                    hb[..id.len()].copy_from_slice(id.as_bytes());
                    put(&hp, &hb)?;
                }
            }
        }
        (self.log)(&format!("main.dol: code handler @0x80001800, code list @0x{cl:08X}, EX helpers @0x{shim_base:08X}, SD file API -> disc"));
        Ok(())
    }

    /// Vanilla files the mod replaces are never read (the File Patch Code checks the mod's copy first): empty them, and the dummy padding
    /// files, keeping their FST entries. If the image is still bigger than a dual-layer disc, take the least destructive steps, asking first.
    fn free_space(&mut self, tree: &Path, m: &ExMod, sd: &[(PathBuf, String)]) -> Result<()> {
        let files = tree.join("files");
        let mut freed = 0u64;
        let mut vanilla: HashMap<String, PathBuf> = HashMap::new();
        let root_prefix = format!("{}/", m.root);
        for f in fsutil::files_recursive(&files) {
            let rel = fsutil::rel(&files, &f);
            if !starts_with_ignore_case(&rel, &root_prefix) {
                vanilla.insert(rel.to_lowercase(), f);
            }
        }
        let pf_prefix = if m.dir.join("pf").is_dir() { format!("{}/pf/", m.root) } else { format!("{}/", m.root) };
        for (_, disc) in sd {
            if !starts_with_ignore_case(disc, &pf_prefix) {
                continue;
            }
            let rel = &disc[pf_prefix.len()..];
            // exact paths only: a mod's X.pac does not always replace the localized X_en.pac (P+Ex ships 64-byte common.pac stubs
            // and the game still loads system/common_en.pac from the disc)
            let Some(hit) = vanilla.get(&rel.to_lowercase()) else { continue };
            if self.keep_shadowed {
                continue;
            }
            let len = fsutil::file_len(hit);
            if len > 0 {
                freed += len;
                put(hit, &[])?;
            }
        }
        for d in ["dummy1.dat", "dummy2.dat", "border.dat"] {
            // disc padding, never read: removed (keeps the FST small)
            let f = files.join(d);
            if f.is_file() {
                freed += fsutil::file_len(&f);
                fs::remove_file(&f)?;
            }
        }
        let mut sse: Vec<PathBuf> = match &self.subspace_list {
            Some(l) if l.is_file() => fs::read_to_string(l)?
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(|l| fsutil::join_disc(&files, &l.replace('\\', "/")))
                .filter(|p| p.is_file())
                .collect(),
            _ => Vec::new(),
        };
        let sse_found = !sse.is_empty();
        let remove_sse = |sse: &mut Vec<PathBuf>, report: &mut Vec<String>| -> Result<()> {
            for f in sse.iter() {
                fs::remove_file(f)?;
            }
            report.push(format!("Subspace Emissary removed ({} files)", sse.len()));
            sse.clear();
            Ok(())
        };
        if self.remove_subspace && !sse.is_empty() {
            remove_sse(&mut sse, &mut self.report)?;
            self.removed.push("Subspace Emissary (you chose to remove it)".into());
        }
        let mut total: u64 = fsutil::files_recursive(tree).iter().map(|f| fsutil::file_len(f)).sum();
        if total > DISC_MAX {
            // too big: the least destructive steps first (vanilla movies to 10-frame placeholders, Subspace, trophy models),
            // each only while still needed, each asked for unless the user said everything's okay (or --fit on the command line)
            let md = files.join("movie");
            let fd = files.join("toy").join("fig");
            let mut movies: Vec<PathBuf> = if md.is_dir() {
                fsutil::files_in(&md).into_iter().filter(|f| ends_with_ignore_case(&f.to_string_lossy(), ".thp") && fsutil::file_len(f) >= 2000000).collect()
            } else {
                Vec::new()
            };
            movies.sort_by(|a, b| fsutil::file_len(b).cmp(&fsutil::file_len(a)));
            // trophy models only (not the pedestal Stand*.brres, name lists or data): each becomes a copy of the smallest one, so the
            // Trophy Gallery still works (every trophy shows that model) - emptied models crash it
            let figs: Vec<PathBuf> = if fd.is_dir() {
                fsutil::files_in(&fd)
                    .into_iter()
                    .filter(|f| ends_with_ignore_case(&f.to_string_lossy(), ".brres") && !starts_with_ignore_case(&fsutil::name(f), "Stand"))
                    .collect()
            } else {
                Vec::new()
            };
            let stub: Option<Vec<u8>> = match figs.iter().min_by_key(|f| fsutil::file_len(f)) {
                Some(f) => Some(fs::read(f)?),
                None => None,
            };
            let sz = |l: &[PathBuf]| -> u64 { l.iter().map(|f| fsutil::file_len(f)).sum() };
            let stub_len = stub.as_ref().map(|s| s.len() as u64).unwrap_or(0);
            let most = total as i64 - sz(&movies) as i64 - sz(&sse) as i64 - if figs.is_empty() { 0 } else { sz(&figs) as i64 - (figs.len() as u64 * stub_len) as i64 };
            require(
                most <= DISC_MAX as i64,
                format!("the image would be {:.2} GB, more than a dual-layer Wii disc holds ({:.2} GB), even after shortening movies, removing Subspace and trophies.", total as f64 / 1e9, DISC_MAX as f64 / 1e9),
            )?;
            let fit = self.fit;
            let ask = |me: &mut Self, total: u64, what: &str| -> Result<()> {
                if me.fit_no_ask {
                    return Ok(());
                }
                let msg = format!(
                    "This mod makes a {:.2} GB image; a dual-layer Wii disc holds {:.2} GB.\n\nTo fit, BrawlBuilderEX needs to {}.",
                    total as f64 / 1e9,
                    DISC_MAX as f64 / 1e9,
                    what
                );
                match me.ask_fit.as_mut() {
                    None => require(fit, format!("{msg} Run again with --fit to allow this.")),
                    Some(f) => {
                        let a = f(&msg);
                        require(a != FitAnswer::No, "cancelled: the image does not fit a dual-layer disc")?;
                        if a == FitAnswer::YesAlways {
                            me.fit_no_ask = true;
                        }
                        Ok(())
                    }
                }
            };
            if !movies.is_empty() {
                ask(self, total, "shorten the vanilla movies (intro, character endings, ...) to 10-frame clips")?;
                let mut n = 0;
                for f in &movies {
                    if total <= DISC_MAX {
                        break;
                    }
                    let len = fsutil::file_len(f);
                    let t = thp::truncate(&fs::read(f)?, 10)?;
                    put(f, &t)?;
                    total -= len - t.len() as u64;
                    n += 1;
                }
                self.report.push(format!("fit: {n} vanilla movies shortened to 10 frames"));
                self.removed.push(format!("{n} vanilla movies (shortened to 10-frame clips)"));
            }
            if total > DISC_MAX && !sse.is_empty() {
                ask(self, total, "delete Subspace Emissary (\"Delete Subspace Emissary\" is not checked)")?;
                total -= sz(&sse);
                remove_sse(&mut sse, &mut self.report)?;
                self.removed.push("Subspace Emissary".into());
            }
            if total > DISC_MAX && !figs.is_empty() {
                ask(self, total, "replace the trophy models with one small model (the Trophy Gallery still works, but every trophy looks the same)")?;
                let stub = stub.unwrap();
                for f in &figs {
                    let len = fsutil::file_len(f);
                    if len <= stub.len() as u64 {
                        continue;
                    }
                    total -= len - stub.len() as u64;
                    put(f, &stub)?;
                }
                self.report.push(format!("fit: {} trophy models replaced by one small model (every trophy in the Vault looks the same)", figs.len()));
                self.removed.push(format!("{} trophy models (all trophies now show one small model)", figs.len()));
            }
        }
        if (self.remove_subspace || sse_found) && sse.is_empty() {
            // Subspace is gone: hide its Solo menu panel (selecting it would hang). The mod's copy is the one the game loads, if it has one.
            let pf = if m.dir.join("pf").is_dir() { "pf/" } else { "" };
            let mut menu = fsutil::join_disc(&files, &format!("{}/{pf}module/sora_menu_main.rel", m.root));
            if !menu.is_file() {
                menu = files.join("module").join("sora_menu_main.rel");
            }
            if crate::ssemenu::hide_subspace_file(&menu)? {
                self.report.push(format!("Subspace Emissary hidden in the Solo menu ({})", fsutil::rel(&files, &menu)));
            } else {
                self.report.push("the Solo menu module is not the vanilla one: the Subspace Emissary button is still there".into());
                self.removed.push("(the Subspace Emissary button is still in the Solo menu: this mod's menu is not the vanilla one, don't pick it)".into());
            }
        }
        (self.log)(&format!("disc space: {:.2} GB of unused vanilla data emptied, image {:.2} GB", freed as f64 / 1e9, total as f64 / 1e9));
        require(total <= DISC_MAX, format!("the image would be {:.2} GB, more than a dual-layer Wii disc holds ({:.2} GB). ", total as f64 / 1e9, DISC_MAX as f64 / 1e9))
    }
}

fn uses_heap(codes: &[GeckoCode], extra_gcts: &[(PathBuf, String)], sd: &[(PathBuf, String)]) -> Result<bool> {
    let probe = |c: &GeckoCode| -> Result<bool> {
        let mut copy = c.clone();
        let notes = gecko::fix_heap_and_fst(&mut copy, -0x10000, &mut |_| None, &mut |_| 0x80000000, &|_| 0)?;
        Ok(notes.iter().any(|x| !x.contains("FST") && !x.contains("without")))
    };
    for c in codes {
        if probe(c)? {
            return Ok(true);
        }
    }
    for (src, _) in extra_gcts {
        let raw = fs::read(src)?;
        if raw.len() >= 16 && get32(&raw, 0) == 0x00D0C0DE {
            for c in gecko::split_at_terminators(&gecko::parse(&raw)?) {
                if probe(&c)? {
                    return Ok(true);
                }
            }
        }
    }
    let re = common2_re();
    for (src, _) in sd.iter().filter(|x| re.is_match(&x.1)) {
        let raw = fs::read(src)?;
        if !lz::is_compressed(&raw) {
            continue;
        }
        let dec = lz::decompress(&raw)?;
        if (0..dec.len().saturating_sub(3)).any(|i| get32(&dec, i) == gecko::HEAP_OLD) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[allow(dead_code)]
fn _e(_: ExError) {}
