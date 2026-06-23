// Copyright 2026 Coralogix Ltd.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! `galeforcecss` command-line interface.
//!
//! Subcommands:
//!
//! - `compile-json` — single-shot. Read a `CompileOptions` JSON object on
//!   stdin, run the compiler, write a `CompileResult` JSON object on
//!   stdout. The shape is stable because the conformance harness depends
//!   on it.
//!
//! - `compile-stream` — long-running. Read JSON-lines `CompileOptions`
//!   on stdin, write JSON-lines `CompileResult` on stdout. Used by the
//!   public package + the Vite plugin so per-call process startup
//!   doesn't dominate latency on HMR.
//!
//! - `build` — read user CSS input from `--input`, scan content globs
//!   from `--content` (or default to ./), compile, write to `--output`
//!   (or stdout). Mirrors `tailwindcss --input/--output/--content`.
//!
//! - `scan` — print extracted candidates from content globs. Useful
//!   for debugging tokenizer behavior on a real project.
//!
//! `compare` lives on the JS side (`scripts/galeforcecss-compare.ts`) — it
//! needs the oracle compiler, which is a Node import.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;

// mimalloc is dramatically faster than the system allocator on
// allocation-heavy workloads. The compile loop allocates ~10–15
// strings per candidate (parsed root, class form, declarations,
// diagnostics) — for a 2,551-candidate corpus that's ~30k mallocs
// per build. Wins ~25% on warm-stream compile vs the default
// allocator on macOS.
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use galeforce_core::{CompileOptions, CompileResult};
use galeforce_scanner::{discover_files, extract_candidates, WalkOptions};

#[derive(Parser, Debug)]
#[command(
    name = "galeforcecss",
    version,
    about = "Tailwind CSS v3-compatible compiler",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Read CompileOptions JSON on stdin, write CompileResult JSON on stdout.
    CompileJson,
    /// Read JSONL CompileOptions on stdin, write JSONL CompileResult on stdout.
    /// Each line on stdin produces exactly one line on stdout. Closes when
    /// stdin EOFs.
    CompileStream,
    /// One-shot build: scan content, compile, write CSS to a file or stdout.
    Build(BuildArgs),
    /// Like `build`, but watches `--input` + `--config` + `--content` paths
    /// and re-compiles on change.
    Watch(WatchArgs),
    /// Print extracted candidates from content globs.
    Scan(ScanArgs),
    /// Scaffold a starter `tailwind.config.js` + entry CSS in the current
    /// directory.
    Init(InitArgs),
    /// Reflection: print every known utility class name as JSON.
    /// Used by editor extensions for autocomplete.
    ListClasses(ListArgs),
    /// Reflection: print every known variant name as JSON.
    ListVariants(ListArgs),
}

#[derive(Args, Debug)]
struct BuildArgs {
    /// Path to the user's input CSS (the file containing `@tailwind …`).
    #[arg(short = 'i', long)]
    input: Option<PathBuf>,
    /// Path to write the compiled CSS to. Stdout if omitted.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Resolved-config JSON file. Produced by the JS-side config loader.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Content roots to scan for candidates. May be repeated. Defaults to `.`.
    #[arg(long)]
    content: Vec<PathBuf>,
    /// Print diagnostics + scan/compile stats to stderr.
    #[arg(long)]
    verbose: bool,
    /// Write diagnostics as JSON to the given path (or `-` for stderr).
    /// Useful for CI integration. Each entry has `severity`, `code`, `message`.
    #[arg(long, value_name = "PATH")]
    diagnostics: Option<String>,
    /// Minify the output CSS.
    #[arg(long)]
    minify: bool,
}

#[derive(Args, Debug)]
struct ScanArgs {
    /// Content roots to scan. May be repeated. Defaults to `.`.
    #[arg(long)]
    content: Vec<PathBuf>,
    /// Print one candidate per line (default). When `--json` is set,
    /// emit a JSON array instead.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct WatchArgs {
    /// Path to the user's input CSS (the file containing `@tailwind …`).
    #[arg(short = 'i', long)]
    input: Option<PathBuf>,
    /// Path to write the compiled CSS to. Stdout if omitted.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Resolved-config JSON file. Produced by the JS-side config loader.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Content roots to scan for candidates. May be repeated. Defaults to `.`.
    #[arg(long)]
    content: Vec<PathBuf>,
    /// Print diagnostics + scan/compile stats to stderr.
    #[arg(long)]
    verbose: bool,
    /// Write diagnostics as JSON to the given path (or `-` for stderr).
    #[arg(long, value_name = "PATH")]
    diagnostics: Option<String>,
    /// Minify the output CSS.
    #[arg(long)]
    minify: bool,
}

