# Blockers / things the owner must do

## `cargo tauri build` failed: C: drive is full (M5)

```
error: failed to build archive at `...\COUPLYCRAB\target\release\deps\libhousehold_app_lib.rlib`:
There is not enough space on the disk. (os error 112)
```

The C: drive (931 GB) had about 7 GB free when the run started. `target/debug` takes ~4.9 GB and the release build needs another ~1.5 GB plus the installer. I deleted the half-written `target/release` (my own build output) to bring free space back to 1.4 GB, and did not retry, so the disk isn't left at zero.

`cargo tauri dev` was run and verified (see DECISIONS.md "Desktop app (M5)"). To produce installers:

1. Free at least ~5 GB on C: (or `cargo clean` here: the whole `target/` dir is regenerable).
2. `cd app && cargo tauri build` (https://v2.tauri.app/reference/cli/#build).
3. Expected output (not verified): `target/release/household-app.exe`, plus installers under `target/release/bundle/msi/` and `target/release/bundle/nsis/`. Building the MSI/NSIS bundles downloads WiX/NSIS into Tauri's cache on first run (https://v2.tauri.app/distribute/windows-installer/).

## Node.js 16 is end-of-life (not blocking)

`node --version` → v16.13.0. Current Vite needs Node 18+, so the frontend pins Vite 4.5.14. To move to current Vite, install Node LTS from https://nodejs.org/ (GUI installer, so not done in this run), then bump `vite` in `app/package.json`.
