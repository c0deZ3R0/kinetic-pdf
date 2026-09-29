//! Two jobs before the app compiles:
//!
//! - pdfium.dll is compiled into the exe with `include_bytes!` (see worker.rs).
//!   Stop early with instructions if it hasn't been downloaded, rather than
//!   failing later with a bare "file not found".
//! - Give the exe its icon and the name Explorer shows for it.
//! - Gather each version's release notes, `packaging/store-changes-<version>.txt`,
//!   into a list the app shows as "What's new" (src/app/whats_new.rs). The same
//!   text is pasted into the Store submission, so it is written once.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=pdfium.dll");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=packaging");

    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("release_notes.rs"), release_notes(&dir.join("packaging"))).expect("could not write release_notes.rs");
    if !dir.join("pdfium.dll").exists() {
        panic!(
            "\n\npdfium.dll is missing; it gets compiled into the exe.\n\
             Download it first by running this in {}:\n\n    \
             powershell -ExecutionPolicy Bypass -File get-pdfium.ps1\n\n",
            dir.display()
        );
    }

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // Ask hybrid-graphics drivers for the discrete GPU; see main.rs.
        for symbol in ["NvOptimusEnablement", "AmdPowerXpressRequestHighPerformance"] {
            println!("cargo:rustc-link-arg-bin=kinetic-pdf=/EXPORT:{symbol},DATA");
        }
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resources = winresource::WindowsResource::new();
        resources
            .set_icon("assets/icon.ico")
            // Explorer and Task Manager show FileDescription as the app's name.
            .set("FileDescription", "Kinetic PDF")
            .set("ProductName", "Kinetic PDF");
        resources.compile().expect(
            "could not embed the icon; rc.exe comes with the Windows SDK, which the \
             Visual Studio Build Tools C++ workload installs",
        );
    }
}

/// `RELEASE_NOTES`: every version that has notes, newest first, with the
/// notes read in from their file.
fn release_notes(packaging: &Path) -> String {
    let mut found: Vec<([u64; 3], String, PathBuf)> = std::fs::read_dir(packaging)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let version = name.strip_prefix("store-changes-")?.strip_suffix(".txt")?.to_owned();
            let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
            let number = [parts.next()??, parts.next()??, parts.next()??];
            parts.next().is_none().then_some((number, version, entry.path()))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let rows: String = found.iter().map(|(_, version, path)| format!("    ({version:?}, include_str!({:?})),\n", path.display().to_string())).collect();
    format!("/// Each version's release notes, newest first.\npub(super) const RELEASE_NOTES: &[(&str, &str)] = &[\n{rows}];\n")
}
