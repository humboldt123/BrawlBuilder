//! The BrawlBuilder build, step by step (port of BrawlBuilder.WorkerCode.cs): Analyze (GCT patching), Extract, Verify, DeleteSSE,
//! CopyModFiles (or the EX build), CopyBanner, Patch, Build. The front end (GUI or CLI) supplies a `Ui` for status, questions and errors.
//! Comments quoted from the C# BrawlBuilder are kept verbatim.

use std::fs;
use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};

use crate::bin::ExError;
use crate::exbuild::{ExBuild, FitAnswer};
use crate::exmod::ExMod;
use crate::fsutil::{self, eq_ignore_case, join_disc};
use crate::wit;

/// what the front end does for the build
pub trait Ui: Sync {
    /// the text on the Build button ("Extracting...", "Copying...")
    fn status(&self, s: &str);
    /// progress of the current step, 0..=100 (None: no progress shown); the front end blinks it with the status like BrawlBuilder
    fn progress(&self, p: Option<f32>);
    /// a Yes/No question (`title`, `text`); true = Yes
    fn ask(&self, title: &str, text: &str) -> bool;
    /// one of the fit steps (EX): do it / do it and don't ask again / cancel
    fn ask_fit(&self, text: &str) -> FitAnswer;
    /// an error or notice the user has to acknowledge
    fn message(&self, title: &str, text: &str, error: bool);
    /// the user asked to cancel
    fn cancelled(&self) -> bool;
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// the Brawl disc image ("" with an extracted ssbb.d in `work`)
    pub brawl_iso: String,
    pub mod_folder: String,
    pub gct_file: String,
    /// BrawlBuilderEX: build `mod_folder` (a launcher-based mod) self-contained
    pub ex: bool,
    pub remove_subspace: bool,
    pub game_id: Option<String>,
    pub game_title: Option<String>,
    pub banner: Option<String>,
    pub output: PathBuf,
    /// split a .wbfs output into 4GB chunks
    pub split: bool,
    /// ask whether to split a .wbfs output (the GUI)
    pub ask_split: bool,
    /// Resources folder (wit, CodePatches.txt, BrawlFileList.txt, SubspaceEmissaryFiles.txt, patch/)
    pub resources: PathBuf,
    /// where ssbb.d is extracted (BrawlBuilder: the working directory, next to the program)
    pub work: PathBuf,
    /// --offset=X
    pub offset: Option<String>,
    /// --no-gct-patch
    pub no_gct_patch: bool,
    /// --no-alt-pad
    pub no_alt_pad: bool,
    /// --notify-gct-patch
    pub notify_gct_patch: bool,
    /// "everything's okay" for the EX fit steps (remembered by the front end)
    pub fit_no_ask: bool,
    /// command line: allow the EX fit steps without asking
    pub fit: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// what the EX build had to take out to fit the disc (shown when the build finishes)
    pub removed: Vec<String>,
    /// the user chose "don't ask again"
    pub fit_no_ask: bool,
    /// EX report lines (BrawlBuilderEX.log)
    pub report: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Analyze,
    Extract,
    Verify,
    DeleteSse,
    CopyModFiles,
    CopyBanner,
    Patch,
    Build,
    Finish,
}

pub fn failure_message(s: State) -> &'static str {
    match s {
        State::Analyze => "Build failed due to an unknown error while patching the GCT file.",
        State::Verify => "Build failed due to an unknown error while verifiying the Brawl image.",
        State::Extract => "Build failed due to an unknown error while extracting the Brawl image.",
        State::DeleteSse => "Build failed due to an unknown error while deleting Subspace Emissary files.",
        State::CopyModFiles => "Build failed due to an unknown error while copying mod files.",
        State::CopyBanner => "Build failed due to an unknown error while copying the custom banner.",
        State::Patch => "Build failed due to an unknown error while applying the GCT file to the Brawl image.",
        State::Build => "Build failed due to an unkonwn error while building the modded Brawl image.",
        State::Finish => "Build failed due to an unknown error.",
    }
}

