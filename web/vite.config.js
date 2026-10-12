// Dev: Vite serves the frontend and forwards /api to `peer serve`.
// https://v4.vitejs.dev/config/server-options.html#server-proxy
// The backend only accepts writes from its own origin (CSRF guard), so the
// proxy presents that origin. Dev only; production is same-origin.
const backend = "http://127.0.0.1:7430";
export default {
  server: { port: 5173, strictPort: true, proxy: { "/api": { target: backend, headers: { origin: backend } } } },
  build: { target: "es2021" },
};
