//! Rule ordering — 6-field sort key used by the compiler to emit rules in
//! Tailwind-compatible cascade order (simplified port; see CLAUDE.md carry-overs).

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SortKey {
    pub layer: u8,
    pub variant_order: u32,
    pub screen_order: u32,
    pub plugin_order: u32,
    pub property_order: u32,
    pub source_order: u32,
}