// Yes most of these are the same. I realized after I wrote them out it would have probably been faster to just make a few special cases, but w/e
#[rustfmt::skip]
const PAC_REL_MAPPINGS: [(&str, &str); 47] = [
    ("stgdonkey", "st_donkey"),         /* 75m               */     ("stgkart", "st_kart"),             /* Mario Circuit          */
    ("stgbattlefield", "st_battle"),    /* Battlefield       */     ("stgmariopast", "st_mariopast"),   /* Mushroomy Kingdom      */
    ("stgdxbigblue", "st_dxbigblue"),   /* Big Blue          */     ("stgnewpork", "st_newpork"),       /* New Pork City          */
    ("stgoldin", "st_oldin"),           /* Bridge of Eldin   */     ("stgnorfair", "st_norfair"),       /* Norfair                */
    ("stgdxzebes", "st_dxzebes"),       /* Brinstar          */     ("stgdxonett", "st_dxonett"),       /* Onett                  */
    ("stgemblem", "st_emblem"),         /* Castle Siege      */     ("stgonlinetraining", "st_otrain"), /* Online Training        */
    ("stgconfigtest", "st_config"),     /* ConfigTest        */     ("stgpictchat", "st_pictchat"),     /* PictoChat              */
    ("stgdxcorneria", "st_dxcorneria"), /* Corneria          */     ("stgpirates", "st_pirates"),       /* Pirate Ship            */
    ("stgdolpic", "st_dolpic"),         /* Delfino Plaza     */     ("stgdxpstadium", "st_dxpstadium"), /* Pokémon Stadium        */
    ("stgearth", "st_earth"),           /* Distant Planet    */     ("stgstadium", "st_stadium"),       /* Pokémon Stadium 2      */
    ("stgedit", "st_stageedit"),        /* Edit              */     ("stgfzero", "st_fzero"),           /* Port Town Aero Dive    */
    ("stgfinal", "st_final"),           /* Final Destination */     ("stgdxrcruise", "st_dxrcruise"),   /* Rainbow Cruise         */
    ("stggw", "st_gw"),                 /* Flat Zone 2       */     ("stgjungle", "st_jungle"),         /* Rumble Falls           */
    ("stgorpheon", "st_orpheon"),       /* Frigate Orpheon   */     ("stgmetalgear", "st_metalgear"),   /* Shadow Moses Island    */
    ("stgdxgreens", "st_dxgreens"),     /* Green Greens      */     ("stgpalutena", "st_palutena"),     /* Skyworld               */
    ("stggreenhill", "st_greenhill"),   /* Green Hill Zone   */     ("stgvillage", "st_village"),       /* Smashville             */
    ("stghalberd", "st_halberd"),       /* Halberd           */     ("stgtengan", "st_tengan"),         /* Spear Pillar           */
    ("stgplankton", "st_plankton"),     /* Hanenbow          */     ("stgice", "st_ice"),               /* Summit                 */
    ("stgheal", "st_heal"),             /* Heal              */     ("stgtarget", "st_tbreak"),         /* Target Break           */
    ("stghomerun", "st_homerun"),       /* Homerun           */     ("stgdxshrine", "st_dxshrine"),     /* Temple                 */
    ("stgdxgarden", "st_dxgarden"),     /* Jungle Japes      */     ("stgmadein", "st_madein"),         /* WarioWare Inc.         */
    ("stgmansion", "st_mansion"),       /* Luigi's Mansion   */     ("stgcrayon", "st_crayon"),         /* Yoshi's Island (Brawl) */
    ("stgstarfox", "st_starfox"),       /* Lylat Cruise      */     ("stgdxyorster", "st_dxyorster"),   /* Yoshi's Island (Melee) */
    ("stgfamicom", "st_famicom"),       /* Mario Bros.       */
];

/// pacRelMappings[name] (a NameValueCollection: case-insensitive)
fn pac_rel(stage: &str) -> Option<&'static str> {
    PAC_REL_MAPPINGS.iter().find(|(p, _)| eq_ignore_case(p, stage)).map(|(_, r)| *r)
}

fn ci(re: &str) -> Regex {
    RegexBuilder::new(re).case_insensitive(true).build().unwrap()
}

enum StepErr {
    Cancel,
    Fail(String),
}
impl From<ExError> for StepErr {
    fn from(e: ExError) -> Self { StepErr::Fail(e.0) }
}
impl From<std::io::Error> for StepErr {
    fn from(e: std::io::Error) -> Self { StepErr::Fail(e.to_string()) }
}
type Step = std::result::Result<(), StepErr>;

/// how a build ended
pub enum End {
    Done(Outcome),
    /// cancelled by the user (or a question answered No)
    Cancelled,
    /// stopped with an error the user has already been shown
    Failed,
}

pub struct Builder<'u> {
    o: Options,
    ui: &'u dyn Ui,
    wit: PathBuf,
    remove_en: bool,
    temp_gct: PathBuf,
    pub state: State,
}

