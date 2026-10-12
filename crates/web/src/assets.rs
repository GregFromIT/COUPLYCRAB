//! The frontend (`web/dist`, built by `npm run build` in `web/`).
//!
//! Release builds embed the files into the binary, so deploying is copying one
//! file. Debug builds read them from disk at request time (rust-embed's default),
//! so a fresh `npm run build` shows up without recompiling.
//! https://docs.rs/rust-embed/8.12.0/rust_embed/

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};

#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/dist"]
// Lets `cargo build` / `cargo test` work before the frontend was ever built;
// `/` then answers with a hint instead of the app.
#[allow_missing = true]
struct Dist;

pub async fn serve(uri: Uri) -> Response {
    let path = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        p => p,
    };
    match Dist::get(path) {
        Some(file) => {
            // Vite puts a content hash in asset file names, so they never change.
            let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
            ([(header::CONTENT_TYPE, file.metadata.mimetype().to_string()), (header::CACHE_CONTROL, cache.to_string())], file.data).into_response()
        }
        None if path == "index.html" => {
            (StatusCode::NOT_FOUND, "Frontend not built: run `npm install && npm run build` in web/, then rebuild peer.").into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
