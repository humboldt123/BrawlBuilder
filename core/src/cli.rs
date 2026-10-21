//! Command line front end (port of ExCli.cs):
//!   --ex --iso <Brawl USA Rev 2 .wbfs/.iso> --mod <mod download folder | sd.raw> --out <out.wbfs|.iso> [--id RSBExx] [--name "Title"]
//!        [--wit wit.exe] [--work dir] [--keep-work] [--fit] [--remove-sse]
//! --tree <dir> patches an already extracted tree in place (no extraction / no disc unless --out is given).
//! Diagnostic options: --no-bake, --mem1-index, --keep-shadowed, --no-free, --analyze.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::bin::{require, Result};
use crate::exbuild::ExBuild;
use crate::exmod::ExMod;
use crate::fsutil;

/// the folder with wit, CodePatches.txt, ... next to the program
pub fn resources_dir() -> PathBuf {
    let exe = std::env::current_exe().ok();
    let base = exe.as_ref().and_then(|e| e.parent()).map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    for c in [base.join("Resources"), base.join("resources"), base.join("..").join("Resources")] {
        // (a macOS bundle's Contents/Resources is ../Resources too: ours is the one with CodePatches.txt)
        if c.join("CodePatches.txt").is_file() {
            return c;
        }
    }
    base.join("Resources")
}

pub fn run(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--pipeline") {
        return run_pipeline(args);
    }
    match run_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ERROR: {e}");
            2
        }
    }
}

fn run_inner(args: &[String]) -> Result<()> {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let has = |k: &str| args.iter().any(|a| a == k);
    let (iso, tree, module, outp, id, name) = (arg("--iso"), arg("--tree"), arg("--mod"), arg("--out"), arg("--id"), arg("--name"));
    let res = resources_dir();
    let wit = arg("--wit").map(PathBuf::from).or_else(|| crate::wit::find_wit(&res)).unwrap_or_else(|| res.join("wit").join("wit.exe"));
    let work = arg("--work").map(PathBuf::from).unwrap_or_else(|| {
        let p = fsutil::full(Path::new(outp.as_deref().or(tree.as_deref()).unwrap_or(".")));
        p.parent().map(|x| x.to_path_buf()).unwrap_or_default().join("BrawlBuilderEX.work")
    });
    require(
        module.is_some() && (iso.is_some() || tree.is_some() || has("--analyze")),
        "usage: --ex --iso <brawl> --mod <mod folder or sd.raw> --out <file> [--id RSBExx] [--name text]",
    )?;
    fs::create_dir_all(&work)?;
    let module = module.unwrap();
    let m = ExMod::resolve(Path::new(&module), &work, &|s| println!("{s}"))?;
    require(m.is_some(), format!("no launcher-based mod (RSBE01.GCT + gc.txt/gameconfig.txt, or an sd.raw) found at {module}"))?;
    let m = m.unwrap();
    println!(
        "mod folder: {}  (SD root /{}, code list {} @0x{:08X}, code handler from {})",
        m.dir.display(),
        m.root,
        fsutil::name(&m.gct_path),
        m.code_list_start,
        fsutil::name(&m.handler_source)
    );
    if has("--analyze") {
        return Ok(());
    }
    let tree_dir = match &tree {
        Some(t) => PathBuf::from(t),
        None => {
            let t = work.join("ssbb.d");
            if t.is_dir() {
                fs::remove_dir_all(&t)?;
            }
            println!("extracting Brawl ...");
            let mut a = vec!["extract".to_string(), iso.clone().unwrap(), t.to_string_lossy().into_owned(), "--psel=DATA".into(), "-1ovv".into()];
            require(run_wit(&wit, &mut a)? == 0, "wit extract failed")?;
            t
        }
    };
    let mut b = ExBuild {
        log: Box::new(|s: &str| println!("{s}")),
        remove_subspace: has("--remove-sse"),
        bake: !has("--no-bake"),
        fit: has("--fit"),
        mem1_index: has("--mem1-index"),
        free_disc_space: !has("--no-free"),
        keep_shadowed: has("--keep-shadowed"),
        subspace_list: Some(res.join("SubspaceEmissaryFiles.txt")),
        ..Default::default()
    };
    b.run(&tree_dir, &m, id.as_deref())?;
    for r in &b.report {
        println!("  {r}");
    }
    if let Some(outp) = outp {
        let _ = fs::remove_file(&outp);
        println!("building {outp} ...");
        let mut a = vec!["copy".to_string(), tree_dir.to_string_lossy().into_owned(), outp.clone(), "-ovv".into()];
        if let Some(id) = &id {
            a.push(format!("--id={id}"));
        }
        if let Some(n) = &name {
            a.push("--name".into());
            a.push(n.clone());
        }
        require(run_wit(&wit, &mut a)? == 0, "wit copy failed")?;
        if !has("--keep-work") && tree.is_none() {
            fs::remove_dir_all(&work)?;
        }
    }
    println!("done");
    Ok(())
}

