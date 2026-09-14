//! Updates from the GitHub Releases of the repo the app is built from.
//!
//! A few seconds after startup a background thread asks GitHub which release
//! is the latest. It reads the tag from the redirect that
//! github.com/<repo>/releases/latest answers with, so there's no JSON to parse
//! and no API rate limit to run into. If that tag is newer than this build,
//! the toolbar offers the update.
//!
//! Installing downloads the release's exe and swaps it in for this one.
//! Windows won't let a running exe be overwritten or deleted, but it can be
//! renamed, so this one moves aside to `pdf-annotate.<pid>.old` and the new one
//! takes its name. This process carries on as it was, and the next start runs
//! the new version. Moved-aside exes are deleted at a later start, once nothing
//! runs them any more.
//!
//! Debug builds don't check, so `cargo run` never swaps out target\debug's exe.
//! `PDF_ANNOTATE_UPDATE=0` turns checking off, and any other value turns it on,
//! debug builds included.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;

use crate::worker::trace;

/// Where releases are published. .github/workflows/release.yml attaches
/// `ASSET` to each one, tagged `v` and the version in Cargo.toml.
const REPO: &str = "c0deZ3R0/pdf-annotate";
const ASSET: &str = "pdf-annotate.exe";

/// The version of this build, from Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How long after startup to check, so the check never competes with opening
/// a file.
const CHECK_DELAY: Duration = Duration::from_secs(3);

/// Far larger than any build; a download past it is refused.
const MOST_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub enum State {
    /// Not checked yet, checking, or nothing newer.
    Current,
    /// A release with this tag is newer than the running build.
    Available(String),
    Downloading(String),
    /// Swapped in; it runs from the next start.
    Ready(String),
    Failed(String),
}

pub struct Updater {
    state: Arc<Mutex<State>>,
    ctx: egui::Context,
}

impl Updater {
    /// Clears up after earlier updates and, unless turned off, checks for a
    /// newer release in the background.
    pub fn start(ctx: egui::Context) -> Self {
        let state = Arc::new(Mutex::new(State::Current));
        let updater = Updater { state: Arc::clone(&state), ctx: ctx.clone() };
        let check = move || {
            remove_leftovers();
            if !enabled() {
                return;
            }
            std::thread::sleep(CHECK_DELAY);
            match latest_tag(REPO) {
                Ok(tag) if is_newer(&tag, VERSION) => {
                    *state.lock().unwrap() = State::Available(tag);
                    ctx.request_repaint();
                }
                Ok(tag) => trace(format_args!("update: {tag} is the latest, and this is {VERSION}")),
                // Offline, or no release yet: nothing worth telling anyone.
                Err(e) => trace(format_args!("update: could not check: {e}")),
            }
        };
        if let Err(e) = std::thread::Builder::new().name("update check".into()).spawn(check) {
            trace(format_args!("update: could not start the check: {e}"));
        }
        updater
    }

    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// Downloads the release on offer and swaps it in, in the background.
    pub fn install(&self) {
        let State::Available(tag) = self.state() else { return };
        self.set(State::Downloading(tag.clone()));
        let (state, ctx) = (Arc::clone(&self.state), self.ctx.clone());
        std::thread::spawn(move || {
            let next = match download(&tag).and_then(|bytes| swap_in(&bytes)) {
                Ok(()) => State::Ready(tag),
                Err(e) => State::Failed(e),
            };
            *state.lock().unwrap() = next;
            ctx.request_repaint();
        });
    }

    pub fn fail(&self, message: String) {
        self.set(State::Failed(message));
    }

    fn set(&self, state: State) {
        *self.state.lock().unwrap() = state;
        self.ctx.request_repaint();
    }
}

fn enabled() -> bool {
    match std::env::var_os("PDF_ANNOTATE_UPDATE") {
        Some(value) => value != "0",
        None => !cfg!(debug_assertions),
    }
}

fn agent(max_redirects: u32, timeout: Duration) -> ureq::Agent {
    ureq::config::Config::builder()
        .user_agent(format!("pdf-annotate/{VERSION}"))
        .https_only(true)
        .max_redirects(max_redirects)
        .timeout_global(Some(timeout))
        .build()
        .new_agent()
}

/// The tag of `repo`'s latest release, from where GitHub redirects its page.
fn latest_tag(repo: &str) -> Result<String, String> {
    let url = format!("https://github.com/{repo}/releases/latest");
    let response = agent(0, Duration::from_secs(20)).get(&url).call().map_err(|e| e.to_string())?;
    let location = response.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or_default();
    tag_from_location(location)
        .map(str::to_owned)
        .ok_or_else(|| format!("no release found ({}, redirected to {location:?})", response.status()))
}

/// `v1.2.3` from `https://github.com/<repo>/releases/tag/v1.2.3`. With no
/// releases, GitHub redirects to the releases list instead, which has no tag.
fn tag_from_location(location: &str) -> Option<&str> {
    let tag = location.rsplit_once("/releases/tag/")?.1;
    (!tag.is_empty() && !tag.contains('/')).then_some(tag)
}

/// Whether release `tag` is a later version than `current`.
fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse_version(tag), parse_version(current)), (Some(tag), Some(current)) if tag > current)
}