#[derive(Args, Debug)]
struct ListArgs {
    /// Resolved config JSON file (produced by the JS-side loader).
    /// When supplied, plugin-defined classes / variants from
    /// `__pluginOutput` are merged into the listing.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct InitArgs {
    /// Overwrite files if they already exist. Without `--force`, init
    /// refuses to clobber an existing `tailwind.config.js` or input CSS.
    #[arg(long)]
    force: bool,
    /// Path for the input CSS file. Defaults to `src/index.css`.
    #[arg(long, default_value = "src/index.css")]
    input: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::CompileJson => cmd_compile_json(),
        Cmd::CompileStream => cmd_compile_stream(),
        Cmd::Build(args) => cmd_build(args),
        Cmd::Watch(args) => cmd_watch(args),
        Cmd::Scan(args) => cmd_scan(args),
        Cmd::Init(args) => cmd_init(args),
        Cmd::ListClasses(args) => cmd_list(args, true),
        Cmd::ListVariants(args) => cmd_list(args, false),
    }
}

fn cmd_list(args: ListArgs, classes: bool) -> Result<()> {
    let config = match &args.config {
        Some(p) => {
            let text = fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            let v: serde_json::Value =
                serde_json::from_str(&text).with_context(|| format!("parsing {}", p.display()))?;
            Some(v)
        }
        None => None,
    };
    let names = if classes {
        galeforce_compiler::list_class_names(config.as_ref())
    } else {
        galeforce_compiler::list_variant_names(config.as_ref())
    };
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer(&mut handle, &names)?;
    handle.write_all(b"\n")?;
    Ok(())
}

fn cmd_compile_json() -> Result<()> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .context("reading stdin")?;
    let mut options: CompileOptions =
        serde_json::from_str(&buf).context("parsing CompileOptions JSON")?;
    apply_content_scan(&mut options);
    let result: CompileResult = galeforce_compiler::compile(&options);

    let stdout = io::stdout();
    let mut handle = io::BufWriter::with_capacity(64 * 1024, stdout.lock());
    // `to_writer` streams directly to stdout without allocating an
    // intermediate String. For 295KB-class outputs that's a meaningful
    // chunk of pure copying we no longer pay for.
    serde_json::to_writer(&mut handle, &result).context("serializing CompileResult")?;
    handle.write_all(b"\n")?;
    handle.flush()?;
    Ok(())
}

