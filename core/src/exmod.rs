//! A launcher-based Brawl mod (Project+ family: Project+, P+Ex/PMEX, Legacy, Minus, Void, ...), found by layout instead of by name:
//! the folder that holds gc.txt + RSBE01.GCT is the mod folder ("/Minusery", "/Project+", "/ex_remix", "/P+Ex"); its name is the SD root
//! the mod's codes use. The Ocarina code handler is taken from the mod's own launcher (.dol/.elf) by its code-list template.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::bin::{get32, index_of, require, sha1_hex, Result};
use crate::fat::FatImage;
use crate::fsutil::{self, eq_ignore_case, ends_with_ignore_case};
use crate::gecko::{self, Line};

pub const HANDLER_LENGTH: usize = 0xAA8;
pub const HANDLER_TEMPLATE_OFFSET: usize = 0x104;
const HANDLER_TEMPLATE: [u8; 8] = [0x3D, 0xE0, 0x80, 0x00, 0x61, 0xEF, 0x22, 0xA8]; // lis r15,0x8000 ; ori r15,r15,0x22A8
const KNOWN_HANDLERS: [&str; 1] = ["21089b4b436dc280686a569d7af45edf96f4c276"];

#[derive(Clone, Debug, Default)]
pub struct Poke {
    pub addr: u32,
    pub if_equal: Option<u32>,
    pub write_addr: u32,
    pub value: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ExMod {
    pub dir: PathBuf,
    pub root: String,
    pub gct_path: PathBuf,
    pub codeset_path: Option<PathBuf>,
    pub code_list_start: u32,
    pub code_list_end: u32,
    pub pokes: Vec<Poke>,
    pub handler: Vec<u8>,
    pub handler_source: PathBuf,
}

impl ExMod {
    /// Accepts what a user is likely to pick: the mod's download folder, its SD folder, its "pf" folder, or a folder / file with an sd.raw
    /// image (extracted to `work`). Returns None if this is not a launcher-based mod.
    pub fn resolve(path: &Path, work: &Path, log: &dyn Fn(&str)) -> Result<Option<ExMod>> {
        let s = path.to_string_lossy();
        if path.is_file() && (ends_with_ignore_case(&s, ".raw") || ends_with_ignore_case(&s, ".img")) {
            return Self::from_image(path, work, log).map(Some);
        }
        if !path.is_dir() {
            return Ok(None);
        }
        let mut p = Some(fsutil::full(path));
        for _ in 0..3 {
            let Some(d) = p.clone() else { break };
            if let Ok(m) = Self::find(&d) {
                return Ok(Some(m));
            }
            if let Some(raw) = fsutil::files_in(&d).into_iter().find(|f| ends_with_ignore_case(&f.to_string_lossy(), ".raw")) {
                return Self::from_image(&raw, work, log).map(Some);
            }
            p = d.parent().map(|x| x.to_path_buf());
        }
        Ok(None)
    }

    fn from_image(raw: &Path, work: &Path, log: &dyn Fn(&str)) -> Result<ExMod> {
        let dir = work.join("sd");
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        log(&format!("extracting {} ...", fsutil::name(raw)));
        FatImage::open(raw)?.extract_all(&dir)?;
        Self::find(&dir)
    }

