# Android (M6): not attempted, prerequisites missing

Checked on 2026-10-12:

| Prerequisite | State |
|---|---|
| `ANDROID_HOME` | not set; no SDK at `%LOCALAPPDATA%\Android\Sdk` |
| `NDK_HOME` | not set |
| JDK | present (`JAVA_HOME=C:\Program Files\Java\jdk-17.0.2`; JDK 21 also on PATH) |
| Rust Android targets | not installed (only `x86_64-pc-windows-msvc`) |
| Free disk on C: | ~1.4 GB, far too little for SDK + NDK + Gradle cache (plan on 10+ GB) |

Per PLAN §5 M6, nothing was installed. Steps for the owner, from https://v2.tauri.app/start/prerequisites/#android:

## 1. Install the SDK and NDK

1. Free disk space first (see BLOCKERS.md).
2. Install Android Studio: https://developer.android.com/studio (GUI installer).
3. In Android Studio → Settings → Languages & Frameworks → Android SDK, install:
   - *SDK Platforms:* one recent Android platform
   - *SDK Tools:* Android SDK Platform-Tools, NDK (Side by side), Android SDK Build-Tools, Android SDK Command-line Tools
4. Set user environment variables (PowerShell, then open a new terminal):
   ```powershell
   [System.Environment]::SetEnvironmentVariable("JAVA_HOME", "C:\Program Files\Android\Android Studio\jbr", "User")
   [System.Environment]::SetEnvironmentVariable("ANDROID_HOME", "$env:LocalAppData\Android\Sdk", "User")
   $ndk = (Get-ChildItem "$env:LocalAppData\Android\Sdk\ndk" | Select-Object -Last 1).FullName
   [System.Environment]::SetEnvironmentVariable("NDK_HOME", $ndk, "User")
   ```
   (Commands from the Tauri prerequisites page above. Using Android Studio's bundled JBR avoids Gradle/JDK version mismatches with the JDK 17/21 already installed.)
5. Rust targets:
   ```sh
   rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
   ```

## 2. Build

From `app/` (https://v2.tauri.app/reference/cli/#android):

```sh
cargo tauri android init     # generates app/src-tauri/gen/android: commit it
cargo tauri android dev      # run on a USB-connected phone or emulator
cargo tauri android build    # APK/AAB under app/src-tauri/gen/android/app/build/outputs/
```

`app/src-tauri/src/lib.rs` already has `#[cfg_attr(mobile, tauri::mobile_entry_point)]` and the crate builds as `cdylib`/`staticlib`, which is what Android needs. Nothing in the cores or `sync` is desktop-specific.

On the phone: install and connect the Tailscale Android app, then in the first-run screen set `listen_ip` to the phone's Tailscale IP (shown in the Tailscale app) and `peers` to the laptops' MagicDNS names.

## 3. Keep the screen awake on Log Session

Options, laziest first. None verified (no device/SDK in this run):

1. **Web Screen Wake Lock API, no plugin.** `await navigator.wakeLock.request("screen")` when the Log Session screen shows; release on leave. Android System WebView (Chromium) supports it since v84, but it needs a secure context: Tauri 2 on Android serves the app from `http://tauri.localhost`, and Chromium treats `*.localhost` as potentially trustworthy, so it should qualify. Test this first. https://developer.mozilla.org/en-US/docs/Web/API/Screen_Wake_Lock_API
2. **`tauri-plugin-keep-screen-on` 0.1.4** (https://gitlab.com/cristofa/tauri-plugin-keep-screen-on, crates.io ~12.7k downloads). Android/iOS. **Maturity flag:** single maintainer, last release 2024-08-30, 0.x. Check it builds against tauri 2.12 before relying on it.
3. **Minimal own Kotlin plugin** (https://v2.tauri.app/develop/plugins/develop-mobile/): one command that runs `activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)` (and `clearFlags` to release) on the UI thread. About 20 lines of Kotlin; no third-party maintenance risk. https://developer.android.com/develop/background-work/background-tasks/awake/screen-on

Not considered: `tauri-plugin-keepawake` (desktop-focused, via the `keepawake` crate).

## 4. v0 Android only syncs while the app is open

The sync loop is a tokio task inside the app process. When the app is backgrounded, Android stops scheduling it within minutes (and kills the process when memory is needed), so the phone syncs only while the app is on screen: on startup, after each change, and every 30 s while open. Changes made on the phone reach the other devices next time the app is open while another peer is online. See LEARNING.md §8.
