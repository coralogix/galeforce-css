//! Content scanning + incremental candidate cache.
//!
//! See `todo.md` § 10. The scanner has two responsibilities:
//!
//! 1. **Discovery** — walk a content root, respecting `.gitignore` and a
//!    project-tunable include/exclude set, and return the files that should
//!    be tokenized.
//! 2. **Incremental cache** — record which candidates each file produced, so
//!    that a subsequent file edit can produce a precise `(added, removed)`
//!    delta against the global candidate set. The HMR fast path depends on
//!    this delta being empty when no Tailwind-relevant tokens changed.
//!
//! Tokenization itself lives in `tokenize.rs` and is shared between the
//! initial scan and incremental updates.

#![allow(clippy::doc_markdown)]

mod cache;
mod tokenize;
mod walk;

pub use cache::{ScanDelta, Scanner};
pub use tokenize::extract_candidates;
pub use walk::{discover_files, WalkOptions};
