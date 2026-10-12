// Tauri expects a fixed dev port: https://v2.tauri.app/start/frontend/vite/
export default {
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2021" },
};