/// `1.2.3` or `v1.2.3`, missing parts counting as 0. Anything after `-` or `+`
/// is ignored; GitHub's latest release is never a pre-release anyway.
fn parse_version(version: &str) -> Option<[u64; 3]> {
    let core = version.strip_prefix('v').unwrap_or(version).split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let version = [parts.next()??, parts.next().unwrap_or(Some(0))?, parts.next().unwrap_or(Some(0))?];
    parts.next().is_none().then_some(version)
}

fn download(tag: &str) -> Result<Vec<u8>, String> {
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/{ASSET}");
    // GitHub redirects the download to its file storage.
    let mut response = agent(10, Duration::from_secs(600)).get(&url).call().map_err(|e| format!("Download failed: {e}"))?;
    let bytes = response.body_mut().with_config().limit(MOST_BYTES).read_to_vec().map_err(|e| format!("Download failed: {e}"))?;
    // Every Windows program starts with these two bytes.
    if !bytes.starts_with(b"MZ") {
        return Err("The download isn't a Windows program.".into());
    }
    Ok(bytes)
}

/// Where this process moved its own exe to, once it has swapped in an update:
/// (the exe's own path, where it went).
static MOVED: Mutex<Option<(PathBuf, PathBuf)>> = Mutex::new(None);

/// The exe to start for work handed to copies of the app (pool.rs). Once an
/// update is swapped in, `exe`'s path holds the new version, whose helpers
/// needn't speak this version's protocol, so this process starts its own.
pub fn running_exe(exe: &Path) -> PathBuf {
    match &*MOVED.lock().unwrap() {
        Some((from, to)) if from == exe => to.clone(),
        _ => exe.to_path_buf(),
    }
}

fn swap_in(bytes: &[u8]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = exe.with_extension("new");
    let old = exe.with_extension(format!("{}.old", std::process::id()));
    std::fs::write(&new, bytes).map_err(|e| format!("Could not write the new version next to {}: {e}", exe.display()))?;
    // Held through both renames, so nothing is started from the path while
    // it's empty; see `running_exe`.
    let mut moved = MOVED.lock().unwrap();
    if let Err(e) = std::fs::rename(&exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("Could not move the running app aside: {e}"));
    }
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        let _ = std::fs::remove_file(&new);
        return Err(format!("Could not put the new version in place: {e}"));
    }
    *moved = Some((exe, old));
    Ok(())
}

/// Starts the app again from the exe an update put in place, opening `file`.
pub fn relaunch(file: Option<&Path>) -> Result<(), String> {
    let exe = match &*MOVED.lock().unwrap() {
        Some((from, _)) => from.clone(),
        None => std::env::current_exe().map_err(|e| e.to_string())?,
    };
    std::process::Command::new(exe)
        .args(file)
        .spawn()
        .map(drop)
        .map_err(|e| format!("Could not start the new version: {e}"))
}

/// Deletes exes that earlier updates moved aside, and a download that was never
/// swapped in. One that is still running can't be deleted; it's left for a
/// later start.
fn remove_leftovers() {
    let Ok(exe) = std::env::current_exe() else { return };
    let (Some(dir), Some(stem)) = (exe.parent(), exe.file_stem().and_then(|s| s.to_str())) else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_str().is_some_and(|name| is_leftover(name, stem)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn is_leftover(name: &str, stem: &str) -> bool {
    let Some(rest) = name.strip_prefix(stem).and_then(|rest| rest.strip_prefix('.')) else { return false };
    rest == "new" || rest.strip_suffix(".old").is_some_and(|pid| pid.parse::<u32>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.3"), "not as text");
        assert!(is_newer("v1", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.2.0"));
        assert!(!is_newer("nightly", "0.1.0"), "a tag that isn't a version is never offered");
        assert_eq!(parse_version("v1.2.3-beta.1"), Some([1, 2, 3]));
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn the_tag_comes_from_the_redirect() {
        let location = "https://github.com/c0deZ3R0/pdf-annotate/releases/tag/v0.2.0";
        assert_eq!(tag_from_location(location), Some("v0.2.0"));
        assert_eq!(tag_from_location("https://github.com/c0deZ3R0/pdf-annotate/releases"), None);
        assert_eq!(tag_from_location(""), None);
    }

    /// Against a real repo with releases: `cargo test --lib update:: -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn reads_the_latest_tag_from_github() {
        let tag = latest_tag("BurntSushi/ripgrep").unwrap();
        assert!(parse_version(&tag).is_some(), "{tag}");
        assert!(is_newer(&tag, "0.0.1"), "{tag}");
    }

    #[test]
    fn only_update_files_are_leftovers() {
        assert!(is_leftover("pdf-annotate.new", "pdf-annotate"));
        assert!(is_leftover("pdf-annotate.1234.old", "pdf-annotate"));
        assert!(!is_leftover("pdf-annotate.exe", "pdf-annotate"));
        assert!(!is_leftover("pdf-annotate.pdb", "pdf-annotate"));
        assert!(!is_leftover("pdf-annotate.notes.old", "pdf-annotate"));
        assert!(!is_leftover("bench.1234.old", "pdf-annotate"));
    }
}
