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
