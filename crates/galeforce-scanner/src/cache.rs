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

//! Incremental candidate cache.
//!
//! The cache lets the dev-server skip CSS rebuilds when a file edit doesn't
//! change the global candidate set. Each tracked file owns a multiset of
//! candidates; the cache maintains refcounts so removing one file doesn't
//! evict candidates that other files still reference.
//!
//! The HMR fast path is:
//! ```text
//! delta = scanner.update_file(path, contents)
//! if !delta.changed_global_set() {
//!     // No new utilities, no removed utilities — keep the previous CSS.
//! } else {
//!     compile_with_galeforce(scanner.candidates())
//! }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

use crate::tokenize::extract_candidates;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScanDelta {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl ScanDelta {
    pub fn changed_global_set(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty()
    }
}

#[derive(Debug, Default)]
struct FileEntry {
    /// Candidate string -> count within this file. Re-used as the source of
    /// truth when the file is updated or removed.
    candidates: HashMap<String, u32>,
}

#[derive(Default)]
pub struct Scanner {
    files: FxHashMap<PathBuf, FileEntry>,
    /// Global refcount: how many file-level occurrences reference each
    /// candidate across all tracked files. A candidate is "in the global set"
    /// iff its refcount is non-zero.
    global: FxHashMap<String, u32>,
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Iterate every candidate currently in the global set, in arbitrary
    /// order. Use `candidates_sorted` when determinism matters.
    pub fn candidates(&self) -> impl Iterator<Item = &str> {
        self.global.keys().map(String::as_str)
    }

    /// Snapshot the candidate set as a sorted Vec. Useful for compile input
    /// and tests.
    pub fn candidates_sorted(&self) -> Vec<String> {
        let mut v: Vec<String> = self.global.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn tracked_file_count(&self) -> usize {
        self.files.len()
    }

    /// Add or replace the candidates contributed by `path` based on `source`.
    /// Returns the delta against the global set.
    pub fn update_file(&mut self, path: impl AsRef<Path>, source: &str) -> ScanDelta {
        let path = path.as_ref().to_path_buf();
        let mut new_entry = FileEntry::default();
        {
            let mut tokens = Vec::new();
            extract_candidates(source, &mut tokens);
            for tok in tokens {
                *new_entry.candidates.entry(tok).or_insert(0) += 1;
            }
        }
        self.replace_entry(path, new_entry)
    }

    /// Drop a file from the cache, releasing its contributions.
    pub fn remove_file(&mut self, path: impl AsRef<Path>) -> ScanDelta {
        let path = path.as_ref().to_path_buf();
        self.replace_entry(path, FileEntry::default())
    }

    fn replace_entry(&mut self, path: PathBuf, new_entry: FileEntry) -> ScanDelta {
        let old_entry = self.files.remove(&path).unwrap_or_default();

        // Compute net change per candidate first, *then* apply it. If we
        // decremented and incremented in two separate passes, candidates
        // present in both the old and new entries would briefly hit 0 and
        // get reported as removed-then-added — a HMR pessimization that
        // would force unnecessary rebuilds.
        let mut net: HashMap<String, i64> = HashMap::new();
        for (cand, count) in old_entry.candidates {
            *net.entry(cand).or_insert(0) -= i64::from(count);
        }
        for (cand, count) in &new_entry.candidates {
            *net.entry(cand.clone()).or_insert(0) += i64::from(*count);
        }

        let mut delta = ScanDelta::default();
        for (cand, change) in net {
            if change == 0 {
                continue;
            }
            let old = self.global.get(&cand).copied().unwrap_or(0);
            let signed_new = i64::from(old) + change;
            let new = if signed_new < 0 { 0 } else { signed_new as u32 };

            if old == 0 && new > 0 {
                delta.added.push(cand.clone());
            } else if old > 0 && new == 0 {
                delta.removed.push(cand.clone());
            }

            if new == 0 {
                self.global.remove(&cand);
            } else {
                self.global.insert(cand, new);
            }
        }

        if !new_entry.candidates.is_empty() {
            self.files.insert(path, new_entry);
        }
        delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn first_update_is_all_added() {
        let mut s = Scanner::new();
        let delta = s.update_file("a.html", r#"<div class="flex hover:bg-red-500"></div>"#);
        assert!(delta.added.contains(&"flex".to_string()));
        assert!(delta.added.contains(&"hover:bg-red-500".to_string()));
        assert!(delta.removed.is_empty());
        assert_eq!(s.tracked_file_count(), 1);
    }

    #[test]
    fn no_op_edit_yields_empty_delta() {
        let mut s = Scanner::new();
        let _ = s.update_file("a.html", r#"<div class="flex"></div>"#);
        let delta = s.update_file("a.html", r#"<div class="flex"></div>"#);
        assert!(!delta.changed_global_set(), "delta = {delta:?}");
    }

    #[test]
    fn whitespace_only_edit_yields_empty_delta() {
        let mut s = Scanner::new();
        let _ = s.update_file("a.html", r#"<div class="flex"></div>"#);
        // Same tokens, different surrounding bytes. Global set unchanged.
        let delta = s.update_file("a.html", r#"<div    class="flex"   ></div>"#);
        assert!(!delta.changed_global_set(), "delta = {delta:?}");
    }

    #[test]
    fn adding_a_class_reports_only_that_class() {
        let mut s = Scanner::new();
        let _ = s.update_file("a.html", r#"<div class="flex"></div>"#);
        // Adding only `grid` should net-add only `grid` — `div`, `class`, and
        // `flex` are still present and unchanged. The cache must not report
        // them as removed-then-added across the edit.
        let delta = s.update_file("a.html", r#"<div class="flex grid"></div>"#);
        assert_eq!(delta.added, vec!["grid".to_string()]);
        assert!(delta.removed.is_empty(), "removed = {:?}", delta.removed);
    }

    #[test]
    fn removing_a_class_only_drops_at_refcount_zero() {
        let mut s = Scanner::new();
        let _ = s.update_file("a.html", r#"<div class="flex"></div>"#);
        let _ = s.update_file("b.html", r#"<div class="flex"></div>"#);

        // Removing `flex` from a.html (replacing markup with one that has no
        // `flex` token but still has `div` etc.) keeps `flex` in the global
        // set — refcount drops from 2 -> 1, never crossing zero.
        let delta = s.update_file("a.html", r#"<div></div>"#);
        assert!(!delta.removed.contains(&"flex".to_string()));

        // Removing it from b.html now drops the global refcount to 0.
        let delta = s.update_file("b.html", r#"<div></div>"#);
        assert!(delta.removed.contains(&"flex".to_string()));
    }

    #[test]
    fn remove_file_releases_candidates() {
        let mut s = Scanner::new();
        let _ = s.update_file("a.html", r#"<div class="flex hover:bg-red-500"></div>"#);
        let delta = s.remove_file("a.html");
        assert!(delta.removed.contains(&"flex".to_string()));
        assert!(delta.removed.contains(&"hover:bg-red-500".to_string()));
        assert_eq!(s.tracked_file_count(), 0);
        assert!(s.candidates_sorted().is_empty());
    }

    #[test]
    fn multiple_occurrences_in_one_file_count_as_one_added_event() {
        let mut s = Scanner::new();
        // `flex` mentioned twice in the same file: the global set transitions
        // from 0 -> 2 in one shot, so `added` reports it exactly once.
        let delta = s.update_file("a.html", r#"<div class="flex"></div><span class="flex">"#);
        let added_flex = delta.added.iter().filter(|c| c.as_str() == "flex").count();
        assert_eq!(added_flex, 1);
        let delta = s.update_file("a.html", r#"<p></p>"#);
        let removed_flex = delta
            .removed
            .iter()
            .filter(|c| c.as_str() == "flex")
            .count();
        assert_eq!(removed_flex, 1);
    }

    #[test]
    fn deltas_disjoint_added_and_removed() {
        let mut s = Scanner::new();
        let _ = s.update_file("a.html", r#"<div class="flex"></div>"#);
        // Replacing `flex` with `grid`. `grid` should be added, `flex`
        // removed; nothing in both lists.
        let delta = s.update_file("a.html", r#"<div class="grid"></div>"#);
        let added = sorted(delta.added.clone());
        let removed = sorted(delta.removed.clone());
        for c in &added {
            assert!(
                !removed.contains(c),
                "candidate `{c}` in both added and removed"
            );
        }
        assert!(added.contains(&"grid".to_string()));
        assert!(removed.contains(&"flex".to_string()));
    }

    #[test]
    fn empty_replacement_is_remove() {
        let mut s = Scanner::new();
        let _ = s.update_file("a.html", r#"<div class="flex grid"></div>"#);
        let delta = s.update_file("a.html", "");
        assert!(delta.removed.contains(&"flex".to_string()));
        assert!(delta.removed.contains(&"grid".to_string()));
        assert_eq!(s.tracked_file_count(), 0);
    }
}
