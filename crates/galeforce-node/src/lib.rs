//! Node bindings (napi-rs) for GaleforceCSS.
//!
//! Exposes the compiler + scanner over N-API. The JS public package
//! (`galeforcecss`) prefers these bindings when the `.node` artifact for
//! the host platform is installed, and falls back to spawning the
//! `galeforcecss compile-stream` CLI bridge otherwise.
//!
//! Surface:
//!
//! - `compile(optionsJson: string) -> string` — single-shot compile.
//!   Takes a JSON-encoded `CompileOptions` and returns a JSON-encoded
//!   `CompileResult`. We pass JSON instead of structured napi objects
//!   because the underlying types live in `galeforce-core` and already
//!   round-trip through serde; the JSON pass-through keeps the binding
//!   layer thin (no manual napi struct definitions for every option)
//!   and gives us a single place (the CLI's `compile-json` subcommand)
//!   that owns the wire format.
//! - `scan(pathsJson: string) -> string` — walk a flat list of file
//!   paths, tokenize each, return JSON array of unique candidates.
//! - `scanPerFile(rootsJson: string) -> string` — walk content roots,
//!   return `{ path -> [candidates] }` so callers can build per-file
//!   refcount-style incremental caches.
//! - `version() -> string` — `galeforce-core`'s crate version.

use galeforce_core::{CompileOptions, CompileResult};
use galeforce_scanner::{discover_files, extract_candidates, WalkOptions};
use napi::Error;
use napi_derive::napi;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

/// Compile a `CompileOptions` JSON string. Returns the `CompileResult`
/// as a JSON string. Errors surface as napi `Error`s so callers see
/// thrown JS exceptions with the message intact.
#[napi]
pub fn compile(options_json: String) -> Result<String, Error> {
    let mut options: CompileOptions = serde_json::from_str(&options_json)
        .map_err(|e| Error::from_reason(format!("invalid CompileOptions JSON: {e}")))?;
    apply_content_scan(&mut options);
    let result: CompileResult = galeforce_compiler::compile(&options);
    serde_json::to_string(&result)
        .map_err(|e| Error::from_reason(format!("serializing CompileResult: {e}")))
}

/// Walk a list of file paths (NOT roots) and return the merged, sorted,
/// deduped candidate set as a JSON array.
#[napi(js_name = "scan")]
pub fn scan(paths_json: String) -> Result<String, Error> {
    let paths: Vec<PathBuf> = serde_json::from_str(&paths_json)
        .map_err(|e| Error::from_reason(format!("invalid paths JSON: {e}")))?;
    let mut set: HashSet<String> = HashSet::new();
    let mut tokens: Vec<String> = Vec::new();
    for path in &paths {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        tokens.clear();
        extract_candidates(&text, &mut tokens);
        for tok in tokens.drain(..) {
            set.insert(tok);
        }
    }
    let mut out: Vec<String> = set.into_iter().collect();
    out.sort();
    serde_json::to_string(&out)
        .map_err(|e| Error::from_reason(format!("serializing scan result: {e}")))
}

/// Walk content roots, tokenize every file, and return a JSON object
/// mapping each file's path to its sorted, deduped candidate list.
#[napi(js_name = "scanPerFile")]
pub fn scan_per_file(roots_json: String) -> Result<String, Error> {
    let roots: Vec<PathBuf> = serde_json::from_str(&roots_json)
        .map_err(|e| Error::from_reason(format!("invalid roots JSON: {e}")))?;
    let walk_roots: Vec<PathBuf> = if roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        roots
    };
    let walk_opts = WalkOptions {
        roots: walk_roots,
        ..Default::default()
    };
    let files = discover_files(&walk_opts)
        .map_err(|e| Error::from_reason(format!("discover_files: {e}")))?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    let mut tokens: Vec<String> = Vec::new();
    for path in &files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        tokens.clear();
        extract_candidates(&text, &mut tokens);
        let mut set: HashSet<String> = HashSet::new();
        for tok in tokens.drain(..) {
            set.insert(tok);
        }
        let mut candidates: Vec<String> = set.into_iter().collect();
        candidates.sort();
        map.insert(path.to_string_lossy().into_owned(), candidates);
    }
    serde_json::to_string(&map)
        .map_err(|e| Error::from_reason(format!("serializing scanPerFile result: {e}")))
}

/// The version of `galeforce-core`, embedded at build time.
#[napi]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// If `options.content` is non-empty, walk those roots, tokenize every
/// file, and merge the dedup'd token set into `options.candidates`.
/// Mirrors the CLI's apply_content_scan — both surfaces honor the
/// `content` field the same way.
fn apply_content_scan(options: &mut CompileOptions) {
    if options.content.is_empty() {
        return;
    }
    let roots: Vec<PathBuf> = options.content.iter().map(PathBuf::from).collect();
    let walk_opts = WalkOptions {
        roots,
        ..Default::default()
    };
    let Ok(files) = discover_files(&walk_opts) else {
        return;
    };
    let mut seen: HashSet<String> = options.candidates.iter().cloned().collect();
    let mut tokens: Vec<String> = Vec::new();
    for path in &files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        tokens.clear();
        extract_candidates(&text, &mut tokens);
        for tok in tokens.drain(..) {
            if seen.insert(tok.clone()) {
                options.candidates.push(tok);
            }
        }
    }
}
