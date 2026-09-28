//! Updates for the Microsoft Store build, asked of the Store itself.
//!
//! A Store app may not fetch its own updates, and the Store only installs
//! them in the background while the app is closed -- which, for someone who
//! leaves it open or never opens the Store, can be a long wait. So the app
//! asks the Store (`StoreContext`) whether a newer package of it is out, and
//! offers it on the toolbar the way the GitHub build does. Taking it up hands
//! over to the Store's own dialog, which asks the user, then downloads and
//! installs the package. This is Microsoft's documented way for an app to
//! update itself from the Store ("Download and install package updates from
//! the Store").
//!
//! Installing a new package of the running app closes it. The app asks about
//! unsaved work before it gets that far, and registers to be started again
//! once the update is in (`RegisterApplicationRestart`, for updates only), with
//! the file that was open.
//!
//! The check blocks until the Store answers, so it runs on the updater's own
//! thread. Starting the install is different: the Store requires that call on
//! the thread that runs the window, so `request_install` is made there and
//! returns at once, and only the wait for the Store to finish is moved off it.
//! Outside a Store install -- `cargo run --features store`, or a package
//! side-loaded with `make-msix.ps1 -Test` -- the Store has no listing to
//! compare against, and the check reports nothing to update.

use std::path::Path;
use std::sync::Mutex;

use windows::core::{Interface, HSTRING};
use windows_collections::IVectorView;
use windows_future::IAsyncOperationWithProgress;
use windows::Services::Store::{StoreContext, StorePackageUpdate, StorePackageUpdateResult, StorePackageUpdateState, StorePackageUpdateStatus};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::IInitializeWithWindow;

/// What the last check found, kept for the install to hand back to the Store.
static OFFERED: Mutex<Option<Offered>> = Mutex::new(None);

/// The Store's update objects are agile (usable from any thread), which the
/// bindings can't know of a generic collection.
struct Offered(IVectorView<StorePackageUpdate>);
unsafe impl Send for Offered {}

/// The version the Store has on offer, as a tag like `v0.10.0`, or `None`
/// when this is the newest.
pub fn check() -> Result<Option<String>, String> {
    let context = StoreContext::GetDefault().map_err(text)?;
    let updates = context.GetAppAndOptionalStorePackageUpdatesAsync().map_err(text)?.join().map_err(text)?;
    let mut newest: Option<[u16; 3]> = None;
    for at in 0..updates.Size().map_err(text)? {
        let version = updates.GetAt(at).and_then(|u| u.Package()?.Id()?.Version()).map_err(text)?;
        // A package's fourth number is always 0 for this app (make-msix.ps1).
        let version = [version.Major, version.Minor, version.Build];
        newest = newest.max(Some(version));
    }
    *OFFERED.lock().unwrap() = newest.is_some().then_some(Offered(updates));
    Ok(newest.map(|[major, minor, patch]| format!("v{major}.{minor}.{patch}")))
}

/// How an install through the Store ended, when the app is still running to
/// hear it.
pub enum Installed {
    /// Put in place; it runs from the next start.
    Done,
    /// Turned down in the Store's dialog.
    Declined,
}

/// An install the Store is carrying out, to wait on away from the window.
pub struct Pending(IAsyncOperationWithProgress<StorePackageUpdateResult, StorePackageUpdateStatus>);
unsafe impl Send for Pending {}

/// Asks the Store to download and install what `check` found, showing its
/// own dialog over `window`. Must be called on the thread that runs the
/// window, as the Store requires; returns as soon as the Store has taken the
/// request.
pub fn request_install(window: isize, reopen: Option<&Path>) -> Result<Pending, String> {
    // Kept, not taken: turned down in the dialog, the button comes back.
    let Some(updates) = OFFERED.lock().unwrap().as_ref().map(|offered| offered.0.clone()) else {
        return Err("The update is no longer on offer; it may already be installed.".into());
    };
    let context = StoreContext::GetDefault().map_err(text)?;
    // A desktop app has no window of its own for the Store to know of, so the
    // dialog is told which one to sit over.
    let hwnd = HWND(window as *mut _);
    context.cast::<IInitializeWithWindow>().and_then(|init| unsafe { init.Initialize(hwnd) }).map_err(text)?;
    restart_after_update(reopen);
    context.RequestDownloadAndInstallStorePackageUpdatesAsync(&updates).map(Pending).map_err(text)
}

impl Pending {
    /// Blocks until the Store is done. If the running app is among what it
    /// installs, the Store closes the app before this returns.
    pub fn wait(self) -> Result<Installed, String> {
        let result = self.0.join().map_err(text)?;
        match result.OverallState().map_err(text)? {
            StorePackageUpdateState::Completed => Ok(Installed::Done),
            StorePackageUpdateState::Canceled => Ok(Installed::Declined),
            StorePackageUpdateState::ErrorLowBattery => Err("The Store won't update while the battery is low.".into()),
            StorePackageUpdateState::ErrorWiFiRecommended | StorePackageUpdateState::ErrorWiFiRequired => {
                Err("The Store is waiting for a Wi-Fi connection to download the update.".into())
            }
            state => Err(format!("The Store couldn't install the update ({state:?}).")),
        }
    }
}

/// Asks Windows to start the app again once an update has replaced it, with
/// `reopen` open. Only for updates: not after a crash, a hang or a restart of
/// the PC, which would otherwise start the app unasked.
fn restart_after_update(reopen: Option<&Path>) {
    use windows_sys::Win32::System::Recovery::{RegisterApplicationRestart, RESTART_NO_CRASH, RESTART_NO_HANG, RESTART_NO_REBOOT};
    // The command line after the exe's own name, at most 1024 characters.
    let line = reopen.map(|path| format!("\"{}\"", path.display())).filter(|line| line.encode_utf16().count() < 1024).unwrap_or_default();
    let line = HSTRING::from(line);
    let result = unsafe { RegisterApplicationRestart(line.as_ptr(), RESTART_NO_CRASH | RESTART_NO_HANG | RESTART_NO_REBOOT) };
    if result != 0 {
        crate::worker::trace(format_args!("update: could not register to restart after the update: {result:#x}"));
    }
}

fn text(e: windows::core::Error) -> String {
    let message = e.message();
    if message.is_empty() {
        format!("The Store couldn't be reached ({:#x}).", e.code().0)
    } else {
        message
    }
}
