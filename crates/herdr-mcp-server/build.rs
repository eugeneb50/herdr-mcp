//! Builds the web UI into the binary: every file of the `dx bundle` output
//! (`<target>/dx/herdr-mcp-ui/release/web/public`) becomes an entry of
//! `$OUT_DIR/web_assets.rs`, which `web.rs` includes. No bundle → no entries,
//! and the bridge serves only `/api/*` unless `http.web-dir` is set.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_TARGET_DIR");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../target"));
    let bundle = target.join("dx/herdr-mcp-ui/release/web/public");
    // Cargo reruns a build script whose watched path is missing on every build;
    // an empty bundle dir keeps it quiet until `dx bundle` fills it.
    if let Err(e) = std::fs::create_dir_all(&bundle) {
        println!("cargo:warning=cannot create {}: {e}", bundle.display());
    }
    println!("cargo:rerun-if-changed={}", bundle.display());

    let mut files = Vec::new();
    collect(&bundle, &mut files);
    files.sort();
    if !files.iter().any(|f| f == &bundle.join("index.html")) {
        files.clear();
    }

    let mut out = String::from("&[\n");
    for file in &files {
        let rel = file
            .strip_prefix(&bundle)
            .expect("collected under the bundle dir")
            .to_string_lossy()
            .replace('\\', "/");
        let _ = writeln!(
            out,
            "    ({rel:?}, {:?}, include_bytes!({:?})),",
            mime(file),
            file.display().to_string()
        );
    }
    out.push(']');
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo")).join("web_assets.rs");
    std::fs::write(dest, out).expect("write web_assets.rs");
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
