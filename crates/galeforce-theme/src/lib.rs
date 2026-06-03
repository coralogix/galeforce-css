//! Theme resolution — dot-path lookups into the resolved Tailwind config value
//! produced by the Node config loader.

use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct Theme {
    root: Value,
}

impl Theme {
    pub fn from_value(root: Value) -> Self {
        Self { root }
    }

    /// Look up `colors.red.500` style dot-paths.
    pub fn lookup(&self, path: &str) -> Option<&Value> {
        let mut node = &self.root;
        for segment in path.split('.') {
            node = node.get(segment)?;
        }
        Some(node)
    }
}
