//! BrawlBuilderEx: the window (port of BrawlBuilder.cs). The page in ui/ is the form; this side does the dialogs and runs the build.
//! Comments quoted from the C# BrawlBuilder are kept verbatim.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use bbex_core::exbuild::FitAnswer;
use bbex_core::pipeline::{self, Builder, End, Options, Ui};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult};

const CAPTION: &str = "BrawlBuilderEX";

#[derive(Default)]
struct AppState {
    building: AtomicBool,
    cancel: AtomicBool,
    exiting: AtomicBool,
    /// command line switches of the C# BrawlBuilder (--offset=, --no-gct-patch, ...)
    args: Mutex<Vec<String>>,
}

/// the form's fields
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Form {
    brawl_iso: String,
    mod_folder: String,
    gct_file: String,
    remove_subspace: bool,
    custom_id: bool,
    game_id: String,
    custom_title: bool,
    game_title: String,
    custom_banner: bool,
    banner: String,
}

fn window(app: &AppHandle) -> WebviewWindow {
    app.get_webview_window("main").expect("main window")
}

/// MessageBox.Show(text, title, buttons, icon)
fn message(app: &AppHandle, title: &str, text: &str, kind: MessageDialogKind, buttons: MessageDialogButtons) -> MessageDialogResult {
    app.dialog().message(text).title(title).kind(kind).buttons(buttons).parent(&window(app)).blocking_show_with_result()
}

fn ask(app: &AppHandle, title: &str, text: &str) -> bool {
    matches!(message(app, title, text, MessageDialogKind::Info, MessageDialogButtons::YesNo), MessageDialogResult::Yes)
}

fn error(app: &AppHandle, text: &str) {
    message(app, "Error", text, MessageDialogKind::Error, MessageDialogButtons::Ok);
}

/// where BrawlBuilder lives: Resources/ next to it, ssbb.d and BrawlBuilderEX.noask in it
fn base_path(app: &AppHandle) -> PathBuf {
    let exe = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| PathBuf::from("."));
    let writable = {
        let probe = exe.join(".bbex-write-test");
        let ok = std::fs::write(&probe, b"").is_ok();
        let _ = std::fs::remove_file(&probe);
        ok
    };
    if writable {
        exe
    } else {
        app.path().app_local_data_dir().unwrap_or(exe)
    }
}

fn resources(app: &AppHandle) -> PathBuf {
    let r = bbex_core::cli::resources_dir();
    if r.is_dir() {
        return r;
    }
    app.path().resource_dir().map(|d| d.join("Resources")).unwrap_or(r)
}

fn fit_no_ask_file(app: &AppHandle) -> PathBuf {
    base_path(app).join("BrawlBuilderEX.noask")
}

struct GuiUi<'a> {
    app: &'a AppHandle,
}

impl Ui for GuiUi<'_> {
    fn status(&self, s: &str) {
        let _ = self.app.emit("status", s);
    }
    fn progress(&self, p: Option<f32>) {
        let _ = self.app.emit("progress", p);
    }
    fn ask(&self, title: &str, text: &str) -> bool {
        ask(self.app, title, text)
    }
    /// ask before a lossy step: do it / do it and don't ask again / cancel the build
    fn ask_fit(&self, text: &str) -> FitAnswer {
        let text = format!("The image is too big for a Wii disc.\n\n{text}\n\nWhen the build finishes you'll get a list of everything that was removed.");
        let r = message(
            self.app,
            CAPTION,
            &text,
            MessageDialogKind::Warning,
            MessageDialogButtons::YesNoCancelCustom("Do it".into(), "Do it, and don't ask again".into(), "Cancel build".into()),
        );
        match r {
            MessageDialogResult::Yes => FitAnswer::Yes,
            MessageDialogResult::No => FitAnswer::YesAlways,
            MessageDialogResult::Custom(l) if l == "Do it" => FitAnswer::Yes,
            MessageDialogResult::Custom(l) if l.starts_with("Do it,") => FitAnswer::YesAlways,
            _ => FitAnswer::No,
        }
    }
    fn message(&self, title: &str, text: &str, err: bool) {
        message(self.app, title, text, if err { MessageDialogKind::Error } else { MessageDialogKind::Warning }, MessageDialogButtons::Ok);
    }
    fn cancelled(&self) -> bool {
        let st = self.app.state::<AppState>();
        st.cancel.load(Ordering::SeqCst)
    }
}

// ------------------------------------------------------------------------------------------------------------- browse / help
#[tauri::command]
async fn browse_iso(app: AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .add_filter("Wii Disk Images (*.iso; *.ciso; *.wbfs; *.wbi; *.wia; *.wdf)", &["iso", "ciso", "wbfs", "wbi", "wia", "wdf"])
        .add_filter("All files (*.*)", &["*"])
        .set_parent(&window(&app))
        .blocking_pick_file()
        .map(|p| p.to_string())
}

