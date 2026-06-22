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

//! Integration tests for `discover_files`. Uses tempfile so each test gets
//! an isolated tree.

use std::fs;
use std::path::PathBuf;

use galeforce_scanner::{discover_files, WalkOptions};
use tempfile::tempdir;

fn touch(root: &std::path::Path, rel: &str, contents: &str) -> PathBuf {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
    path
}

fn scan(opts: &WalkOptions) -> Vec<PathBuf> {
    discover_files(opts).unwrap()
}

#[test]
fn discovers_default_extensions() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    touch(root, "src/App.tsx", "<div className=\"flex\"></div>");
    touch(root, "src/page.html", "<div class=\"grid\"></div>");
    touch(root, "src/data.json", r#"{"a": 1}"#);

    let opts = WalkOptions {
        roots: vec![root.to_path_buf()],
        ..Default::default()
    };
    let files: Vec<String> = scan(&opts)
        .into_iter()
        .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
        .collect();

    assert!(files.iter().any(|f| f.ends_with("App.tsx")));
    assert!(files.iter().any(|f| f.ends_with("page.html")));
    assert!(!files.iter().any(|f| f.ends_with("data.json")));
}

#[test]
fn prunes_node_modules_even_when_not_gitignored() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    // Note: no `.gitignore` — DEFAULT_PRUNE_DIRS still applies.
    touch(root, "src/a.html", "");
    touch(root, "node_modules/foo/bar.html", "");

    let opts = WalkOptions {
        roots: vec![root.to_path_buf()],
        respect_gitignore: false,
        ..Default::default()
    };
    let files: Vec<String> = scan(&opts)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    assert!(files.iter().any(|f| f.ends_with("a.html")));
    assert!(
        !files.iter().any(|f| f.contains("node_modules")),
        "node_modules leaked: {files:?}"
    );
}

#[test]
fn respects_gitignore() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    touch(root, ".gitignore", "secret.html\n");
    touch(root, "src/a.html", "");
    touch(root, "src/secret.html", "");

    // `.gitignore` only takes effect inside a real git repo. We emulate by
    // initializing a bare `.git` dir.
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

    let opts = WalkOptions {
        roots: vec![root.to_path_buf()],
        respect_gitignore: true,
        ..Default::default()
    };
    let files: Vec<String> = scan(&opts)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    assert!(files.iter().any(|f| f.ends_with("a.html")));
    assert!(!files.iter().any(|f| f.ends_with("secret.html")));
}

#[test]
fn additional_exclude_globs_apply() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    touch(root, "src/keep.tsx", "");
    touch(root, "src/skip.tsx", "");

    let opts = WalkOptions {
        roots: vec![root.to_path_buf()],
        exclude: vec!["**/skip.*".into()],
        ..Default::default()
    };
    let files: Vec<String> = scan(&opts)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    assert!(files.iter().any(|f| f.ends_with("keep.tsx")));
    assert!(!files.iter().any(|f| f.ends_with("skip.tsx")));
}

#[test]
fn empty_roots_yields_empty() {
    let opts = WalkOptions::default();
    assert!(scan(&opts).is_empty());
}
