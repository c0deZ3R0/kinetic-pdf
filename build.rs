//! Two jobs before the app compiles:
//!
//! - pdfium.dll is compiled into the exe with `include_bytes!` (see worker.rs).
//!   Stop early with instructions if it hasn't been downloaded, rather than
//!   failing later with a bare "file not found".
//! - Give the exe its icon and the name Explorer shows for it.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=pdfium.dll");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    // Android loads libpdfium.so from the APK instead; see android/.
    let android = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android");
    if !android && !dir.join("pdfium.dll").exists() {
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