impl<'u> Builder<'u> {
    pub fn new(o: Options, ui: &'u dyn Ui) -> Self {
        // Set up wit
        let wit = wit::find_wit(&o.resources).unwrap_or_else(|| o.resources.join("wit").join("wit.exe"));
        let temp_gct = o.work.join("temp.gct");
        // Don't remove the _en suffix by default
        Builder { o, ui, wit, remove_en: false, temp_gct, state: State::Analyze }
    }

    fn ssbb(&self) -> PathBuf { self.o.work.join("ssbb.d") }
    fn files(&self) -> PathBuf { self.ssbb().join("files") }

    /// StopWorker: show the message and stop the build
    fn stop(&self, msg: &str) -> StepErr {
        StepErr::Fail(msg.to_string())
    }

    fn check(&self) -> Step {
        if self.ui.cancelled() { Err(StepErr::Cancel) } else { Ok(()) }
    }

    fn delete_brawl_folder(&self) {
        let d = self.ssbb();
        if d.exists() {
            if let Err(e) = fs::remove_dir_all(&d) {
                self.ui.message("Warning", &format!("Unable to delete ssbb.d directory. Deletion failed with error:\n\n{e}"), false);
            }
        }
    }

    /// the whole build (buildWorker_DoWork)
    pub fn run(&mut self) -> End {
        let _ = fs::remove_file(&self.temp_gct); // Make sure any leftover gct gets removed
        let r = self.run_inner();
        // Clean up files
        let _ = fs::remove_file(&self.temp_gct);
        match r {
            Ok(o) => End::Done(o),
            Err(StepErr::Cancel) => End::Cancelled,
            Err(StepErr::Fail(msg)) => {
                self.ui.message("Error", &msg, true);
                End::Failed
            }
        }
    }

    fn run_inner(&mut self) -> std::result::Result<Outcome, StepErr> {
        if !self.wit.is_file() {
            return Err(self.stop("Unable to find wit executable, stopping build..."));
        }
        self.check()?;

        // Check if wit is already running
        if wit::already_running()
            && self.ui.ask("Notice", "One or more instances of wit are already running. This could potentially cause issues with the build. Would you like the program to try and kill all instances of wit before continuing?")
        {
            // Make sure it worked
            if !wit::kill_all() {
                return Err(self.stop("Unable to kill all instances of wit, stopping build..."));
            }
        }
        self.check()?;

        let mut outcome = Outcome::default();
        // Set state to the starting state
        self.state = State::Analyze;
        while self.state != State::Finish {
            match self.state {
                State::Analyze => {
                    if !self.o.ex && !self.o.gct_file.is_empty() && !self.o.no_gct_patch {
                        self.analyze()?;
                    }
                    self.state = State::Extract;
                }
                State::Extract => {
                    self.extract()?;
                    self.state = State::Verify;
                }
                State::Verify => {
                    self.verify()?;
                    self.state = State::DeleteSse;
                }
                State::DeleteSse => {
                    if self.o.remove_subspace {
                        self.delete_sse()?;
                    }
                    self.state = State::CopyModFiles;
                }
                State::CopyModFiles => {
                    if self.o.ex {
                        outcome = self.run_ex()?;
                    } else if !self.o.mod_folder.is_empty() {
                        self.copy_mod_files()?;
                    }
                    self.state = State::CopyBanner;
                }
                State::CopyBanner => {
                    if self.o.banner.is_some() {
                        self.copy_banner()?;
                    }
                    self.state = State::Patch;
                }
                State::Patch => {
                    if !self.o.ex && (!self.o.gct_file.is_empty() || self.o.game_id.is_some()) {
                        self.patch()?;
                    }
                    self.state = State::Build;
                }
                State::Build => {
                    self.build()?;
                    self.state = State::Finish;
                }
                State::Finish => {}
            }
        }

        // Clean up
        self.delete_brawl_folder();
        Ok(outcome)
    }