#[tauri::command]
async fn browse_mod(app: AppHandle) -> Option<String> {
    app.dialog().file().set_parent(&window(&app)).blocking_pick_folder().map(|p| p.to_string())
}

#[tauri::command]
async fn browse_gct(app: AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .add_filter("GCT File (*.gct)", &["gct"])
        .add_filter("All files (*.*)", &["*"])
        .set_parent(&window(&app))
        .blocking_pick_file()
        .map(|p| p.to_string())
}

#[tauri::command]
async fn browse_banner(app: AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .add_filter("Wii Banner Files (*.bnr)", &["bnr"])
        .add_filter("All files (*.*)", &["*"])
        .set_parent(&window(&app))
        .blocking_pick_file()
        .map(|p| p.to_string())
}

#[tauri::command]
async fn help(app: AppHandle, which: String) {
    let text = match which.as_str() {
        "mod" => "Select the folder where all the brawl files to be replaced are located.\nUsually this is named \"pf\".\n\nFor Project M, this folder can be found inside the \"\\projectm\\\" folder.\n\nFor other mods it will most likely be in the \"\\private\\wii\\app\\RSBE\\\" folder",
        "iso" => "Select your unmodded Brawl disk image. This can be an ISO, CISO, WBFS, WBI, WIA, or WDF file\n\nNote: If you have a folder containing all of Brawl's files extracted using wit, then you can also just copy the contents of that folder to an 'ssbb.d' folder in the same directory as BrawlBuilder, and this field will be unnecessary.",
        _ => "Select the GCT file containing the codes for the selected mod. This can usually be found in the \"\\codes\\\" folder and will probably be named  \"RSBE01.gct\"",
    };
    message(&app, "Help", text, MessageDialogKind::Info, MessageDialogButtons::Ok);
}

// ------------------------------------------------------------------------------------------------------------- build
#[tauri::command]
async fn build_click(app: AppHandle, form: Form) {
    let st = app.state::<AppState>();
    if st.building.load(Ordering::SeqCst) {
        // Cancel
        if ask(&app, "Cancel?", "Are you sure you want to cancel the build?") {
            let _ = app.emit("cancelling", "Cancelling...");
            st.cancel.store(true, Ordering::SeqCst);
        }

        return;
    }

    let base = base_path(&app);

    // Verify stuff
    if !Path::new(&form.brawl_iso).is_file() && !base.join("ssbb.d").is_dir() {
        error(&app, "Brawl ISO location is invalid");
        return;
    }

    if form.mod_folder.is_empty() && form.gct_file.is_empty() && !form.remove_subspace && !form.custom_id && !form.custom_title && !form.custom_banner {
        error(&app, "You haven't specified any changes, building not required.");
        return;
    }

    // BrawlBuilderEX: launcher-based mods (Project+, P+Ex, Legacy, Minus, Void, ...) are built self-contained (no SD card / launcher)
    let mut ex = false;
    if !form.mod_folder.is_empty() && pipeline::looks_like_launcher_mod(Path::new(&form.mod_folder)) {
        let r = message(
            &app,
            CAPTION,
            "This looks like a launcher-based mod (it has its own code list and SD card folder).\n\nBuild a self-contained image that needs no SD card, no launcher and no cheat file? (recommended)\n\nThe whole mod folder is put on the disc and the mod's own code list is built in; the GCT field is not used.",
            MessageDialogKind::Info,
            MessageDialogButtons::YesNoCancel,
        );
        match r {
            MessageDialogResult::Yes => ex = true,
            MessageDialogResult::No => {}
            _ => return,
        }
    }

    if !form.mod_folder.is_empty() && !ex {
        if !Path::new(&form.mod_folder).is_dir() {
            error(&app, "Mod folder location is invalid");
            return;
        }

        if !pipeline::looks_like_pf(Path::new(&form.mod_folder))
            && !ask(&app, "Notice", "It doesn't look like the mod folder you selected contains any replacement Brawl files, are you sure you selected the right foler? Usually it will be called 'pf'.\n\nDo you still want to attempt the build using the selected folder?")
        {
            return;
        }
    }

    if !ex && !form.gct_file.is_empty() && !Path::new(&form.gct_file).is_file() {
        error(&app, "GCT file location is invalid");
        return;
    }

    if form.custom_id && !pipeline::valid_game_id(&form.game_id) {
        error(&app, "Game ID must be 6 characters and be made up of some combination of A-Z (uppercase only), 0-9, and underscores. No other characters are allowed.");
        return;
    }

    if form.custom_banner && !Path::new(&form.banner).is_file() {
        error(&app, "Banner file location is invalid");
        return;
    }

    // Ask where to save output
    let id = if form.custom_id { form.game_id.clone() } else { "RSBE01".into() };
    let title = if form.custom_title { form.game_title.clone() } else { "Super Smash Bros. Brawl".into() };
    let Some(save) = app
        .dialog()
        .file()
        .set_title("Select where you wish to save modded Brawl image...")
        .add_filter("ISO Image", &["iso"])
        .add_filter("CISO Image", &["ciso"])
        .add_filter("WBFS Image", &["wbfs"])
        .add_filter("WBI Image", &["wbi"])
        .add_filter("WIA Image", &["wia"])
        .add_filter("WDF Image", &["wdf"])
        .set_file_name(format!("{title} [{id}]"))
        .set_parent(&window(&app))
        .blocking_save_file()
    else {
        return;
    };
    let output = PathBuf::from(save.to_string());

    let args = st.args.lock().unwrap().clone();
    let has = |k: &str| args.iter().any(|a| a == k);
    let options = Options {
        brawl_iso: form.brawl_iso.clone(),
        mod_folder: form.mod_folder.clone(),
        gct_file: form.gct_file.clone(),
        ex,
        remove_subspace: form.remove_subspace,
        game_id: form.custom_id.then(|| form.game_id.clone()),
        game_title: form.custom_title.then(|| form.game_title.clone()),
        banner: form.custom_banner.then(|| form.banner.clone()),
        output: output.clone(),
        split: false,
        ask_split: true,
        resources: resources(&app),
        work: base.clone(),
        offset: args.iter().find_map(|a| a.strip_prefix("--offset=").map(|s| s.to_string())),
        no_gct_patch: has("--no-gct-patch"),
        no_alt_pad: has("--no-alt-pad"),
        notify_gct_patch: has("--notify-gct-patch"),
        fit_no_ask: fit_no_ask_file(&app).is_file(),
        fit: false,
    };

    // Disable form controls
    st.cancel.store(false, Ordering::SeqCst);
    st.building.store(true, Ordering::SeqCst);
    let _ = app.emit("building", true);

    // Start background worker
    let app2 = app.clone();
    std::thread::spawn(move || {
        let ui = GuiUi { app: &app2 };
        let mut b = Builder::new(options, &ui);
        let end = b.run();
        build_completed(&app2, end, &output);
    });
}

