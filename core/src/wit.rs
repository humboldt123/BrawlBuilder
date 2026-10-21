//! Running Wiimm's wit (extract / dolpatch / copy) with progress, like BrawlBuilder's DoWit.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use regex::Regex;

use crate::bin::{ExError, Result};

/// wit next to the app (Resources/wit/wit[.exe]), else on PATH
pub fn find_wit(resources: &Path) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "wit.exe" } else { "wit" };
    let local = resources.join("wit").join(exe);
    if local.is_file() {
        return Some(local);
    }
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|p| p.join(exe)).find(|p| p.is_file()))
}

/// whether a wit process is already running (Windows only; elsewhere assumed not)
pub fn already_running() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Ok(o) = Command::new("tasklist").args(["/FI", "IMAGENAME eq wit.exe", "/NH"]).creation_flags(0x08000000).output() {
            return String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains("wit.exe");
        }
    }
    false
}

/// kill every running wit (Windows only)
pub fn kill_all() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill").args(["/F", "/IM", "wit.exe"]).creation_flags(0x08000000).output();
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    !already_running()
}

pub struct WitResult {
    pub code: i32,
    pub stderr: String,
}

/// run wit in `cwd`; `progress` gets every "NN%" wit prints; `cancelled` is polled and kills wit when it turns true
pub fn run(wit: &Path, cwd: &Path, args: &[String], progress: &mut dyn FnMut(u32), cancelled: &dyn Fn() -> bool) -> Result<Option<WitResult>> {
    let mut cmd = Command::new(wit);
    cmd.args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|e| ExError(format!("could not start wit ({}): {e}", wit.display())))?;
    let stderr = Arc::new(Mutex::new(String::new()));
    let err_pipe = child.stderr.take().unwrap();
    let err_buf = stderr.clone();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = BufReader::new(err_pipe).read_to_string(&mut s);
        *err_buf.lock().unwrap() = s;
    });
    let out = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        // wit updates its progress line with '\r'
        let mut r = BufReader::new(out);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\r', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    for part in String::from_utf8_lossy(&buf).split('\n') {
                        let _ = tx.send(part.to_string());
                    }
                }
            }
        }
    });
    let pct = Regex::new(r"(\d+)%").unwrap();
    loop {
        while let Ok(line) = rx.try_recv() {
            if let Some(c) = pct.captures(&line) {
                if let Ok(p) = c[1].parse() {
                    progress(p);
                }
            }
        }
        if cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        if let Some(status) = child.try_wait()? {
            while let Ok(line) = rx.try_recv() {
                if let Some(c) = pct.captures(&line) {
                    if let Ok(p) = c[1].parse() {
                        progress(p);
                    }
                }
            }
            let _ = err_thread.join();
            let s = stderr.lock().unwrap().clone();
            return Ok(Some(WitResult { code: status.code().unwrap_or(-1), stderr: s }));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
