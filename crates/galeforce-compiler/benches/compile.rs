//! Plain-Rust benchmark for the compile path. No process spawn, no JSON
//! I/O — measures the actual compute. Drives a frozen corpus from a
//! representative real-world Tailwind v3 project.
//!
//! Run with `cargo bench -p galeforce-compiler` (release profile).

use std::time::Instant;

use galeforce_core::CompileOptions;

const CORPUS_JSON: &str = include_str!("../benches-corpus.json");

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((samples.len() as f64) * p).ceil() as usize;
    samples[idx.min(samples.len() - 1).max(0)]
}

fn bench<F: FnMut()>(label: &str, iters: usize, mut f: F) {
    // Warmup
    f();
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        f();
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let sum: f64 = samples.iter().sum();
    let mean = sum / samples.len() as f64;
    let min = samples.iter().cloned().fold(f64::INFINITY, f64::min);
    let p95 = percentile(&mut samples, 0.95);
    println!("  {label:<48}  mean={mean:>8.2}ms  min={min:>8.2}ms  p95={p95:>8.2}ms",);
}

fn main() {
    let candidates: Vec<String> = serde_json::from_str(CORPUS_JSON).expect("corpus JSON");
    println!("bench: {} candidates", candidates.len());
    println!();

    let input_css = "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n".to_string();
    let config = serde_json::json!({ "darkMode": "class" });

    // ---- full real-world workload ----
    let opts = CompileOptions {
        candidates: candidates.clone(),
        input_css: Some(input_css.clone()),
        config: Some(config.clone()),
        ..Default::default()
    };
    bench("full workload (real corpus)", 20, || {
        let _ = galeforce_compiler::compile(&opts);
    });

    // ---- isolate phases ----
    let opts_no_inputcss = CompileOptions {
        candidates: candidates.clone(),
        input_css: None,
        config: Some(config.clone()),
        ..Default::default()
    };
    bench("workload, no input CSS (skip directive proc)", 20, || {
        let _ = galeforce_compiler::compile(&opts_no_inputcss);
    });

    let opts_no_config = CompileOptions {
        candidates: candidates.clone(),
        input_css: None,
        config: None,
        ..Default::default()
    };
    bench("workload, no config", 20, || {
        let _ = galeforce_compiler::compile(&opts_no_config);
    });

    // ---- shape buckets ----
    let static_only: Vec<String> = vec!["flex"; 1000].into_iter().map(String::from).collect();
    let opts_static = CompileOptions {
        candidates: static_only,
        input_css: None,
        config: None,
        ..Default::default()
    };
    bench("1000x static `flex`", 20, || {
        let _ = galeforce_compiler::compile(&opts_static);
    });

    let color_only: Vec<String> = vec!["bg-red-500"; 1000]
        .into_iter()
        .map(String::from)
        .collect();
    let opts_color = CompileOptions {
        candidates: color_only,
        input_css: None,
        config: None,
        ..Default::default()
    };
    bench("1000x value `bg-red-500`", 20, || {
        let _ = galeforce_compiler::compile(&opts_color);
    });

    let variant_color: Vec<String> = vec!["dark:hover:bg-blue-500/50"; 1000]
        .into_iter()
        .map(String::from)
        .collect();
    let opts_var = CompileOptions {
        candidates: variant_color,
        input_css: None,
        config: Some(config.clone()),
        ..Default::default()
    };
    bench("1000x `dark:hover:bg-blue-500/50`", 20, || {
        let _ = galeforce_compiler::compile(&opts_var);
    });

    let unknown: Vec<String> = vec!["AbortController"; 1000]
        .into_iter()
        .map(String::from)
        .collect();
    let opts_unknown = CompileOptions {
        candidates: unknown,
        input_css: None,
        config: None,
        ..Default::default()
    };
    bench("1000x unknown `AbortController`", 20, || {
        let _ = galeforce_compiler::compile(&opts_unknown);
    });
}
