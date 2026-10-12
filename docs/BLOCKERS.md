# Blockers / things the owner must do

## Node.js 16 is end-of-life (not blocking)

`node --version` → v16.13.0. Current Vite needs Node 18+, so the frontend pins Vite 4.5.14. To move to current Vite, install Node LTS from https://nodejs.org/ (GUI installer, so not done in this run), then bump `vite` in `app/package.json`.

## Android SDK / NDK not installed (M6)

`ANDROID_HOME` and `NDK_HOME` unset, no SDK under `%LOCALAPPDATA%\Android\Sdk`, no Rust Android targets. M6 was not attempted. Full steps (Android Studio install, env vars, `rustup target add`, `cargo tauri android init/build`, keep-screen-awake options) are in `docs/ANDROID.md`. Needs ~10 GB free disk (SDK + NDK + Gradle cache).
