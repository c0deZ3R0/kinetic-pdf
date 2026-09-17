# Kinetic PDF for Android

An experimental build of the same app for Android phones (arm64). Open a PDF
from a file manager, Drive or mail with **Open with → Kinetic PDF**; saves go
back to that file when the app that shared it allows writing.

Not there yet: the app's own Open button does nothing (there is no document
picker yet), a second "Open with" while the app is running brings the app
forward without opening the new file (swipe the app away first), and the
toolbar is laid out for a desktop window.

## Build

Needs Rust with the `aarch64-linux-android` target, the Android NDK, the
Android SDK (platform 34 and build-tools 34), a JDK, and
[cargo-apk](https://crates.io/crates/cargo-apk).

```
powershell -ExecutionPolicy Bypass -File android/get-pdfium.ps1
set ANDROID_HOME=%LOCALAPPDATA%\Android\Sdk
set ANDROID_NDK_ROOT=%LOCALAPPDATA%\Android\ndk\android-ndk-r28c
set CARGO_APK_RELEASE_KEYSTORE=%USERPROFILE%\.android\debug.keystore
set CARGO_APK_RELEASE_KEYSTORE_PASSWORD=android
cargo apk build -p kinetic-pdf-android --release
```

The APK is `target/release/apk/kinetic-pdf-android.apk`. It is signed with the
debug key, which is fine for installing on your own phone but not for a store.
Errors and panics go to logcat under the tag `kinetic-pdf`:
`adb logcat -s kinetic-pdf`.