fn cmd_compile_stream() -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line.context("reading stdin line")?;
        if line.trim().is_empty() {
            continue;
        }

        // Parse as a generic JSON value first so we can branch on `type`.
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                // Emit a synthetic error result so the consumer's
                // line-based parser stays in sync. We can't recover the
                // original request id (we don't have one yet), so the
                // consumer matches by index.
                let err = serde_json::json!({
                    "output": { "css": "", "map": null },
                    "diagnostics": [{
                        "severity": "error",
                        "code": "invalid-json",
                        "message": format!("invalid JSON: {e}"),
                    }],
                    "candidateCount": 0,
                    "ruleCount": 0,
                });
                writeln!(handle, "{err}")?;
                handle.flush()?;
                continue;
            }
        };

        match value.get("type").and_then(|t| t.as_str()) {
            Some("scan") => {
                // Flat list of file paths to scan.
                let paths: Vec<PathBuf> = value
                    .get("content")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .map(PathBuf::from)
                            .collect()
                    })
                    .unwrap_or_default();
                let candidates = collect_candidates(&paths).unwrap_or_default();
                let resp = serde_json::json!({ "type": "scan", "candidates": candidates });
                writeln!(handle, "{resp}")?;
                handle.flush()?;
            }
            Some("scan-per-file") => {
                // Content roots to walk; respond with per-file candidate map.
                let roots: Vec<PathBuf> = value
                    .get("content")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .map(PathBuf::from)
                            .collect()
                    })
                    .unwrap_or_default();
                let bench = std::env::var("GALEFORCE_BENCH_SCAN").is_ok();
                let t_scan = std::time::Instant::now();
                let files = collect_candidates_per_file(&roots).unwrap_or_default();
                if bench {
                    eprintln!(
                        "[galeforce:rust] scan {} files in {:.0}ms",
                        files.len(),
                        t_scan.elapsed().as_secs_f64() * 1000.0
                    );
                }
                let t_json = std::time::Instant::now();
                let resp = serde_json::json!({ "type": "scan-per-file", "files": files });
                writeln!(handle, "{resp}")?;
                handle.flush()?;
                if bench {
                    eprintln!(
                        "[galeforce:rust] json+write {:.0}ms",
                        t_json.elapsed().as_secs_f64() * 1000.0
                    );
                }
            }
            _ => {
                let bench = std::env::var("GALEFORCE_BENCH_COMPILE").is_ok();
                let t_parse = std::time::Instant::now();
                // Treat as CompileOptions (backward-compat: no `type` field).
                let mut options: CompileOptions = match serde_json::from_value(value) {
                    Ok(o) => o,
                    Err(e) => {
                        let err = serde_json::json!({
                            "output": { "css": "", "map": null },
                            "diagnostics": [{
                                "severity": "error",
                                "code": "invalid-json",
                                "message": format!("invalid CompileOptions: {e}"),
                            }],
                            "candidateCount": 0,
                            "ruleCount": 0,
                        });
                        writeln!(handle, "{err}")?;
                        handle.flush()?;
                        continue;
                    }
                };
                let parse_ms = t_parse.elapsed().as_secs_f64() * 1000.0;
                apply_content_scan(&mut options);
                let t_compile = std::time::Instant::now();
                let result = galeforce_compiler::compile(&options);
                let compile_ms = t_compile.elapsed().as_secs_f64() * 1000.0;
                let t_write = std::time::Instant::now();
                // Stream straight to stdout — saves the intermediate String
                // allocation for 295KB-class CSS payloads.
                serde_json::to_writer(&mut handle, &result).context("serializing CompileResult")?;
                handle.write_all(b"\n")?;
                handle.flush()?;
                if bench {
                    let write_ms = t_write.elapsed().as_secs_f64() * 1000.0;
                    eprintln!(
                        "[galeforce:rust] compile parse={:.0}ms compile={:.0}ms write={:.0}ms cands={} input={}B output={}B",
                        parse_ms,
                        compile_ms,
                        write_ms,
                        options.candidates.len(),
                        options.input_css.as_deref().map(str::len).unwrap_or(0),
                        result.output.css.len(),
                    );
                }
            }
        }
    }
    Ok(())
}

/// Walk `roots` (or `.` if empty), tokenize every file, and return a map
/// from canonical path string to the sorted, deduped candidate list for
/// that file. Files that cannot be decoded as UTF-8 are silently skipped.
fn collect_candidates_per_file(roots: &[PathBuf]) -> Result<HashMap<String, Vec<String>>> {
    use rayon::prelude::*;
    let bench = std::env::var("GALEFORCE_BENCH_SCAN").is_ok();
    let walk_roots: Vec<PathBuf> = if roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        roots.to_vec()
    };
    let walk_opts = WalkOptions {
        roots: walk_roots,
        ..Default::default()
    };
    let t_walk = std::time::Instant::now();
    let files = discover_files(&walk_opts).map_err(|e| anyhow::anyhow!(e))?;
    if bench {
        eprintln!(
            "[galeforce:rust]   discover {} files in {:.0}ms",
            files.len(),
            t_walk.elapsed().as_secs_f64() * 1000.0
        );
    }
    let t_scan = std::time::Instant::now();
    // Parallel read + tokenize. On a typical large monorepo
    // (~16k content files) this drops a single-threaded ~3.9s scan
    // to ~600ms on an 8-core machine. The serialize-to-HashMap step
    // runs on the main thread (cheap) after each worker returns its
    // (path, candidates) pair.
    let entries: Vec<(String, Vec<String>)> = files
        .par_iter()
        .filter_map(|path| {
            let text = fs::read_to_string(path).ok()?;
            let mut tokens: Vec<String> = Vec::new();
            extract_candidates(&text, &mut tokens);
            let set: HashSet<String> = tokens.into_iter().collect();
            let mut candidates: Vec<String> = set.into_iter().collect();
            candidates.sort();
            Some((path.to_string_lossy().into_owned(), candidates))
        })
        .collect();
    if bench {
        eprintln!(
            "[galeforce:rust]   read+tokenize {:.0}ms",
            t_scan.elapsed().as_secs_f64() * 1000.0
        );
    }
    let t_map = std::time::Instant::now();
    let mut map: HashMap<String, Vec<String>> = HashMap::with_capacity(entries.len());
    for (k, v) in entries {
        map.insert(k, v);
    }
    if bench {
        eprintln!(
            "[galeforce:rust]   build map {:.0}ms",
            t_map.elapsed().as_secs_f64() * 1000.0
        );
    }
    Ok(map)
}

