//! Updates: from the GitHub Releases of the repo the app is built from, or,
//! in the Store build, from the Microsoft Store (see `store`).
//!
//! A few seconds after startup a background thread asks GitHub which release
//! is the latest. It reads the tag from the redirect that
//! github.com/<repo>/releases/latest answers with, so there's no JSON to parse
//! and no API rate limit to run into. If that tag is newer than this build,
//! the toolbar offers the update. The question is asked again every few hours,
//! for a window left open for days.
//!
//! Installing downloads the release's exe and swaps it in for this one.
//! Windows won't let a running exe be overwritten or deleted, but it can be
//! renamed, so this one moves aside to `kinetic-pdf.<pid>.old` and the new one
//! takes its name. This process carries on as it was, and the next start runs
//! the new version. Moved-aside exes are deleted at a later start, once nothing
//! runs them any more.
//!
//! The Store build asks the Store instead, and installing hands over to the
//! Store's own dialog; the app's folder there is read-only, and the Store is
//! the only thing allowed to put a new version in it.
//!
//! Debug builds don't check, so `cargo run` never swaps out target\debug's exe.
//! `KINETIC_PDF_UPDATE=0` turns checking off, and any other value turns it on,
//! debug builds included.

#[cfg(feature = "store")]
mod store;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;

use crate::worker::trace;

/// Where releases are published. .github/workflows/release.yml attaches
/// `ASSET` to each one, tagged `v` and the version in Cargo.toml.
#[cfg(not(feature = "store"))]
const REPO: &str = "c0deZ3R0/kinetic-pdf";
#[cfg(not(feature = "store"))]
const ASSET: &str = "kinetic-pdf.exe";

/// The version of this build, from Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How long after startup to check, so the check never competes with opening
/// a file.
const CHECK_DELAY: Duration = Duration::from_secs(3);

/// How long between checks while the app stays open.
const RECHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);

/// Far larger than any build; a download past it is refused.
#[cfg(not(feature = "store"))]
const MOST_BYTES: u64 = 256 * 1024 * 1024;

/// The Store reports availability without a destination version. Only GitHub
/// offers carry a release tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferedUpdate {
    pub tag: Option<String>,
}

impl OfferedUpdate {
    pub fn button_label(&self) -> String {
        self.tag.as_ref().map_or_else(|| "Update available".into(), |tag| format!("Update to {tag}"))
    }

