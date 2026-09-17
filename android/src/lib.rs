//! Kinetic PDF on Android. The app is the same library the Windows exe runs;
//! this is what differs around it:
//!
//! - Android starts the app by calling `android_main` from NativeActivity, and
//!   gives it folders of its own for the cache and the author name.
//! - A PDF arrives through "Open with" as a content URI rather than a path, so
//!   it is copied into the app's folder and opened from there, and each save
//!   is written back through the same URI, if the app that shared it allows.
//! - Android shows the on-screen keyboard only when asked, so it is asked
//!   whenever a text box has the focus.

#![cfg(target_os = "android")]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;
use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use kinetic_pdf::app;
use winit::platform::android::activity::AndroidApp;

/// How often to look for a save to write back.
const SAVE_CHECK: Duration = Duration::from_secs(1);

#[no_mangle]
fn android_main(android: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("kinetic-pdf"));
    std::panic::set_hook(Box::new(|info| log::error!("{info}")));

    let Some(data) = android.internal_data_path() else {
        log::error!("Android gave the app no folder of its own");
        return;
    };
    use_folders(&data);

    let opened = match opened_file(&android, &data) {
        Ok(opened) => opened,
        Err(e) => {
            log::error!("could not open the file the app was opened with: {e}");
            None
        }
    };
    let initial = opened.as_ref().map(|o| o.copy.clone());

    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        android_app: Some(android.clone()),
        ..Default::default()
    };
    // As on Windows; see main.rs.
    options.multisampling = 4;
    options.stencil_buffer = 8;

    let result = eframe::run_native(
        "Kinetic PDF",
        options,
        Box::new(move |cc| {
            Ok(Box::new(Phone {
                app: app::App::new(cc, initial),
                android,
                keyboard: false,
                opened,
                checked: Instant::now(),
            }))
        }),
    );
    if let Err(e) = &result {
        log::error!("{e}");
    }
    // winit makes its event loop once per process, and Android keeps the
    // process around to start the app in again, so end it with the app.
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}

/// Points the cache, the author name and temporary files, which look for the
/// usual folders of a Linux user, into the app's own.
fn use_folders(data: &Path) {
    let tmp = data.join("tmp");
    let _ = std::fs::create_dir_all(&tmp);
    std::env::set_var("HOME", data);
    std::env::set_var("XDG_CACHE_HOME", data.join("cache"));
    std::env::set_var("XDG_CONFIG_HOME", data.join("config"));
    std::env::set_var("TMPDIR", tmp);
}

/// The app with what Android needs done around it.
struct Phone {
    app: app::App,
    android: AndroidApp,
    /// Whether the on-screen keyboard was last asked for.
    keyboard: bool,
    opened: Option<Opened>,
    checked: Instant,
}

/// A file the app was opened with, and its copy the app reads and saves.
struct Opened {
    copy: PathBuf,
    uri: GlobalRef,
    /// When the copy last matched the original.
    written: Option<SystemTime>,
}

impl eframe::App for Phone {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);

        let wants = ui.ctx().egui_wants_keyboard_input();
        if wants != self.keyboard {
            if wants {
                self.android.show_soft_input(false);
            } else {
                self.android.hide_soft_input(false);
            }
            self.keyboard = wants;
        }

        if self.checked.elapsed() >= SAVE_CHECK {
            self.checked = Instant::now();
            self.write_back_saves();
        }
    }
}

impl Phone {
    /// Writes the copy back to where it came from once it has been saved.
    fn write_back_saves(&mut self) {
        let Some(opened) = &mut self.opened else { return };
        let modified = std::fs::metadata(&opened.copy).and_then(|m| m.modified()).ok();
        if modified.is_none() || modified == opened.written {
            return;
        }
        opened.written = modified;
        let (android, uri, copy) = (self.android.clone(), opened.uri.clone(), opened.copy.clone());
        std::thread::spawn(move || match write_back(&android, &uri, &copy) {
            Ok(()) => log::info!("saved back to the original"),
            Err(e) => log::error!("could not save back to the original, only to {}: {e}", copy.display()),
        });
    }
}

/* ------------------------------------------------------------------ *
 * Files through Android's content URIs
 * ------------------------------------------------------------------ */

fn vm(android: &AndroidApp) -> Result<JavaVM, String> {
    unsafe { JavaVM::from_raw(android.vm_as_ptr().cast()) }.map_err(|e| e.to_string())
}

/// The activity, which android-activity holds a reference to for as long as
/// the app runs.
fn activity(android: &AndroidApp) -> JObject<'static> {
    unsafe { JObject::from_raw(android.activity_as_ptr().cast()) }
}

/// A JNI call's result, with any Java exception it threw cleared so the next
/// call can go ahead.
fn java<T>(env: &mut JNIEnv, result: jni::errors::Result<T>) -> Result<T, String> {
    result.map_err(|e| {
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        e.to_string()
    })
}

