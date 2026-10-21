//! File system helpers with .NET-compatible behaviour (recursive enumeration order, case-insensitive ordinal compares).

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

/// entries of one directory in the order the OS returns them (NTFS: name order, like .NET); elsewhere sorted for determinism
fn read_dir(dir: &Path) -> Vec<(PathBuf, bool)> {
    let mut v: Vec<(PathBuf, bool)> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            (e.path(), is_dir)
        }).collect(),
        Err(_) => Vec::new(),
    };
    if !cfg!(windows) {
        v.sort_by(|a, b| cmp_ignore_case(&name(&a.0), &name(&b.0)));
    }
    v
}

pub fn name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// all files below `dir`, breadth-first like .NET's Directory.EnumerateFiles(dir, "*", AllDirectories)
pub fn files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut q = VecDeque::from([dir.to_path_buf()]);
    while let Some(d) = q.pop_front() {
        for (p, is_dir) in read_dir(&d) {
            if is_dir { q.push_back(p) } else { out.push(p) }
        }
    }
    out
}

/// files and directories below `dir` (any order)
pub fn entries_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut q = VecDeque::from([dir.to_path_buf()]);
    while let Some(d) = q.pop_front() {
        for (p, is_dir) in read_dir(&d) {
            if is_dir { q.push_back(p.clone()) }
            out.push(p);
        }
    }
    out
}

/// files directly in `dir`
pub fn files_in(dir: &Path) -> Vec<PathBuf> {
    read_dir(dir).into_iter().filter(|(_, d)| !d).map(|(p, _)| p).collect()
}

/// subdirectories directly in `dir`
pub fn dirs_in(dir: &Path) -> Vec<PathBuf> {
    read_dir(dir).into_iter().filter(|(_, d)| *d).map(|(p, _)| p).collect()
}

/// StringComparer.OrdinalIgnoreCase: compare upper-cased chars
pub fn cmp_ignore_case(a: &str, b: &str) -> Ordering {
    let up = |s: &str| s.chars().flat_map(|c| c.to_uppercase()).collect::<Vec<char>>();
    up(a).cmp(&up(b))
}

pub fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) || cmp_ignore_case(a, b) == Ordering::Equal
}

pub fn starts_with_ignore_case(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && eq_ignore_case(&s[..prefix.len()], prefix)
}

pub fn ends_with_ignore_case(s: &str, suffix: &str) -> bool {
    s.len() >= suffix.len() && s.is_char_boundary(s.len() - suffix.len()) && eq_ignore_case(&s[s.len() - suffix.len()..], suffix)
}

/// path of `p` relative to `base` with '/' separators
pub fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

/// a disc path ("a/b/c") under `root` as a native path
pub fn join_disc(root: &Path, disc: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in disc.split('/') {
        p.push(part);
    }
    p
}

pub fn file_len(p: &Path) -> u64 {
    fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// replace a file (delete first: never writes through a hard link into a shared copy)
pub fn put(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::write(path, data)
}

pub fn full(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    eq_ignore_case(&full(a).to_string_lossy(), &full(b).to_string_lossy())
}
