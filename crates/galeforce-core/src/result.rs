use serde::{Deserialize, Serialize};

use crate::Diagnostic;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CssOutput {
    pub css: String,
    /// Optional source map JSON string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileResult {
    pub output: CssOutput,
    pub diagnostics: Vec<Diagnostic>,
    /// Number of candidates that produced rules.
    pub candidate_count: usize,
    /// Number of generated rules (post-sort).
    pub rule_count: usize,
}
