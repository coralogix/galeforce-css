//! File discovery for the content scanner.
//!
//! Built on the `ignore` crate so we get `.gitignore` semantics for free,
//! plus the standard set of project-level excludes Tailwind users expect
//! (node_modules, target, dist, …).

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

/// File-extension allowlist used by Tailwind v3 by default.
///
/// We deliberately re-derive this here rather than reading
/// `tailwindcss.config.js` `content`. The Node config loader is responsible
/// for supplying user-configured globs; this list is the fallback used when
/// the user hasn't pinned any.
pub const DEFAULT_EXTENSIONS: &[&str] = &[
    "html",
    "htm",
    "vue",
    "svelte",
    "astro",
    "mdx",
    "md",
    "js",
    "mjs",
    "cjs",
    "jsx",
    "ts",
    "tsx",
    "php",
    "blade.php",
    "twig",
    "hbs",
    "ejs",
    "rb",
    "erb",
    "haml",
];

/// Directories that should never be scanned, even if a user glob would
/// otherwise match. These dwarf any plausible content dir and Tailwind
/// itself excludes them by default.
const DEFAULT_PRUNE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    "out",
    "coverage",
    ".vite",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "target",
];

#[derive(Clone, Debug)]
pub struct WalkOptions {
    /// Roots to recurse from. Each is walked independently.
    pub roots: Vec<PathBuf>,
    /// Additional include globs. Empty means "every supported extension under
    /// the roots".
    pub include: Vec<String>,
    /// Additional exclude globs (applied after .gitignore + DEFAULT_PRUNE_DIRS).
    pub exclude: Vec<String>,
    /// Whether to follow symlinks. Default false.
    pub follow_links: bool,
    /// Whether to consult `.gitignore`. Default true.
    pub respect_gitignore: bool,
    /// File extensions to consider. Defaults to `DEFAULT_EXTENSIONS`.
    pub extensions: Option<Vec<String>>,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            include: Vec::new(),
            exclude: Vec::new(),
            follow_links: false,
            respect_gitignore: true,
            extensions: None,
        }
    }
}

/// Walk every root and return the absolute paths of files that should be
/// tokenized. Output order is determined by the `ignore` crate (parallel walk
/// would change order; we use the sequential walker for determinism).
pub fn discover_files(opts: &WalkOptions) -> Result<Vec<PathBuf>, String> {
    if opts.roots.is_empty() {
        return Ok(Vec::new());
    }

    let include_set = build_glob_set(&opts.include).map_err(|e| format!("include glob: {e}"))?;
    let exclude_set = build_glob_set(&opts.exclude).map_err(|e| format!("exclude glob: {e}"))?;
    let extensions: Vec<String> = opts.extensions.clone().unwrap_or_else(|| {
        DEFAULT_EXTENSIONS
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    });

    let mut paths = Vec::new();

    for root in &opts.roots {
        // Fast path: the caller already resolved this entry to a
        // concrete file (common when the Vite plugin pre-expands
        // content globs JS-side via tinyglobby). Skip the WalkBuilder
        // setup entirely — building 16k WalkBuilders for 16k files is
        // ~2s of pure overhead with no actual walking to do.
        if root.is_file() {
            let matches_include = extension_matches(root, &extensions)
                || (!include_set.is_empty() && include_set.is_match(root));
            let matches_exclude = !exclude_set.is_empty() && exclude_set.is_match(root);
            if matches_include && !matches_exclude {
                paths.push(root.clone());
            }
            continue;
        }

        let mut builder = WalkBuilder::new(root);
        builder
            .follow_links(opts.follow_links)
            .git_ignore(opts.respect_gitignore)
            .git_global(opts.respect_gitignore)
            .git_exclude(opts.respect_gitignore)
            .hidden(false)
            .parents(opts.respect_gitignore);

        // DEFAULT_PRUNE_DIRS: skip directories at any depth.
        let mut overrides = ignore::overrides::OverrideBuilder::new(root);
        for prune in DEFAULT_PRUNE_DIRS {
            overrides
                .add(&format!("!**/{prune}/**"))
                .map_err(|e| format!("prune {prune}: {e}"))?;
        }
        builder.overrides(overrides.build().map_err(|e| format!("overrides: {e}"))?);

        for entry in builder.build() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }

            if !extension_matches(path, &extensions)
                && (include_set.is_empty() || !include_set.is_match(path))
            {
                continue;
            }

            if !exclude_set.is_empty() && exclude_set.is_match(path) {
                continue;
            }

            paths.push(path.to_path_buf());
        }
    }

    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn build_glob_set(patterns: &[String]) -> Result<GlobSet, globset::Error> {
    if patterns.is_empty() {
        return GlobSetBuilder::new().build();
    }
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(Glob::new(p)?);
    }
    b.build()
}

fn extension_matches(path: &Path, extensions: &[String]) -> bool {
    // We compare against full suffix (e.g. `.blade.php`) and against the simple
    // extension. Tailwind's default content list includes `.blade.php` which
    // `Path::extension` would only resolve to `.php`.
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    for ext in extensions {
        if name.len() > ext.len() + 1
            && name.ends_with(ext.as_str())
            && name.as_bytes()[name.len() - ext.len() - 1] == b'.'
        {
            return true;
        }
    }
    false
}