/// buildWorker_RunWorkerCompleted
fn build_completed(app: &AppHandle, end: End, output: &Path) {
    let st = app.state::<AppState>();
    if st.exiting.load(Ordering::SeqCst) {
        app.exit(1);
        return;
    }

    // Re-enable controls / Reset status
    st.building.store(false, Ordering::SeqCst);
    st.cancel.store(false, Ordering::SeqCst);
    let _ = app.emit("building", false);
    let _ = window(app).set_focus();

    // Show success if builder actually finished
    if let End::Done(outcome) = end {
        if outcome.fit_no_ask && !fit_no_ask_file(app).is_file() {
            let _ = std::fs::write(fit_no_ask_file(app), "BrawlBuilderEX: do what is needed to fit the disc without asking. Delete this file to be asked again.\r\n");
        }
        if output.is_file() {
            let text = if outcome.removed.is_empty() {
                "Build Completed!".to_string()
            } else {
                format!("Build Completed!\n\nTo fit on a dual-layer Wii disc, BrawlBuilderEX removed:\n\n- {}", outcome.removed.join("\n- "))
            };
            message(app, "Success!", &text, MessageDialogKind::Info, MessageDialogButtons::Ok);
        } else {
            error(app, "BrawlBuilder did not encounter any errors during the build process, but the output image doesn't seem to exist, which indicates that the build did not complete successfully. Sorry about that.");
        }
    }
}

#[tauri::command]
async fn exit_click(app: AppHandle) {
    let st = app.state::<AppState>();
    if st.building.load(Ordering::SeqCst) {
        // Cancel quitting, if user chooses to quit, then the background worker will do it once it cancels.
        if !ask(&app, "Exit", "A build is in progress, quitting will stop the current build. Are you sure you want to quit?") {
            return;
        }

        st.exiting.store(true, Ordering::SeqCst);
        let _ = app.emit("cancelling", "Quitting...");
        st.cancel.store(true, Ordering::SeqCst);
        return;
    }
    app.exit(0);
}

#[cfg(windows)]
fn attach_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(pid: u32) -> i32;
    }
    unsafe {
        AttachConsole(u32::MAX); // ATTACH_PARENT_PROCESS
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // command line EX build (like BrawlBuilder.exe --ex ...)
    if args.iter().any(|a| a == "--ex") {
        #[cfg(windows)]
        attach_console();
        std::process::exit(bbex_core::cli::run(&args));
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { args: Mutex::new(args), ..Default::default() })
        .invoke_handler(tauri::generate_handler![browse_iso, browse_mod, browse_gct, browse_banner, help, build_click, exit_click])
        .run(tauri::generate_context!())
        .expect("error while running BrawlBuilderEx");
}