fn content_resolver<'local>(env: &mut JNIEnv<'local>, android: &AndroidApp) -> Result<JObject<'local>, String> {
    let resolver = env.call_method(activity(android), "getContentResolver", "()Landroid/content/ContentResolver;", &[]);
    java(env, resolver.and_then(|r| r.l()))
}

/// Opens `uri` through the content resolver, in a mode such as "r" or "wt".
fn open_uri(env: &mut JNIEnv, android: &AndroidApp, uri: &JObject, mode: &str) -> Result<File, String> {
    let resolver = content_resolver(env, android)?;
    let mode = java(env, env.new_string(mode))?;
    let pfd = env.call_method(
        &resolver,
        "openFileDescriptor",
        "(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
        &[JValue::Object(uri), JValue::Object(&mode)],
    );
    let pfd = java(env, pfd.and_then(|p| p.l()))?;
    if pfd.is_null() {
        return Err("the provider gave no file".to_owned());
    }
    let fd = env.call_method(&pfd, "detachFd", "()I", &[]);
    let fd = java(env, fd.and_then(|f| f.i()))?;
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// The file in the intent that started the app, copied into `data`.
fn opened_file(android: &AndroidApp, data: &Path) -> Result<Option<Opened>, String> {
    let vm = vm(android)?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;

    let intent = env.call_method(activity(android), "getIntent", "()Landroid/content/Intent;", &[]);
    let intent = java(&mut env, intent.and_then(|i| i.l()))?;
    if intent.is_null() {
        return Ok(None);
    }
    let uri = env.call_method(&intent, "getData", "()Landroid/net/Uri;", &[]);
    let uri = java(&mut env, uri.and_then(|u| u.l()))?;
    if uri.is_null() {
        return Ok(None);
    }

    let name = display_name(&mut env, android, &uri).unwrap_or_else(|| "document.pdf".to_owned());
    let dir = data.join("opened");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let copy = dir.join(safe_file_name(&name));

    let mut bytes = Vec::new();
    open_uri(&mut env, android, &uri, "r")?.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    std::fs::write(&copy, &bytes).map_err(|e| e.to_string())?;

    let uri = env.new_global_ref(&uri);
    let uri = java(&mut env, uri)?;
    let written = std::fs::metadata(&copy).and_then(|m| m.modified()).ok();
    Ok(Some(Opened { copy, uri, written }))
}

/// The name the provider shows for `uri`, from its OpenableColumns.
fn display_name(env: &mut JNIEnv, android: &AndroidApp, uri: &JObject) -> Option<String> {
    let resolver = content_resolver(env, android).ok()?;
    let null = JObject::null();
    let cursor = env.call_method(
        &resolver,
        "query",
        "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
        &[JValue::Object(uri), JValue::Object(&null), JValue::Object(&null), JValue::Object(&null), JValue::Object(&null)],
    );
    let cursor = java(env, cursor.and_then(|c| c.l())).ok()?;
    if cursor.is_null() {
        return None;
    }
    let name = (|| {
        let first = env.call_method(&cursor, "moveToFirst", "()Z", &[]);
        if !java(env, first.and_then(|f| f.z())).ok()? {
            return None;
        }
        let column = java(env, env.new_string("_display_name")).ok()?;
        let index = env.call_method(&cursor, "getColumnIndex", "(Ljava/lang/String;)I", &[JValue::Object(&column)]);
        let index = java(env, index.and_then(|i| i.i())).ok().filter(|i| *i >= 0)?;
        let value = env.call_method(&cursor, "getString", "(I)Ljava/lang/String;", &[JValue::Int(index)]);
        let value = JString::from(java(env, value.and_then(|v| v.l())).ok()?);
        if value.is_null() {
            return None;
        }
        let name = env.get_string(&value).map(String::from);
        java(env, name).ok()
    })();
    let closed = env.call_method(&cursor, "close", "()V", &[]);
    let _ = java(env, closed);
    name
}

/// `name` as a file name that can't leave the folder, ending in .pdf.
fn safe_file_name(name: &str) -> String {
    let mut safe: String = name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    if safe.is_empty() || safe.starts_with('.') {
        safe.insert_str(0, "document");
    }
    if !safe.to_lowercase().ends_with(".pdf") {
        safe.push_str(".pdf");
    }
    safe
}

/// Replaces the original behind `uri` with the saved copy.
fn write_back(android: &AndroidApp, uri: &GlobalRef, copy: &Path) -> Result<(), String> {
    let bytes = std::fs::read(copy).map_err(|e| e.to_string())?;
    let vm = vm(android)?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let mut file = open_uri(&mut env, android, uri.as_obj(), "wt")?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    // Some providers don't truncate for "wt"; a file that shrank would keep
    // its old tail.
    let _ = file.set_len(bytes.len() as u64);
    file.sync_all().map_err(|e| e.to_string())
}