    #[cfg(feature = "store")]
    fn from_store_count(count: u32) -> Option<Self> {
        (count > 0).then_some(Self { tag: None })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum State {
    /// Not checked yet, checking, or nothing newer.
    Current,
    /// The source has an update available for the running app.
    Available(OfferedUpdate),
    Downloading(OfferedUpdate),
    /// Swapped in; it runs from the next start.
    Ready(OfferedUpdate),
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
            // A Store install's folder is read-only, and holds no leftovers.
            if !cfg!(feature = "store") {
                remove_leftovers();
            }
            if !enabled() {
                return;
            }
            std::thread::sleep(CHECK_DELAY);
            loop {
                match newest_offered() {
                    // Once something is on offer, the button speaks for it;
                    // a later check mustn't undo a download under way.
                    Ok(Some(tag)) => {
                        let mut state = state.lock().unwrap();
                        if *state == State::Current {
                            *state = State::Available(tag);
                            ctx.request_repaint();
                        }
                    }
                    Ok(None) => trace(format_args!("update: v{VERSION} is the latest")),
                    // Offline, or no release yet: nothing worth telling anyone.
                    Err(e) => trace(format_args!("update: could not check: {e}")),
                }
                std::thread::sleep(RECHECK_EVERY);
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
    /// `_reopen` is the file open now, for the Store build to open again
    /// once the Store has put the new version in and started it.
    pub fn install(&self, _reopen: Option<PathBuf>) {
        let State::Available(tag) = self.state() else { return };
        self.set(State::Downloading(tag.clone()));
        let (state, ctx) = (Arc::clone(&self.state), self.ctx.clone());
        // Started here, on the thread that runs the window, as the Store
        // requires; its dialog is shown over that window.
        #[cfg(feature = "store")]
        let pending = {
            let window = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow() } as isize;
            match store::request_install(window, _reopen.as_deref()) {
                Ok(pending) => pending,
                Err(e) => return self.set(State::Failed(e)),
            }
        };
        std::thread::spawn(move || {
            #[cfg(not(feature = "store"))]
            let next = match download(tag.tag.as_deref().expect("GitHub updates have a release tag")).and_then(|bytes| swap_in(&bytes)) {
                Ok(()) => State::Ready(tag),
                Err(e) => State::Failed(e),
            };
            #[cfg(feature = "store")]
            let next = match pending.wait() {
                Ok(store::Installed::Done) => State::Ready(tag),
                Ok(store::Installed::Declined) => State::Available(tag),
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
    match std::env::var_os("KINETIC_PDF_UPDATE") {
        Some(value) => value != "0",
        None => !cfg!(debug_assertions),
    }
}

#[cfg(not(feature = "store"))]
fn agent(max_redirects: u32, timeout: Duration) -> ureq::Agent {
    ureq::config::Config::builder()
        .user_agent(format!("kinetic-pdf/{VERSION}"))
        .https_only(true)
        .max_redirects(max_redirects)
        .timeout_global(Some(timeout))
        .build()
        .new_agent()
}

/// The Store's update list is authoritative; its Package objects describe
/// installed packages, so comparing their versions would hide updates.
fn newest_offered() -> Result<Option<OfferedUpdate>, String> {
    #[cfg(feature = "store")]
    { store::check() }
    #[cfg(not(feature = "store"))]
    {
        let tag = latest_tag(REPO)?;
        Ok(is_newer(&tag, VERSION).then_some(OfferedUpdate { tag: Some(tag) }))
    }
}

/// The tag of `repo`'s latest release, from where GitHub redirects its page.
#[cfg(not(feature = "store"))]
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
#[cfg(not(feature = "store"))]
fn tag_from_location(location: &str) -> Option<&str> {
    let tag = location.rsplit_once("/releases/tag/")?.1;
    (!tag.is_empty() && !tag.contains('/')).then_some(tag)
}

/// Whether release `tag` is a later version than `current`.
pub(crate) fn is_newer(tag: &str, current: &str) -> bool {
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

#[cfg(not(feature = "store"))]
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

#[cfg(not(feature = "store"))]
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
    fn github_offer_names_the_release() {
        let offer = OfferedUpdate { tag: Some("v0.13.1".into()) };
        assert_eq!(offer.button_label(), "Update to v0.13.1");
    }

    #[cfg(feature = "store")]
    #[test]
    fn store_updates_do_not_require_a_newer_package_version() {
        assert_eq!(OfferedUpdate::from_store_count(0), None);
        for count in [1, 2] {
            let offer = OfferedUpdate::from_store_count(count).unwrap();
            assert_eq!(offer.tag, None, "installed versions are not release tags");
            assert_eq!(offer.button_label(), "Update available");
        }
    }

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

    #[cfg(not(feature = "store"))]
    #[test]
    fn the_tag_comes_from_the_redirect() {
        let location = "https://github.com/c0deZ3R0/kinetic-pdf/releases/tag/v0.2.0";
        assert_eq!(tag_from_location(location), Some("v0.2.0"));
        assert_eq!(tag_from_location("https://github.com/c0deZ3R0/kinetic-pdf/releases"), None);
        assert_eq!(tag_from_location(""), None);
    }

    /// Against a real repo with releases: `cargo test --lib update:: -- --ignored`.
    #[cfg(not(feature = "store"))]
    #[test]
    #[ignore = "needs the network"]
    fn reads_the_latest_tag_from_github() {
        let tag = latest_tag("BurntSushi/ripgrep").unwrap();
        assert!(parse_version(&tag).is_some(), "{tag}");
        assert!(is_newer(&tag, "0.0.1"), "{tag}");
    }

    #[test]
    fn only_update_files_are_leftovers() {
        assert!(is_leftover("kinetic-pdf.new", "kinetic-pdf"));
        assert!(is_leftover("kinetic-pdf.1234.old", "kinetic-pdf"));
        assert!(!is_leftover("kinetic-pdf.exe", "kinetic-pdf"));
        assert!(!is_leftover("kinetic-pdf.pdb", "kinetic-pdf"));
        assert!(!is_leftover("kinetic-pdf.notes.old", "kinetic-pdf"));
        assert!(!is_leftover("bench.1234.old", "kinetic-pdf"));
    }
}