fn run_wit(exe: &Path, args: &mut Vec<String>) -> Result<i32> {
    let st = Command::new(exe).args(args.iter()).status()?;
    Ok(st.code().unwrap_or(-1))
}

/// answers every question with yes and prints what the GUI would show
struct ConsoleUi;

impl crate::pipeline::Ui for ConsoleUi {
    fn status(&self, s: &str) {
        println!("[status] {s}");
    }
    fn progress(&self, _p: Option<f32>) {}
    fn ask(&self, title: &str, text: &str) -> bool {
        println!("[ask: {title}] {} -> yes", text.replace('\n', " "));
        true
    }
    fn ask_fit(&self, text: &str) -> crate::exbuild::FitAnswer {
        println!("[ask fit] {} -> do it", text.replace('\n', " "));
        crate::exbuild::FitAnswer::Yes
    }
    fn message(&self, title: &str, text: &str, _error: bool) {
        println!("[{title}] {text}");
    }
    fn cancelled(&self) -> bool {
        false
    }
}

/// --pipeline: the GUI's build (classic BrawlBuilder steps or EX) without the window, every question answered yes
///   --iso <brawl image> [--mod <folder>] [--gct <file>] --out <image> [--id X] [--name T] [--banner f] [--remove-sse] [--work dir]
fn run_pipeline(args: &[String]) -> i32 {
    use crate::pipeline::{looks_like_launcher_mod, Builder, End, Options};
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let has = |k: &str| args.iter().any(|a| a == k);
    let module = arg("--mod").unwrap_or_default();
    let ex = !module.is_empty() && looks_like_launcher_mod(Path::new(&module));
    let out = PathBuf::from(arg("--out").unwrap_or_else(|| "out.wbfs".into()));
    let o = Options {
        brawl_iso: arg("--iso").unwrap_or_default(),
        mod_folder: module,
        gct_file: arg("--gct").unwrap_or_default(),
        ex,
        remove_subspace: has("--remove-sse"),
        game_id: arg("--id"),
        game_title: arg("--name"),
        banner: arg("--banner"),
        output: out.clone(),
        resources: resources_dir(),
        work: arg("--work").map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
        ..Default::default()
    };
    println!("pipeline: {} build", if ex { "EX" } else { "classic" });
    let ui = ConsoleUi;
    let mut b = Builder::new(o, &ui);
    match b.run() {
        End::Done(outcome) => {
            for r in &outcome.removed {
                println!("removed: {r}");
            }
            println!("done: {} ({} bytes)", out.display(), fsutil::file_len(&out));
            if out.is_file() { 0 } else { 3 }
        }
        End::Cancelled => {
            println!("cancelled in {:?}", b.state);
            4
        }
        End::Failed => {
            println!("failed in {:?}", b.state);
            5
        }
    }
}