    pub fn find(package: &Path) -> Result<ExMod> {
        // the code list: RSBE01.GCT (any case) with a launcher config: gc.txt beside it, or gameconfig.txt up to two folders above
        // (MinimaLauncher layout: <sd>/codes/RSBE01.gct + <sd>/gameconfig.txt)
        let all = fsutil::files_recursive(package);
        let mut cands: Vec<(PathBuf, PathBuf)> = Vec::new();
        for f in all.iter().filter(|f| eq_ignore_case(&fsutil::name(f), "RSBE01.GCT")) {
            let dir = f.parent().unwrap().to_path_buf();
            let mut cfg = file_in(&dir, "gc.txt");
            let mut up = Some(dir.clone());
            for _ in 0..3 {
                if cfg.is_some() {
                    break;
                }
                let Some(u) = up.clone() else { break };
                cfg = file_in(&u, "gameconfig.txt");
                up = u.parent().map(|x| x.to_path_buf());
            }
            if let Some(cfg) = cfg {
                cands.push((f.clone(), cfg));
            }
        }
        require(!cands.is_empty(), format!("no RSBE01.GCT with a launcher config (gc.txt / gameconfig.txt) found in {} - is this a launcher-based mod?", package.display()))?;
        // shortest path first (stable: enumeration order breaks ties)
        let pick = cands.iter().min_by_key(|c| c.0.to_string_lossy().encode_utf16().count()).unwrap().clone();
        let mut m = ExMod { gct_path: pick.0.clone(), ..Default::default() };
        let gdir = m.gct_path.parent().unwrap().to_path_buf();
        m.parse_gc(&String::from_utf8_lossy(&fs::read(&pick.1)?))?;
        // the SD folder the mod's codes use: the File Patch Code's root string, else the most frequent "/<folder>" string naming a real folder
        let gct_raw = fs::read(&m.gct_path)?;
        let mut root = root_string(&gecko::parse(&gct_raw)?);
        let mut near: Vec<PathBuf> = Vec::new();
        {
            let mut up = Some(gdir.clone());
            for _ in 0..3 {
                let Some(u) = up.clone() else { break };
                near.extend(fsutil::dirs_in(&u));
                near.push(u.clone());
                up = u.parent().map(|x| x.to_path_buf());
            }
        }
        if root.is_none() {
            let latin1: String = gct_raw.iter().map(|&b| b as char).collect();
            let names: Vec<String> = near.iter().map(|d| fsutil::name(d)).collect();
            let skip = ["pf", "apps", "codes", "private", "module", "sound", "system"];
            let re = Regex::new(r"/([A-Za-z0-9_+.-]{2,32})[/\x00]").unwrap();
            let mut counts: Vec<(String, usize)> = Vec::new();
            let mut pos = 0;
            while let Some(c) = re.captures_at(&latin1, pos) {
                let whole = c.get(0).unwrap();
                let n = c[1].to_string();
                if names.iter().any(|x| eq_ignore_case(x, &n)) && !skip.iter().any(|s| eq_ignore_case(s, &n)) {
                    match counts.iter_mut().find(|(k, _)| eq_ignore_case(k, &n)) {
                        Some(e) => e.1 += 1,
                        None => counts.push((n, 1)),
                    }
                }
                pos = whole.end();
            }
            // OrderByDescending(count).First(): the first key reaching the maximum
            root = Some(match counts.iter().max_by(|a, b| a.1.cmp(&b.1).then(std::cmp::Ordering::Greater)) {
                Some((k, _)) => k.clone(),
                None => fsutil::name(&gdir),
            });
        }
        let root = root.unwrap();
        let dir = near.iter().find(|d| eq_ignore_case(&fsutil::name(d), &root)).cloned();
        require(dir.is_some(), format!("the SD folder /{root} was not found next to {}", m.gct_path.display()))?;
        m.dir = dir.unwrap();
        m.root = fsutil::name(&m.dir);
        for cs in ["codeset.txt", "RSBE01_codeset.txt"] {
            if let Some(f) = file_in(&gdir, cs) {
                m.codeset_path = Some(f);
            }
        }
        // handler: prefer a known console launcher's copy
        let mut found: Vec<(u32, PathBuf, Vec<u8>)> = Vec::new();
        for f in all.iter().filter(|f| {
            let s = f.to_string_lossy();
            ends_with_ignore_case(&s, ".dol") || ends_with_ignore_case(&s, ".elf")
        }) {
            let d = fs::read(f)?;
            let Some(i) = index_of(&d, &HANDLER_TEMPLATE, 0) else { continue };
            if i < HANDLER_TEMPLATE_OFFSET || i - HANDLER_TEMPLATE_OFFSET + HANDLER_LENGTH > d.len() {
                continue;
            }
            let h = d[i - HANDLER_TEMPLATE_OFFSET..i - HANDLER_TEMPLATE_OFFSET + HANDLER_LENGTH].to_vec();
            if get32(&h, 0xA8) != 0x9421FF58 {
                continue; // entry (0x800018A8) must be the handler's prologue
            }
            let rank = (if KNOWN_HANDLERS.contains(&sha1_hex(&h).as_str()) { 0 } else { 2 })
                + if f.to_string_lossy().to_ascii_lowercase().contains("dolphin") { 1 } else { 0 };
            found.push((rank, f.clone(), h));
        }
        require(!found.is_empty(), format!("no Gecko code handler found in the mod's launcher files (.dol/.elf) under {}", package.display()))?;
        let best = found.iter().min_by_key(|x| x.0).unwrap().clone();
        m.handler = best.2;
        m.handler_source = best.1;
        Ok(m)
    }