/// If `options.content` is non-empty, walk those roots, tokenize every
/// file, and merge the dedup'd token set into `options.candidates`. The
/// caller's pre-supplied candidates take priority — they remain at the
/// front of the list and aren't duplicated. Errors during walking are
/// silent: the scanner is best-effort.
fn apply_content_scan(options: &mut CompileOptions) {
    if options.content.is_empty() {
        return;
    }
    let roots: Vec<PathBuf> = options.content.iter().map(PathBuf::from).collect();
    let scanned = match collect_candidates(&roots) {
        Ok(v) => v,
        Err(_) => return,
    };
    let mut seen: HashSet<String> = options.candidates.iter().cloned().collect();
    for tok in scanned {
        if seen.insert(tok.clone()) {
            options.candidates.push(tok);
        }
    }
}

fn cmd_build(args: BuildArgs) -> Result<()> {
    do_one_build(
        args.input.as_deref(),
        args.output.as_deref(),
        args.config.as_deref(),
        &args.content,
        args.verbose,
        args.minify,
        args.diagnostics.as_deref(),
    )
}

/// Shared body for `build` + each tick of `watch`. Reads the input CSS
/// and config (if present), scans content roots for candidates, compiles,
/// and emits to `output_path` or stdout. Returns the rule count for the
/// caller to log when running in watch mode.
fn do_one_build(
    input_path: Option<&std::path::Path>,
    output_path: Option<&std::path::Path>,
    config_path: Option<&std::path::Path>,
    content: &[PathBuf],
    verbose: bool,
    minify: bool,
    diagnostics: Option<&str>,
) -> Result<()> {
    let debug = is_debug_env();
    let t_total = std::time::Instant::now();

    let input_css = match input_path {
        Some(p) => Some(fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?),
        None => None,
    };

    let config = match config_path {
        Some(p) => {
            let text =
                fs::read_to_string(p).with_context(|| format!("reading config {}", p.display()))?;
            let v: serde_json::Value = serde_json::from_str(&text)
                .with_context(|| format!("parsing config {}", p.display()))?;
            Some(v)
        }
        None => None,
    };

    let t_scan = std::time::Instant::now();
    let candidates = collect_candidates(content)?;
    let scan_elapsed = t_scan.elapsed();

    // Mirror Tailwind's stderr behavior: when the content roots produce
    // zero candidates the user almost always misconfigured `content`.
    // Print a single warning to stderr so the issue is visible without
    // requiring `--verbose`.
    if candidates.is_empty() && !content.is_empty() {
        let mut stderr = io::stderr().lock();
        writeln!(
            stderr,
            "galeforcecss: warning — no utility classes were detected in your content. Double-check your `content` paths."
        )
        .ok();
    }

    let t_compile = std::time::Instant::now();
    let opts = CompileOptions {
        input_css,
        config,
        candidates,
        features: galeforce_core::FeatureFlags {
            minify,
            ..Default::default()
        },
        ..Default::default()
    };
    let result = galeforce_compiler::compile(&opts);
    let compile_elapsed = t_compile.elapsed();

    if verbose {
        let mut stderr = io::stderr().lock();
        for d in &result.diagnostics {
            writeln!(stderr, "[{}] {} ({})", d.severity_str(), d.message, d.code).ok();
        }
        writeln!(
            stderr,
            "  -> {} candidate(s), {} rule(s)",
            result.candidate_count, result.rule_count
        )
        .ok();
    }

    if debug {
        let mut stderr = io::stderr().lock();
        writeln!(
            stderr,
            "[galeforce-debug] scan={:?} compile={:?} total={:?} candidates={} rules={}",
            scan_elapsed,
            compile_elapsed,
            t_total.elapsed(),
            result.candidate_count,
            result.rule_count,
        )
        .ok();
    }

    if let Some(target) = diagnostics {
        emit_diagnostics_json(target, &result.diagnostics)?;
    }

    match output_path {
        Some(p) => {
            if let Some(parent) = p.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("creating output dir {}", parent.display()))?;
                }
            }
            fs::write(p, &result.output.css).with_context(|| format!("writing {}", p.display()))?;
        }
        None => {
            io::stdout()
                .lock()
                .write_all(result.output.css.as_bytes())?;
        }
    }
    Ok(())
}

