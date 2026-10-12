# Blockers / things the owner must do

## Node.js 16 is end-of-life (not blocking)

`node --version` → v16.13.0. Current Vite needs Node 18+, so the frontend pins Vite 4.5.14. To move to current Vite, install Node LTS from https://nodejs.org/ (GUI installer, so not done in this run), then bump `vite` in `app/package.json`.