    fn parse_gc(&mut self, t: &str) -> Result<()> {
        let hex = |k: &str| -> Result<u32> {
            let re = Regex::new(&format!(r"{k}\s*=\s*([0-9A-Fa-f]+)")).unwrap();
            let c = re.captures(t);
            require(c.is_some(), format!("gc.txt has no {k}"))?;
            Ok(u32::from_str_radix(&c.unwrap()[1], 16).unwrap_or(0))
        };
        self.code_list_start = hex("codeliststart")?;
        self.code_list_end = hex("codelistend")?;
        let h = |s: &str| u32::from_str_radix(s, 16).unwrap_or(0);
        let pie = Regex::new(r"pokeifequal\(\s*([0-9A-Fa-f]+)\s*,\s*([0-9A-Fa-f]+)\s*,\s*([0-9A-Fa-f]+)\s*,\s*([0-9A-Fa-f]+)\s*\)").unwrap();
        for c in pie.captures_iter(t) {
            self.pokes.push(Poke { addr: h(&c[1]), if_equal: Some(h(&c[2])), write_addr: h(&c[3]), value: h(&c[4]) });
        }
        // poke( not preceded by a lower-case letter (regex has no look-behind: check the byte before)
        let pk = Regex::new(r"poke\(\s*([0-9A-Fa-f]+)\s*,\s*([0-9A-Fa-f]+)\s*\)").unwrap();
        for c in pk.captures_iter(t) {
            let st = c.get(0).unwrap().start();
            if st > 0 && t.as_bytes()[st - 1].is_ascii_lowercase() {
                continue;
            }
            let a = h(&c[1]);
            self.pokes.push(Poke { addr: a, if_equal: None, write_addr: a, value: h(&c[2]) });
        }
        Ok(())
    }

    /// the mod folder exactly as it sits on the SD card, as (source file, disc path under files/) - development leftovers skipped
    pub fn sd_files(&self) -> (Vec<(PathBuf, String)>, usize) {
        const DEV_DIRS: [&str; 4] = ["source", "tools", "documentation", "gctrealmate"];
        const DEV_EXT: [&str; 8] = [".exe", ".dll", ".bat", ".map", ".md", ".bak", ".ini", ".log"];
        let mut o = Vec::new();
        let mut skipped = 0;
        for f in fsutil::files_recursive(&self.dir) {
            let rel = fsutil::rel(&self.dir, &f);
            let top = rel.split_once('/').map(|x| x.0).unwrap_or("");
            let fs_ = f.to_string_lossy();
            if DEV_DIRS.iter().any(|d| eq_ignore_case(d, top)) || DEV_EXT.iter().any(|e| ends_with_ignore_case(&fs_, e)) {
                continue;
            }
            let disc = format!("{}/{}", self.root, rel);
            if disc.chars().any(|c| c as u32 > 0x7E) {
                skipped += 1; // FST names are ASCII; such files are tool leftovers
                continue;
            }
            o.push((f, disc));
        }
        (o, skipped)
    }
}

fn file_in(dir: &Path, name: &str) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    fsutil::files_in(dir).into_iter().find(|f| eq_ignore_case(&fsutil::name(f), name))
}

/// the string the File Patch Code writes to 0x80406920 ("/Minusery/")
fn root_string(l: &[Line]) -> Option<String> {
    for i in 0..l.len() {
        if ((l[i].a >> 24) & 0xFE) != 6 || (0x80000000 | (l[i].a & 0x1FFFFFF)) != 0x80406920 || l[i].b == 0 || l[i].b >= 64 {
            continue;
        }
        let mut sb = String::new();
        for k in 0..l[i].b as usize {
            let Some(ln) = l.get(i + 1 + k / 8) else { break };
            let w = if k % 8 < 4 { ln.a } else { ln.b };
            let c = ((w >> (24 - 8 * (k % 4))) & 0xFF) as u8;
            if c == 0 {
                break;
            }
            sb.push(c as char);
        }
        let n = sb.trim_matches('/').to_string();
        if !n.is_empty() && !n.contains('/') {
            return Some(n);
        }
    }
    None
}

#[allow(dead_code)]
fn _unused(_: HashMap<u8, u8>) {}