    fn analyze(&mut self) -> Step {
        self.ui.status("Analyzing...");
        let cp = self.o.resources.join("CodePatches.txt");
        if !cp.is_file() {
            if !self.ui.ask("Notice", "CodePatches.txt not found. Do you want to continue without GCT patching? The output ISO may not work without this.") {
                return Err(StepErr::Cancel);
            }
            return self.check();
        }
        // First we'll read the patches we are going to make
        let actions = ["REMOVE", "PATCH", "TO", "IF", "ENDIF", "REMOVE_EN"];
        let mut action = String::new();
        let mut do_actions = true;

        let mut bytes_string = String::new();
        let mut patch_bytes_string = String::new();

        let mut removes = 0;
        let mut successful_removes = 0;

        let mut patches = 0;
        let mut successful_patches = 0;

        // Load GCT into memory
        let mut gct_bytes = fs::read(&self.o.gct_file)?;
        let hex = |s: &str| -> Vec<u8> { (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect() };

        for s in fs::read_to_string(&cp)?.lines() {
            let t = s.trim();
            let line = t.get(..s.find('#').unwrap_or(s.len())).unwrap_or(t); // Trim whitespace, ignore comments

            if actions.contains(&line) {
                // Handle one-liner statements. These don't affect action
                if line == "ENDIF" {
                    do_actions = true; // 'IF' statement is over, start doing actions again (if they were ever stopped)
                    continue;
                } else if line == "REMOVE_EN" && do_actions {
                    self.remove_en = true;
                    continue;
                }

                action = line.to_string();

                continue;
            }

            if !do_actions {
                continue; // If we aren't doing actions just keep looping until the 'ENDIF'
            }

            // Handle multi-line statements
            match action.as_str() {
                "IF" => {
                    if line.is_empty() {
                        // Blank line means end of codes block, so we can handle the 'IF' statement now.
                        action.clear();

                        if !bytes_string.is_empty() {
                            let bytes = hex(&bytes_string);
                            if search_bytes(&gct_bytes, &bytes) < 1 {
                                do_actions = false; // If code wasn't found, stop doing actions until the 'ENDIF' statement
                            }
                        }

                        bytes_string.clear();
                    } else {
                        bytes_string.push_str(&line.replace(' ', ""));
                    }
                }
                "REMOVE" => {
                    if line.is_empty() {
                        action.clear();

                        if !bytes_string.is_empty() {
                            removes += 1;
                            let bytes = hex(&bytes_string);
                            let index = search_bytes(&gct_bytes, &bytes);
                            if index >= 0 {
                                gct_bytes.drain(index as usize..index as usize + bytes.len());
                                successful_removes += 1;
                            }

                            bytes_string.clear();
                        }
                    } else {
                        bytes_string.push_str(&line.replace(' ', ""));
                    }
                }
                "PATCH" => {
                    if line.is_empty() {
                        action.clear(); // Patch is done after next 'TO'
                    } else {
                        patch_bytes_string.push_str(&line.replace(' ', ""));
                    }
                }
                "TO" => {
                    if line.is_empty() {
                        action.clear();

                        if !bytes_string.is_empty() && !patch_bytes_string.is_empty() {
                            patches += 1;
                            let patch_bytes = hex(&patch_bytes_string);
                            let bytes = hex(&bytes_string);
                            let index = search_bytes(&gct_bytes, &patch_bytes);
                            if index >= 0 {
                                let i = index as usize;
                                gct_bytes.splice(i..i + patch_bytes.len(), bytes);

                                successful_patches += 1;
                            }

                            bytes_string.clear();
                            patch_bytes_string.clear();
                        }
                    } else {
                        bytes_string.push_str(&line.replace(' ', ""));
                    }
                }
                _ => {}
            }
        }

        if self.o.notify_gct_patch {
            self.ui.message(
                "Notice",
                &format!(
                    "Removed codes: {successful_removes}/{removes}\nPatched Codes: {successful_patches}/{patches}\nRemove '_en': {}",
                    if self.remove_en { "True" } else { "False" }
                ),
                false,
            );
        }

        fs::create_dir_all(&self.o.work)?;
        fs::write(&self.temp_gct, &gct_bytes)?;
        self.check()
    }

    fn extract(&mut self) -> Step {
        self.ui.status("Extracting...");

        let mut skip_extraction = false;

        // Check if there is already an extracted Brawl folder
        if self.ssbb().is_dir() && self.o.brawl_iso.is_empty() {
            skip_extraction = true;
        } else if self.ssbb().is_dir() {
            if self.ui.ask("Notice", "There is already an ssbb.d directory. Would you like to skip the extraction and use these files instead?\n\nNote: If you choose 'No', the current ssbb.d directory will be overwritten.") {
                skip_extraction = true;
            } else {
                self.delete_brawl_folder();
            }

            self.check()?;
        }

        if !skip_extraction {
            // Extract brawl to ssbb.d folder
            fs::create_dir_all(&self.o.work)?;
            self.do_wit(vec!["extract".into(), self.o.brawl_iso.clone(), "ssbb.d".into(), "--psel=DATA".into(), "-1ovv".into()], false, false)?;
        }

        if !self.ssbb().is_dir() {
            return Err(self.stop("Extraction failed, stopping build..."));
        }

        if self.ui.cancelled() {
            // Since we have been copying files in, ssbb.d is no longer a clean Brawl. Delete it.
            self.delete_brawl_folder();
            return Err(StepErr::Cancel);
        }

        Ok(())
    }

    fn verify(&mut self) -> Step {
        self.ui.status("Verifying...");

        let list = self.o.resources.join("BrawlFileList.txt");
        if list.is_file() {
            let mut file_list: Vec<(String, u64)> = Vec::new();

            // Parse BrawlFileList.txt
            let mut success = true;
            for s in fs::read_to_string(&list)?.lines() {
                // Each line should have 1 space separating the file path from the file size
                let parts: Vec<&str> = s.split(' ').collect();

                // Try to convert file size string to long
                let Some(size) = parts.get(1).and_then(|x| x.parse::<u64>().ok()) else {
                    if !self.ui.ask("Notice", "Error parsing BrawlFileList.txt, do you want to continue without verifying extracted files?") {
                        return Err(StepErr::Cancel);
                    }

                    success = false;
                    break;
                };

                // Add to list
                file_list.push((parts[0].replace('\\', "/"), size));
            }

            // Verify brawl files
            if success {
                for (file, size) in file_list {
                    let p = join_disc(&self.ssbb(), &file);
                    if !p.is_file() || fsutil::file_len(&p) != size {
                        if !self.ui.ask("Notice", "One or more files are either missing or the wrong size in the extracted Brawl image. Do you still wish to continue?") {
                            return Err(StepErr::Cancel);
                        }

                        break;
                    }
                }
            }
        } else if !self.ui.ask("Notice", "BrawlFileList.txt not found, do you want to continue without verifying extracted files?") {
            return Err(StepErr::Cancel);
        }

        self.check()
    }

    fn delete_sse(&mut self) -> Step {
        let list = self.o.resources.join("SubspaceEmissaryFiles.txt");
        self.ui.status("Deleting SSE...");
        if list.is_file() {
            for file in fs::read_to_string(&list)?.lines() {
                let file = file.trim();
                if !file.is_empty() {
                    let _ = fs::remove_file(join_disc(&self.files(), &file.replace('\\', "/")));
                }
            }
        } else if !self.ui.ask("Notice", "SubspaceEmissaryFiles.txt not found. Do you want to continue the build without removing Subspace Emissary?") {
            return Err(StepErr::Cancel);
        }

        self.check()
    }

    /// a step was cancelled while copying into ssbb.d
    fn cancel_dirty(&self) -> StepErr {
        self.ui.progress(None);
        // Since we have been copying files in, ssbb.d is no longer a clean Brawl. Delete it.
        self.delete_brawl_folder();
        StepErr::Cancel
    }

    fn copy_mod_files(&mut self) -> Step {
        let mod_folder = PathBuf::from(&self.o.mod_folder);
        if !mod_folder.is_dir() {
            // Mod folder was selected, but it has been deleted or moved since the initial check
            return Err(self.stop("Mod folder not found, stopping build..."));
        }
        let files = self.files();

        // If the mod contains stages (actually just alt stages) then we have to do some preparation
        if mod_folder.join("stage").join("melee").is_dir() {
            self.ui.status("Preparing...");

            // The first thing we want to do is get a list of all alt stages included in the mod. This is to resolve the problem described here: https://github.com/mogzol/BrawlBuilder/issues/4#issuecomment-245806404
            // We will use that list to create duplicates of the original .rel files for each alternate stage prior to copying over any files.
            let alt = ci(r"_[A-Z]\.pac$");
            let alt_stages: Vec<PathBuf> = fsutil::files_in(&mod_folder.join("stage").join("melee")).into_iter().filter(|f| alt.is_match(&fsutil::name(f))).collect();

            let alt_stage_matcher = ci(r"^(STG.+?)(?:_.+|LV[0-9])?(_[A-Z]).pac$");
            for stage in alt_stages {
                let stage_name = fsutil::name(&stage);
                let Some(m) = alt_stage_matcher.captures(&stage_name) else { continue };

                let base_name = &m[1];
                let alt_code = &m[2];
                let Some(rel) = pac_rel(base_name) else { continue };
                let rel_path = files.join("module").join(rel);

                let base_rel = rel_path.with_file_name(format!("{rel}.rel"));
                let alt_rel = rel_path.with_file_name(format!("{rel}{alt_code}.rel"));
                if base_rel.is_file() && !alt_rel.exists() {
                    fs::copy(&base_rel, &alt_rel)?;
                }
            }
        }

        self.ui.status("Copying...");

        // Get absolute paths to the mod files
        let mod_files_absolute = fsutil::files_recursive(&mod_folder);

        let progress_max = mod_files_absolute.len().max(1);
        for (progress, absolute_file) in mod_files_absolute.iter().enumerate() {
            // Convert to relative path for easy copying
            let relative_file = format!("/{}", fsutil::rel(&mod_folder, absolute_file));

            // Some files need _en added to the end, check in ssbb.d for that
            let relative_file_en = match relative_file.rfind('.') {
                Some(d) => format!("{}_en{}", &relative_file[..d], &relative_file[d..]),
                None => format!("{relative_file}_en"),
            };
            let dst = join_disc(&files, relative_file.trim_start_matches('/'));
            let dst_en = join_disc(&files, relative_file_en.trim_start_matches('/'));

            // Check brawl files for match with _en, if found then set needs_en to true
            let needs_en = dst_en.is_file();

            // Perform copy
            fs::create_dir_all(dst.parent().unwrap())?; // Just in case it doesn't already exist

            // If remove_en was set in the CodePatches.txt file, then remove _en suffixes
            // (the old file is removed first, so a hard-linked ssbb.d never gets written through)
            if self.remove_en {
                let _ = fs::remove_file(&dst);
                fs::copy(absolute_file, &dst)?;
                let _ = fs::remove_file(&dst_en); // Get rid of those pesky space-wasting _en files, we don't need em here
            } else {
                let target = if needs_en { &dst_en } else { &dst };
                let _ = fs::remove_file(target);
                fs::copy(absolute_file, target)?;
            }

            self.ui.progress(Some((progress + 1) as f32 / progress_max as f32 * 100.0));

            if self.ui.cancelled() {
                return Err(self.cancel_dirty());
            }
        }

        // Stop blinker before continuing
        self.ui.progress(None);

        // Base stage files need to be padded to be the same size as their largest alt stage
        // If there aren't alt stages this will do nothing
        if !self.o.no_alt_pad {
            let stages = fsutil::files_in(&files.join("stage").join("melee"));

            self.ui.status("Padding...");
            let alt = ci(r"_[A-Z]\.pac$");

            for (progress, stage) in stages.iter().enumerate() {
                let name = fsutil::name(stage);
                if !alt.is_match(&name) {
                    // If not an alt stage
                    let filename = name.rsplit_once('.').map(|x| x.0).unwrap_or(&name).to_string();

                    // Select all the alt stages for this stage
                    let alt_of = ci(&format!(r"{}_[A-Z]\.pac$", regex::escape(&filename)));

                    // Determine the largest one (resharper converted my foreach loop. LINQ is cool.)
                    let largest = stages.iter().filter(|s| alt_of.is_match(&fsutil::name(s))).map(|s| fsutil::file_len(s)).max().unwrap_or(0);

                    // If base stage is smaller, add padding to match largest alt stage
                    let base_stage_size = fsutil::file_len(stage);
                    if base_stage_size < largest {
                        let padding = largest - base_stage_size;

                        let mut stream = fs::OpenOptions::new().append(true).open(stage)?;
                        std::io::Write::write_all(&mut stream, &vec![0u8; padding as usize])?;
                    }
                }

                self.ui.progress(Some((progress + 1) as f32 / stages.len().max(1) as f32 * 100.0));

                if self.ui.cancelled() {
                    return Err(self.cancel_dirty());
                }
            }

            // Stop blinker before continuing
            self.ui.progress(None);
        }

        // Finally we need to clean up any extra uneeded alternate stage .rel files (since we made copies of the original for every alt stage)
        // The way we will do this is delete any alternate .rel files that are identical to the base one (since the mod will fall back to the base one anyway)
        let module = files.join("module");
        let alt_rel = ci(r"_[A-Z]\.rel$");
        let alt_rels: Vec<PathBuf> = fsutil::files_in(&module).into_iter().filter(|f| alt_rel.is_match(&fsutil::name(f))).collect();

        if !alt_rels.is_empty() {
            self.ui.status("Cleaning Up...");

            for (progress, alt_rel) in alt_rels.iter().enumerate() {
                let name = fsutil::name(alt_rel);
                let filename = &name[..name.len() - 4];
                let base_rel = module.join(format!("{}.rel", &filename[..filename.len() - 2])); // Remove the _[A-Z] part to get the base rel name

                if base_rel.is_file() {
                    // First check if files are the same size. If yes, then check if they are identical.
                    if fsutil::file_len(&base_rel) == fsutil::file_len(alt_rel) && fs::read(&base_rel)? == fs::read(alt_rel)? {
                        fs::remove_file(alt_rel)?;
                    }
                }

                self.ui.progress(Some((progress + 1) as f32 / alt_rels.len() as f32 * 100.0));

                if self.ui.cancelled() {
                    return Err(self.cancel_dirty());
                }
            }

            // Stop blinker before continuing
            self.ui.progress(None);
        }

        Ok(())
    }

    fn run_ex(&mut self) -> std::result::Result<Outcome, StepErr> {
        self.ui.status("Building EX...");
        let work = self.o.work.join("ex.work");
        let ui = self.ui;
        let Some(m) = ExMod::resolve(Path::new(&self.o.mod_folder), &work, &|s| ui.status(s))? else {
            return Err(self.stop("No launcher-based mod (RSBE01.GCT + gc.txt / gameconfig.txt, or an sd.raw) was found in the mod folder."));
        };
        let mut b = ExBuild {
            log: Box::new(move |s: &str| ui.status(&if s.chars().count() > 40 { format!("{}...", s.chars().take(40).collect::<String>()) } else { s.to_string() })),
            remove_subspace: self.o.remove_subspace,
            fit: self.o.fit,
            fit_no_ask: self.o.fit_no_ask,
            ask_fit: if self.o.fit { None } else { Some(Box::new(move |msg: &str| ui.ask_fit(msg))) },
            subspace_list: Some(self.o.resources.join("SubspaceEmissaryFiles.txt")),
            ..Default::default()
        };
        let r = b.run(&self.ssbb(), &m, self.o.game_id.as_deref());
        let _ = fs::write(self.o.work.join("BrawlBuilderEX.log"), b.report.join("\r\n") + "\r\n");
        if work.is_dir() {
            let _ = fs::remove_dir_all(&work);
        }
        match r {
            Ok(()) => {
                self.check()?;
                Ok(Outcome { removed: b.removed, fit_no_ask: b.fit_no_ask, report: b.report })
            }
            Err(e) if e.0.starts_with("cancelled:") => Err(StepErr::Cancel),
            Err(e) => Err(StepErr::Fail(format!("EX build failed: {}", e.0))),
        }
    }

    fn copy_banner(&mut self) -> Step {
        let banner = self.o.banner.clone().unwrap_or_default();
        if Path::new(&banner).is_file() {
            fs::copy(&banner, self.files().join("opening.bnr"))?;
        } else if !self.ui.ask("Notice", "Custom banner file not found, do you want to continue the build without a custom banner?") {
            return Err(StepErr::Cancel);
        }

        if self.ui.cancelled() {
            return Err(self.cancel_dirty());
        }

        Ok(())
    }

    fn patch(&mut self) -> Step {
        let mut patch_args: Vec<String> = Vec::new();
        let gct = if self.temp_gct.is_file() { self.temp_gct.clone() } else { PathBuf::from(&self.o.gct_file) };

        if gct.is_file() {
            // Determine GCT file size
            let gct_size = fsutil::file_len(&gct);

            // Set up offset
            let offset = self.o.offset.clone().unwrap_or_else(|| "80570000".into());

            let patch = self.o.resources.join("patch");
            patch_args.push("NEW=TEXT,80001800,10C0".into());
            patch_args.push(format!("LOAD=80001800,{}", patch.join("codehandler.bin").display()));
            patch_args.push(format!("XML={}", patch.join("PatchCommon.xml").display()));
            patch_args.push(format!("NEW=DATA,{offset},{gct_size:X}"));
            patch_args.push(format!("LOAD={offset},{}", gct.display()));
        } else if !self.o.gct_file.is_empty() {
            // GCT file was selected, but it has been deleted or moved since the initial check
            return Err(self.stop("GCT file not found, stopping build..."));
        }

        if let Some(id) = &self.o.game_id {
            // Need to patch dol to avoid the Please insert disc screen
            let (first, last) = id.split_at(4);

            if first != "RSBE" {
                // Convert to hex
                patch_args.push(format!("805A14B0={}", first.bytes().map(|b| format!("{b:02X}")).collect::<String>()));
            }

            if last != "01" {
                // Convert to hex
                patch_args.push(format!("805A14B8={}", last.bytes().map(|b| format!("{b:02X}")).collect::<String>()));
            }
        }

        if !patch_args.is_empty() {
            // Apply patch
            self.ui.status("Patching...");

            let mut args = vec!["dolpatch".to_string(), "ssbb.d/sys/main.dol".to_string()];
            args.extend(patch_args);
            self.do_wit(args, true, false)?;
        }

        if self.ui.cancelled() {
            return Err(self.cancel_dirty());
        }

        Ok(())
    }

    fn build(&mut self) -> Step {
        self.ui.status("Building...");

        let mut split_output = self.o.split;
        if !split_output && fsutil::ends_with_ignore_case(&self.o.output.to_string_lossy(), ".wbfs") && self.o.ask_split {
            split_output = self.ui.ask("Split output?", "You selected a wbfs output. Do you want to split the output into 4GB chunks for use on FAT32 filesystems? (Assuming the output is greater than 4GB)");
        }

        let mut args = vec!["copy".to_string(), "ssbb.d".to_string(), self.o.output.to_string_lossy().into_owned(), if split_output { "-ovvz".into() } else { "-ovv".into() }];
        if let Some(id) = &self.o.game_id {
            args.push(format!("--id={id}"));
        }
        if let Some(t) = &self.o.game_title {
            args.push("--name".into());
            args.push(t.clone());
        }

        self.do_wit(args, false, true)
    }

    fn do_wit(&self, args: Vec<String>, force_no_progress: bool, finalize: bool) -> Step {
        let ui = self.ui;
        let mut finalize = finalize;
        if !force_no_progress {
            ui.progress(Some(0.0));
        }
        let res = wit::run(&self.wit, &self.o.work, &args, &mut |p| {
            if force_no_progress {
                return;
            }
            ui.progress(Some(p as f32));

            // During build 99% can stay up for a loooong time depending on drive speed, so switch status to Finalizing...
            if p >= 99 && finalize {
                ui.progress(None);
                ui.status("Finalizing...");
                finalize = false; // No need to keep setting it, once is enough
            }
        }, &|| ui.cancelled())?;

        // Stop blinker before continuing
        ui.progress(None);

        let Some(res) = res else {
            // Didn't finish working, delete ssbb.d
            self.delete_brawl_folder();
            return Err(StepErr::Cancel);
        };

        // Check wit exit code
        let error = res.stderr.trim_end();
        if res.code != 0 {
            return Err(StepErr::Fail(format!(
                "Wit closed unexpectedly with exit code {}, stopping build...{}",
                res.code,
                if error.is_empty() { String::new() } else { format!("\n\nWit error messages:\n\n{error}") }
            )));
        }
        // If there were errors, but exit code was fine, notify the user, but let them continue
        else if !error.is_empty()
            && !ui.ask("Notice", &format!("Wit didn't exit with an error code, however it did write to the error output with the following:\n\n{error}\n\nDo you still want to continue the build?"))
        {
            return Err(StepErr::Cancel);
        }

        if ui.cancelled() {
            // Didn't finish working, delete ssbb.d
            self.delete_brawl_folder();
            return Err(StepErr::Cancel);
        }

        Ok(())
    }
}

// http://stackoverflow.com/a/26880541/1687909
fn search_bytes(haystack: &[u8], needle: &[u8]) -> i64 {
    let len = needle.len();
    if len > haystack.len() {
        return -1;
    }
    let limit = haystack.len() - len;
    for i in 0..=limit {
        if haystack[i..i + len] == *needle {
            return i as i64;
        }
    }
    -1
}

/// a launcher-based mod (its own code list + SD folder, or an sd.raw image), up to two folders above `path`
pub fn looks_like_launcher_mod(path: &Path) -> bool {
    if path.is_file() {
        return fsutil::ends_with_ignore_case(&path.to_string_lossy(), ".raw");
    }
    let mut p = Some(fsutil::full(path));
    for _ in 0..3 {
        let Some(d) = p.clone() else { break };
        if !d.is_dir() {
            break;
        }
        if fsutil::files_in(&d).iter().any(|f| fsutil::ends_with_ignore_case(&f.to_string_lossy(), ".raw")) {
            return true;
        }
        if fsutil::files_recursive(&d).iter().any(|f| eq_ignore_case(&fsutil::name(f), "RSBE01.GCT")) {
            return true;
        }
        p = d.parent().map(|x| x.to_path_buf());
    }
    false
}

/// a folder that holds replacement Brawl files (the classic "pf" check)
pub fn looks_like_pf(path: &Path) -> bool {
    const BRAWL_FOLDERS: [&str; 17] = ["effect", "fighter", "game", "info", "info2", "item", "item_gen", "menu", "menu2", "minigame", "module", "movie", "net", "sound", "stage", "system", "toy"];
    fsutil::dirs_in(path).iter().any(|d| BRAWL_FOLDERS.contains(&fsutil::name(d).as_str()))
}

/// ^[A-Z0-9_]{6}$
pub fn valid_game_id(id: &str) -> bool {
    id.len() == 6 && id.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}
