//! Shared types for GaleforceCSS.
//!
//! This crate is consumed by every other Galeforce crate. It deliberately has no
//! heavy dependencies and exposes only data: errors, diagnostics, source
//! spans, and the public compile request/response shapes.

mod diagnostic;
mod error;
mod options;
mod result;
mod span;

pub use diagnostic::{Diagnostic, Severity};
pub use error::GaleforceError;
pub use options::{
    BuildMode, CompatFlags, CompileOptions, DarkMode, FeatureFlags, ImportantMode, Layer,
    TailwindVersion,
};
pub use result::{CompileResult, CssOutput};
pub use span::SourceSpan;
