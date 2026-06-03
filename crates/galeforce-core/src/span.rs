use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub file: Option<String>,
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub column: u32,
}

impl SourceSpan {
    pub fn new(start: usize, end: usize, line: u32, column: u32) -> Self {
        Self {
            file: None,
            start,
            end,
            line,
            column,
        }
    }
}