/// Write the compile result's diagnostics as a JSON array. `target` is
/// either a filesystem path or `-` for stderr. Each diagnostic surfaces
/// as `{ "severity": …, "code": …, "message": … }` — the same shape the
/// JSON-mode subcommands emit, so tooling can parse them uniformly.
fn emit_diagnostics_json(target: &str, diagnostics: &[galeforce_core::Diagnostic]) -> Result<()> {
    let entries: Vec<serde_json::Value> = diagnostics
        .iter()
        .map(|d| {
            serde_json::json!({
                "severity": d.severity_str(),
                "code": d.code,
                "message": d.message,
            })
        })
        .collect();
    let json = serde_json::to_string_pretty(&entries)?;
    if target == "-" {
        let mut stderr = io::stderr().lock();
        stderr.write_all(json.as_bytes())?;
        stderr.write_all(b"\n")?;
    } else {
        let path = std::path::Path::new(target);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating dir {}", parent.display()))?;
            }
        }
        fs::write(path, json)
            .with_context(|| format!("writing diagnostics to {}", path.display()))?;
    }
    Ok(())
}

fn is_debug_env() -> bool {
    matches!(
        std::env::var("GALEFORCE_DEBUG").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

fn cmd_watch(args: WatchArgs) -> Result<()> {
    use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, DebouncedEventKind};
    use std::sync::mpsc::channel;
    use std::time::Duration;

    // Initial build.
    let mut stderr = io::stderr().lock();
    let started = std::time::Instant::now();
    if let Err(e) = do_one_build(
        args.input.as_deref(),
        args.output.as_deref(),
        args.config.as_deref(),
        &args.content,
        args.verbose,
        args.minify,
        args.diagnostics.as_deref(),
    ) {
        writeln!(stderr, "galeforcecss watch: initial build failed: {e:#}")?;
    } else {
        writeln!(
            stderr,
            "galeforcecss watch: built in {:?}",
            started.elapsed()
        )?;
    }

    // Set up the debounced watcher. 200 ms collapses keystroke-rate
    // saves into a single event without feeling laggy.
    let (tx, rx) = channel();
    let mut debouncer = new_debouncer(Duration::from_millis(200), move |res| {
        let _ = tx.send(res);
    })
    .context("starting filesystem watcher")?;

    // Watch:
    // - the input CSS file (if set)
    // - the config file (if set)
    // - each content root (recursively)
    let watcher = debouncer.watcher();
    if let Some(p) = &args.input {
        watcher
            .watch(p, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching {}", p.display()))?;
    }
    if let Some(p) = &args.config {
        watcher
            .watch(p, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching {}", p.display()))?;
    }
    let content_roots: Vec<PathBuf> = if args.content.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.content.clone()
    };
    for root in &content_roots {
        watcher
            .watch(root, RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", root.display()))?;
    }
    writeln!(
        stderr,
        "galeforcecss watch: watching for changes (Ctrl-C to exit)"
    )?;
    drop(stderr);

    // Loop on debounced events. Each batch may contain multiple paths
    // — we don't care which fired, just that something changed and a
    // re-compile is in order.
    while let Ok(events) = rx.recv() {
        let events = match events {
            Ok(v) => v,
            Err(err) => {
                let mut stderr = io::stderr().lock();
                writeln!(stderr, "galeforcecss watch: {err}")?;
                continue;
            }
        };
        if !events
            .iter()
            .any(|e| matches!(e.kind, DebouncedEventKind::Any))
        {
            continue;
        }
        let mut stderr = io::stderr().lock();
        let started = std::time::Instant::now();
        match do_one_build(
            args.input.as_deref(),
            args.output.as_deref(),
            args.config.as_deref(),
            &args.content,
            args.verbose,
            args.minify,
            args.diagnostics.as_deref(),
        ) {
            Ok(()) => writeln!(
                stderr,
                "galeforcecss watch: rebuilt in {:?}",
                started.elapsed()
            )?,
            Err(e) => writeln!(stderr, "galeforcecss watch: build failed: {e:#}")?,
        }
    }
    Ok(())
}

fn cmd_init(args: InitArgs) -> Result<()> {
    // Two artifacts: a tailwind config (so users with the
    // `@coralogix/galeforcecss-config-loader` get something to load) and an entry
    // CSS file (the natural target of `galeforcecss build -i`).
    let cwd = std::env::current_dir().context("getting cwd")?;
    let config_path = cwd.join("tailwind.config.js");
    let input_path = cwd.join(&args.input);

    let config_body = "/** @type {import('tailwindcss').Config} */\n\
        module.exports = {\n\
        \x20\x20content: [\n\
        \x20\x20\x20\x20'./src/**/*.{html,js,jsx,ts,tsx,vue,svelte}',\n\
        \x20\x20\x20\x20'./index.html',\n\
        \x20\x20],\n\
        \x20\x20theme: {\n\
        \x20\x20\x20\x20extend: {},\n\
        \x20\x20},\n\
        \x20\x20plugins: [],\n\
        }\n";
    let input_body = "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n";

    write_or_skip(&config_path, config_body, args.force)?;
    write_or_skip(&input_path, input_body, args.force)?;

    let mut stderr = io::stderr().lock();
    writeln!(
        stderr,
        "galeforcecss init: scaffolded {} and {}",
        config_path.display(),
        input_path.display()
    )?;
    writeln!(
        stderr,
        "galeforcecss init: next step → `galeforcecss build -i {} -o dist/output.css --content src`",
        args.input.display()
    )?;
    Ok(())
}

fn write_or_skip(path: &std::path::Path, body: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        let mut stderr = io::stderr().lock();
        writeln!(
            stderr,
            "galeforcecss init: {} already exists; pass --force to overwrite",
            path.display()
        )?;
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating dir {}", parent.display()))?;
        }
    }
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn cmd_scan(args: ScanArgs) -> Result<()> {
    let candidates = collect_candidates(&args.content)?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    if args.json {
        let json = serde_json::to_string(&candidates)?;
        handle.write_all(json.as_bytes())?;
        handle.write_all(b"\n")?;
    } else {
        for c in &candidates {
            writeln!(handle, "{c}")?;
        }
    }
    Ok(())
}

/// Walk `roots` (or `.` if empty), tokenize every discovered file, dedupe.
/// The tokenizer over-extracts on purpose — that's correct; the parser
/// rejects non-utility tokens. We dedupe to keep the candidate set
/// compact.
fn collect_candidates(roots: &[PathBuf]) -> Result<Vec<String>> {
    use rayon::prelude::*;
    let walk_roots: Vec<PathBuf> = if roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        roots.to_vec()
    };
    let walk_opts = WalkOptions {
        roots: walk_roots,
        ..Default::default()
    };
    let files = discover_files(&walk_opts).map_err(|e| anyhow::anyhow!(e))?;
    // Parallel read + tokenize → per-worker dedup HashSet → final
    // reduce into a single sorted Vec. Each worker reduces its own
    // sub-batch into a HashSet before merging to keep contention off
    // the final set.
    let set: HashSet<String> = files
        .par_iter()
        .fold(HashSet::new, |mut acc, path| {
            // Skip files we can't decode as UTF-8 (binary assets snuck
            // through an extension allow-list). Best-effort.
            if let Ok(text) = fs::read_to_string(path) {
                let mut tokens: Vec<String> = Vec::new();
                extract_candidates(&text, &mut tokens);
                for tok in tokens {
                    acc.insert(tok);
                }
            }
            acc
        })
        .reduce(HashSet::new, |mut a, mut b| {
            if a.len() < b.len() {
                std::mem::swap(&mut a, &mut b);
            }
            a.extend(b.drain());
            a
        });
    let mut out: Vec<String> = set.into_iter().collect();
    out.sort();
    Ok(out)
}

trait DiagSeverityExt {
    fn severity_str(&self) -> &'static str;
}
impl DiagSeverityExt for galeforce_core::Diagnostic {
    fn severity_str(&self) -> &'static str {
        match self.severity {
            galeforce_core::Severity::Error => "error",
            galeforce_core::Severity::Warning => "warning",
            galeforce_core::Severity::Info => "info",
        }
    }
}
