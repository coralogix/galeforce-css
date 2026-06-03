# Performance

GaleforceCSS is faster than Tailwind v3 by a meaningful margin. Numbers below are what to expect on real workloads.

## Real-world baseline

**horizon-tailwind-react** (2,551 candidates → 706 rules):

| | Tailwind 3 CLI | GaleforceCSS CLI | Speedup |
|---|---:|---:|---:|
| Cold build | 246 ms | 10 ms | **23x** |
| Warm import | 15 ms | 2 ms | **7.5x** |

Synthetic smoke (734 candidates):

| | Tailwind 3 CLI | GaleforceCSS CLI | Speedup |
|---|---:|---:|---:|
| Cold build | 195 ms | 5 ms | **38x** |

## Pure compute

The compile loop — no spawn, no I/O — runs in **1.24 ms** on the real corpus.

| Candidate | Cost |
|---|---:|
| `flex` (static) | 0.34 µs |
| Unknown utility | 0.21 µs |
| `bg-red-500` (value lookup) | 0.41 µs |
| `dark:hover:bg-blue-500/50` | 0.41 µs |

## Where the gains come from

1. **`default_theme()` cached in `OnceLock`** — was rebuilding ~12k JSON-tree allocations per build. 245 ms → 16.5 ms (15x).
2. **Prefix-indexed `find_value_utilities`** — replaces a linear walk over ~150 entries with `HashMap<&str, Vec<&'static ValueUtility>>`. 16.5 ms → 2.4 ms (6.8x).
3. **Rayon parallelization** — `par_iter` over candidates when `n ≥ 256`. 2.4 ms → 1.2 ms (1.9x).
4. **Scanner fast path for explicit file paths** — bypass `WalkBuilder` instances when the Vite plugin pre-expands globs. ~16k files: 2.0 s of overhead removed.
5. **Parallel content scan in the CLI** — `par_iter` across 8 cores. 3.9 s → 460 ms on a 16k-file repo.

The hot loop is now ~0.4 µs/candidate for color resolution. Further gains need lower-level work (skipping `String` allocations in the rule emitter, tighter rayon chunking).

## HMR latency

For small edits, the compile loop is rarely the bottleneck:

| Phase | Typical |
|---|---:|
| File watch debounce | 200 ms (fixed) |
| Scanner re-walk | 2–10 ms |
| Compile | 1–5 ms |
| IPC round-trip (CLI bridge) | 1–3 ms |
| Vite HMR send | 5–20 ms (browser-side) |

Levers if you need more:

- **Lower watch debounce.** Default 200 ms in `galeforcecss watch`.
- **Narrow `content` globs.** Fewer files = faster scan.
- **Use `createCompileStream()` in long-running tools.** Amortizes process startup.

## Benchmarking

```bash
pnpm --filter @cx/galeforcecss-conformance bench                                 # synthetic
pnpm --filter @cx/galeforcecss-conformance bench -- --project /path/to/your/app  # real
cargo bench -p galeforce-compiler                                             # pure compute
```

## Memory

The CLI binary uses `mimalloc` — ~25% faster on warm-stream compile vs the system allocator. Peak resident on the real corpus is well under 50 MB.
