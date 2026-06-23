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

#![allow(dead_code)]
//! Tailwind v3.4.19 `Offsets` system, ported to Rust.
//!
//! Variant bitmask: 256 bits stored as `[u128; 2]` in `[high, low]`
//! order. The derived `Ord` on the array lex-compares element 0 first
//! which lets the bitmask sort as a single 256-bit unsigned integer
//! with `high` as the MSB half. 256 bits covers ~200 named variants
//! plus dozens of arbitrary `[X]` ones — more than enough for the
//! corePlugins families (group/peer × pseudo, aria/data/supports ×
//! group/peer composites) plus user variants.
//!
//! This module mirrors `vendor/tailwindcss-v3/src/lib/offsets.js`
//! field-for-field and method-for-method. The upstream system uses
//! JavaScript bigints; we use `u128` for the variant bitmask, which
//! covers >99% of real projects (typical apps register ~75 named +
//! ~20 arbitrary variants per build). For projects that exceed 128
//! unique variants we emit a one-shot diagnostic and bits >127 get
//! dropped (graceful degrade — only affects emission order, never
//! correctness of declarations).
//!
//! Cross-references to upstream (line numbers from `offsets.js`):
//!
//!   - `RuleOffset` typedef: lines 20-30
//!   - constructor: lines 33-79
//!   - `create(layer)`: lines 85-97
//!   - `arbitraryProperty(name)`: lines 103-109
//!   - `forVariant(variant, index)`: lines 118-128
//!   - `applyVariantOffset(rule, variant, options)`: lines 136-151
//!   - `applyParallelOffset(offset, parallelIndex)`: lines 158-163
//!   - `recordVariant(variant, fnCount)`: lines 188-205
//!   - `recordVariants(variants, getLength)`: lines 175-179
//!   - `compare(a, b)`: lines 212-278 — THE sort cascade
//!   - `recalculateVariantOffsets()`: lines 287-305
//!   - `remapArbitraryVariantOffsets(list)`: lines 312-332
//!   - `sortArbitraryProperties(list)`: lines 339-376
//!   - `sort(list)`: lines 383-391
//!   - `remap-bitfield.js`: `remapBitfield(num, mapping)`

use std::cmp::Ordering;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

/// 256-bit variant bitmask. `[0]` = high 128 bits, `[1]` = low 128
/// bits — chosen so the derived `Ord` on `[u128; 2]` lex-compares
/// equivalently to a single unsigned 256-bit int.
pub type VarBits = [u128; 2];

/// All-zero bitmask.
pub const VAR_BITS_ZERO: VarBits = [0u128, 0u128];

/// `1 << position` as a 256-bit bitmask. `position >= 256` saturates
/// at the top bit (deterministic collision for the rare overflow case).
#[inline]
pub fn var_bit(position: u32) -> VarBits {
    if position >= 256 {
        return [1u128 << 127, 0u128];
    }
    if position < 128 {
        [0u128, 1u128 << position]
    } else {
        [1u128 << (position - 128), 0u128]
    }
}

#[inline]
pub fn var_or(a: VarBits, b: VarBits) -> VarBits {
    [a[0] | b[0], a[1] | b[1]]
}

#[inline]
pub fn var_and(a: VarBits, b: VarBits) -> VarBits {
    [a[0] & b[0], a[1] & b[1]]
}

#[inline]
pub fn var_andnot(a: VarBits, b: VarBits) -> VarBits {
    [a[0] & !b[0], a[1] & !b[1]]
}

#[inline]
pub fn var_is_zero(a: VarBits) -> bool {
    a[0] == 0 && a[1] == 0
}

#[inline]
pub fn var_eq(a: VarBits, b: VarBits) -> bool {
    a[0] == b[0] && a[1] == b[1]
}

#[inline]
pub fn var_cmp(a: VarBits, b: VarBits) -> Ordering {
    match a[0].cmp(&b[0]) {
        Ordering::Equal => a[1].cmp(&b[1]),
        ord => ord,
    }
}

/// Tailwind v3's six logical layers. Sort priority follows the
/// `layerPositions` map in `offsets.js:53-64`:
/// `defaults < base < components < utilities < user < variants`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layer {
    Defaults = 0,
    Base = 1,
    Components = 2,
    Utilities = 3,
    /// User CSS (e.g., rules defined alongside `@apply`). Sorts AFTER
    /// utilities so `.foo { @apply font-bold }` in user CSS picks up
    /// the deprecated-utility cascade order correctly.
    User = 4,
    Variants = 5,
}

impl Layer {
    /// Sort key — see `offsets.js:53-64 layerPositions`.
    #[inline]
    pub fn position(self) -> u8 {
        self as u8
    }
}

/// Variant sort-callback function pointer (mirrors `options.sort` in
/// `offsets.js:212-278 compare()` step 3). Each `matchVariant` plugin
/// may pass a `sort:` option that runs when two rules tied on every
/// earlier sort axis share the same matchVariant `id`.
///
/// The only family with a sort callback in vanilla v3.4.19 is
/// screens (`compareScreens('min', a.value, z.value)` / `'max'`).
pub type VariantSortFn = fn(a: &VariantOption, b: &VariantOption) -> Ordering;

/// Per-variant metadata carried on the rule's `options` array.
/// Mirrors the `VariantOption` typedef in `offsets.js:11-17` plus
/// the `variant` field set by `applyVariantOffset`.
#[derive(Clone, Debug, Default)]
pub struct VariantOption {
    /// Unique id per `matchVariant` group. Two options collide for
    /// sort-callback comparison only when their ids match (see
    /// `compare()` step 3).
    pub id: u32,
    /// Optional sort callback (today only screen variants).
    pub sort: Option<VariantSortFn>,
    /// Variant value (`'768px'` for `sm`, the raw arg for `min-[600px]`).
    pub value: Option<String>,
    /// Modifier — the part after a `/` in `group-hover/sidebar`.
    pub modifier: Option<String>,
    /// The variant's bitmask, captured at `applyVariantOffset` time
    /// so the compare-callback's masking step has direct access.
    pub variant: VarBits,
}

/// Per-rule sort key. Mirrors `RuleOffset` typedef in
/// `offsets.js:20-30`.
#[derive(Clone, Debug)]
pub struct RuleOffset {
    pub layer: Layer,
    /// The rule's ORIGINAL layer before any variants pushed it into
    /// `Layer::Variants`. Only differs from `layer` when the rule
    /// has at least one variant applied. Used as a secondary sort
    /// axis so variant-wrapped utilities sort alongside their non-
    /// variant siblings.
    pub parent_layer: Layer,
    /// 0 or 1 (matches upstream `0n` / `1n`). Set to 1 when the
    /// rule is an arbitrary property (`[grid-template-columns:...]`).
    pub arbitrary: u8,
    /// Variant bitmask. Each registered variant gets one bit (or N
    /// bits for parallel matchVariants). For two rules:
    /// `a.variants - b.variants` lex-compares as bigints. Lower
    /// bits = earlier emission.
    pub variants: VarBits,
    /// Parallel-rule index. Used when a single matchUtility yields
    /// multiple rules (e.g., `animate-spin` yields `@keyframes spin`
    /// AND `.animate-spin {...}`). Keyframes get index 0 so they
    /// emit BEFORE the consuming utility.
    pub parallel_index: u32,
    /// Per-layer monotonically increasing counter. Final tiebreaker
    /// once every other axis ties. We pack our existing
    /// (plugin_order, within_plugin_order, alphabetic) ordering
    /// into this field.
    pub index: u64,
    /// Assigned by `sortArbitraryProperties` — alphabetic rank of
    /// the arbitrary property name across the whole stylesheet.
    /// `0` means not an arbitrary property.
    pub property_offset: u32,
    /// Arbitrary property name (empty when `arbitrary == 0`). Stored
    /// owned because the property string isn't `&'static`.
    pub property: String,
    /// Variant-option stack — one entry per variant that contributes
    /// a `sort:` callback. Pushed at the front (mirrors
    /// `[].concat(options, rule.options)` in `applyVariantOffset`)
    /// so the most-recently-applied variant's sort runs first.
    pub options: SmallVec<[VariantOption; 4]>,
}

impl RuleOffset {
    /// Construct a default RuleOffset for the given layer.
    /// Initializes `parent_layer == layer`, all numerics to 0,
    /// `index` set by the caller (`Offsets::create` bumps it).
    fn default_for(layer: Layer) -> Self {
        Self {
            layer,
            parent_layer: layer,
            arbitrary: 0,
            variants: VAR_BITS_ZERO,
            parallel_index: 0,
            index: 0,
            property_offset: 0,
            property: String::new(),
            options: SmallVec::new(),
        }
    }
}

impl PartialEq for RuleOffset {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for RuleOffset {}

impl PartialOrd for RuleOffset {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RuleOffset {
    /// Mirrors `Offsets.compare()` in `offsets.js:212-278`.
    ///
    /// 1. layer
    /// 2. parent_layer (only when both rules ended up in variants)
    /// 3. for each shared option.id with a sort callback, run it
    ///    under the bitmask `~(maxFnVariant | (maxFnVariant - 1))`
    /// 4. variants bitmask (numeric)
    /// 5. parallel_index
    /// 6. arbitrary
    /// 7. property_offset
    /// 8. index
    fn cmp(&self, other: &Self) -> Ordering {
        // 1. layer
        match self.layer.position().cmp(&other.layer.position()) {
            Ordering::Equal => {}
            ord => return ord,
        }
        // 2. parent_layer (only meaningful when both rules are in
        //    Layer::Variants; for non-variant rules parent_layer ==
        //    layer so this is a no-op).
        match self
            .parent_layer
            .position()
            .cmp(&other.parent_layer.position())
        {
            Ordering::Equal => {}
            ord => return ord,
        }
        // 3. matchVariant sort callbacks. Walk a.options × b.options
        //    looking for matching ids with sort fns. Upstream's mask
        //    skips the comparison if higher-bit variants differ
        //    between a and b.
        for a_opt in &self.options {
            for b_opt in &other.options {
                if a_opt.id != b_opt.id {
                    continue;
                }
                let Some(sort) = a_opt.sort else {
                    continue;
                };
                if b_opt.sort.is_none() {
                    continue;
                }
                // Pick the numerically larger of the two variant
                // bitmasks (lex on `[high, low]` matches single-int
                // ordering). The mask zeros out everything from that
                // bit downward so we compare only HIGHER variants.
                let max_fn_variant = if var_cmp(a_opt.variant, b_opt.variant) == Ordering::Greater {
                    a_opt.variant
                } else {
                    b_opt.variant
                };
                // mask = ~(maxFnVariant | (maxFnVariant - 1))
                // Computed on the 256-bit form: treat the two-word
                // value as a single integer, subtract 1 (with borrow),
                // OR with maxFnVariant, then bitwise NOT.
                let m = max_fn_variant;
                let (low_minus, borrow) = m[1].overflowing_sub(1);
                let high_minus = m[0].wrapping_sub(if borrow { 1 } else { 0 });
                let or_low = m[1] | low_minus;
                let or_high = m[0] | high_minus;
                let mask: VarBits = [!or_high, !or_low];
                let self_masked = var_and(self.variants, mask);
                let other_masked = var_and(other.variants, mask);
                if !var_eq(self_masked, other_masked) {
                    continue;
                }
                let result = sort(a_opt, b_opt);
                if result != Ordering::Equal {
                    return result;
                }
            }
        }
        // 4. variants bitmask
        match var_cmp(self.variants, other.variants) {
            Ordering::Equal => {}
            ord => return ord,
        }
        // 5. parallel_index
        match self.parallel_index.cmp(&other.parallel_index) {
            Ordering::Equal => {}
            ord => return ord,
        }
        // 6. arbitrary (non-arbitrary `0` sorts before `1`)
        match self.arbitrary.cmp(&other.arbitrary) {
            Ordering::Equal => {}
            ord => return ord,
        }
        // 7. property_offset (alphabetic via sortArbitraryProperties)
        match self.property_offset.cmp(&other.property_offset) {
            Ordering::Equal => {}
            ord => return ord,
        }
        // 8. index (registration order within layer)
        self.index.cmp(&other.index)
    }
}

/// Single-compile-lifetime registry mirroring upstream's `Offsets`
/// class. Each `compile()` call constructs one; arbitrary variants
/// register at first encounter and get alphabetically remapped at
/// final sort time.
#[derive(Debug, Default)]
pub struct Offsets {
    /// Per-layer monotonic counter — `offsets.js:39-46`.
    next_index: [u64; 6],
    /// Total variant bits allocated so far — `offsets.js:71`.
    reserved_variant_bits: u32,
    /// Variant-name → assigned bit. Insertion-ordered so we can
    /// preserve registration order even when iterating for the
    /// alphabetic remap.
    variant_offsets: FxHashMap<String, VarBits>,
    /// Whether we've spilled past the 256-bit limit. One-shot
    /// diagnostic flag — set true on the FIRST variant we couldn't
    /// allocate a bit for. Used by callers to surface a warning.
    pub overflow: bool,
}

impl Offsets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mirrors `Offsets.create(layer)` (`offsets.js:85-97`).
    /// Returns a fresh rule offset with the per-layer index bumped.
    pub fn create(&mut self, layer: Layer) -> RuleOffset {
        let mut offset = RuleOffset::default_for(layer);
        let slot = layer.position() as usize;
        offset.index = self.next_index[slot];
        self.next_index[slot] += 1;
        offset
    }

    /// Mirrors `Offsets.arbitraryProperty(name)` (`offsets.js:103-109`).
    pub fn arbitrary_property(&mut self, name: &str) -> RuleOffset {
        let mut offset = self.create(Layer::Utilities);
        offset.arbitrary = 1;
        offset.property = name.to_string();
        offset
    }

    /// Mirrors `Offsets.recordVariant(variant, fnCount)`
    /// (`offsets.js:188-205`). Assigns the next free bit and
    /// reserves `fn_count` bits for parallel-rule variants. Returns
    /// the variant's own RuleOffset (used by `forVariant`).
    ///
    /// `fn_count` defaults to 1 for `addVariant` and equals the
    /// number of functions for `matchVariant` plugins that return
    /// parallel rules (e.g., `marker` has 2 selectors so it
    /// reserves 2 bits in upstream's setup).
    pub fn record_variant(&mut self, name: &str, fn_count: u32) -> RuleOffset {
        let fn_count = fn_count.max(1);
        let bit = if self.reserved_variant_bits >= 256 {
            // Overflow — past our 256-bit budget. Surface once;
            // subsequent calls all share the top bit (worst-case
            // collision but deterministic).
            self.overflow = true;
            var_bit(255)
        } else {
            var_bit(self.reserved_variant_bits)
        };
        self.variant_offsets.insert(name.to_string(), bit);
        self.reserved_variant_bits = self.reserved_variant_bits.saturating_add(fn_count);
        let mut offset = self.create(Layer::Variants);
        offset.variants = bit;
        offset
    }

    /// Convenience for the common case where the variant doesn't
    /// need its own RuleOffset returned (just the bit).
    pub fn record_variant_bit(&mut self, name: &str, fn_count: u32) -> VarBits {
        if let Some(&existing) = self.variant_offsets.get(name) {
            return existing;
        }
        let offset = self.record_variant(name, fn_count);
        offset.variants
    }

    /// Mirrors `Offsets.recordVariants(variants, getLength)`
    /// (`offsets.js:175-179`). Iterates and records each.
    pub fn record_variants<'a, I: IntoIterator<Item = (&'a str, u32)>>(&mut self, vs: I) {
        for (name, fn_count) in vs {
            // Don't re-register variants — first record wins.
            if !self.variant_offsets.contains_key(name) {
                self.record_variant(name, fn_count);
            }
        }
    }

    /// True iff `name` has already been registered.
    pub fn has_variant(&self, name: &str) -> bool {
        self.variant_offsets.contains_key(name)
    }

    /// Look up a registered variant's bit. `None` for unknown
    /// variants — callers should treat them as having no bit
    /// (variants = 0).
    pub fn variant_bit(&self, name: &str) -> Option<VarBits> {
        self.variant_offsets.get(name).copied()
    }

    /// Mirrors `Offsets.forVariant(variant, index)` (`offsets.js:118-128`).
    /// Returns a fresh `Layer::Variants` offset with the variant's
    /// bit set at position `bit << index` (the `index` shift
    /// supports `applyParallelOffset` for parallel-rule variants).
    ///
    /// Returns `None` for unknown variants — upstream throws; we
    /// degrade so unsupported variants don't panic compile.
    pub fn for_variant(&mut self, name: &str, parallel_index: u32) -> Option<RuleOffset> {
        let bit = *self.variant_offsets.get(name)?;
        let mut offset = self.create(Layer::Variants);
        // Shift the bit-pattern left by `parallel_index` (across the
        // 256-bit composed integer). Implemented as a two-word shift.
        offset.variants = shift_left(bit, parallel_index);
        Some(offset)
    }

    /// Mirrors `Offsets.applyVariantOffset(rule, variant, options)`
    /// (`offsets.js:136-151`). Combines the variant's bitmask into
    /// the rule's, preserves the rule's original layer in
    /// `parent_layer`, and prepends `option` to the rule's options
    /// stack (so the most-recently-applied variant's sort fn runs
    /// first in `compare()` step 3).
    pub fn apply_variant_offset(
        &self,
        mut rule: RuleOffset,
        variant: &RuleOffset,
        mut option: VariantOption,
    ) -> RuleOffset {
        option.variant = variant.variants;
        // parent_layer = rule.layer (unless rule already in
        // Variants, then preserve its parent_layer).
        let new_parent = if matches!(rule.layer, Layer::Variants) {
            rule.parent_layer
        } else {
            rule.layer
        };
        rule.parent_layer = new_parent;
        rule.layer = Layer::Variants;
        rule.variants = var_or(rule.variants, variant.variants);
        // Mirror `[].concat(options, rule.options)` if the option
        // carries a sort callback; otherwise leave options alone.
        if option.sort.is_some() {
            rule.options.insert(0, option);
        }
        // parallel_index = max(rule.parallel_index, variant.parallel_index)
        rule.parallel_index = rule.parallel_index.max(variant.parallel_index);
        rule
    }

    /// Mirrors `Offsets.applyParallelOffset(offset, parallelIndex)`
    /// (`offsets.js:158-163`). Used for parallel rules from a
    /// single matchUtility call (e.g., animation emits
    /// `@keyframes` + `.animate-X`).
    pub fn apply_parallel_offset(&self, mut offset: RuleOffset, parallel_index: u32) -> RuleOffset {
        offset.parallel_index = parallel_index;
        offset
    }

    /// Mirrors `Offsets.recalculateVariantOffsets()`
    /// (`offsets.js:287-305`). Returns the remap mapping
    /// `[(old_bit, new_bit), ...]` for arbitrary variants only.
    /// Variants get alphabetic bit positions starting from the
    /// smallest existing arbitrary variant's bit.
    fn recalculate_variant_offsets(&self) -> Vec<(VarBits, VarBits)> {
        // Collect arbitrary variants (names starting with `[`).
        let mut variants: Vec<(&str, VarBits)> = self
            .variant_offsets
            .iter()
            .filter(|(name, _)| name.starts_with('['))
            .map(|(n, &b)| (n.as_str(), b))
            .collect();
        // Sort by name alphabetically — mirrors upstream's
        // `fastCompare`.
        variants.sort_by(|a, b| a.0.cmp(b.0));
        // Sort the offsets numerically. Together with the variant
        // sort above this gives us the (old_bit, new_bit) pairs.
        let mut new_offsets: Vec<VarBits> = variants.iter().map(|(_, b)| *b).collect();
        new_offsets.sort_by(|a, b| var_cmp(*a, *b));
        let mut mapping: Vec<(VarBits, VarBits)> = variants
            .iter()
            .zip(new_offsets.iter())
            .map(|((_, old), &new)| (*old, new))
            .collect();
        mapping.retain(|(a, z)| !var_eq(*a, *z));
        mapping
    }

    /// Mirrors `Offsets.sort(list)` (`offsets.js:383-391`). The
    /// full pipeline: remap arbitrary variant bits alphabetically,
    /// assign arbitrary-property offsets alphabetically, then sort
    /// by `compare`.
    ///
    /// Returns the (RuleOffset, T) list sorted in place.
    pub fn sort<T>(&mut self, mut list: Vec<(RuleOffset, T)>) -> Vec<(RuleOffset, T)> {
        // 1. Remap arbitrary variant bitmasks alphabetically.
        let mapping = self.recalculate_variant_offsets();
        if !mapping.is_empty() {
            for (offset, _) in list.iter_mut() {
                offset.variants = remap_bitfield(offset.variants, &mapping);
            }
        }
        // 2. Assign property_offset alphabetically across arbitrary
        //    properties seen in this list.
        let mut props: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (offset, _) in &list {
            if offset.arbitrary == 1 {
                props.insert(offset.property.clone());
            }
        }
        if !props.is_empty() {
            let mut prop_to_offset: FxHashMap<String, u32> = FxHashMap::default();
            let mut next: u32 = 1;
            for prop in &props {
                prop_to_offset.insert(prop.clone(), next);
                next += 1;
            }
            for (offset, _) in list.iter_mut() {
                if offset.arbitrary == 1 {
                    if let Some(&po) = prop_to_offset.get(&offset.property) {
                        offset.property_offset = po;
                    }
                }
            }
        }
        // 3. Sort by the offset compare cascade.
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }
}

/// Mirrors `remap-bitfield.js:68-82 remapBitfield(num, mapping)`.
/// Builds an `oldMask` of bits set in `num` that have entries in
/// `mapping`, and a `newMask` of their replacements. Returns
/// `(num & !oldMask) | newMask`.
fn remap_bitfield(num: VarBits, mapping: &[(VarBits, VarBits)]) -> VarBits {
    let mut old_mask: VarBits = VAR_BITS_ZERO;
    let mut new_mask: VarBits = VAR_BITS_ZERO;
    for (old, new) in mapping {
        if !var_is_zero(var_and(num, *old)) {
            old_mask = var_or(old_mask, *old);
            new_mask = var_or(new_mask, *new);
        }
    }
    var_or(var_andnot(num, old_mask), new_mask)
}

/// 256-bit left-shift used by `for_variant` to apply a parallel-rule
/// offset. `n` clamps at 256 (degrades to zero — matches upstream's
/// bigint semantics when bits roll off the top).
fn shift_left(num: VarBits, n: u32) -> VarBits {
    if n >= 256 {
        return VAR_BITS_ZERO;
    }
    if n == 0 {
        return num;
    }
    if n < 128 {
        let low = num[1] << n;
        let high = (num[0] << n) | (num[1] >> (128 - n));
        [high, low]
    } else {
        // Shift by [128, 256): low becomes 0, high gets old low <<
        // (n - 128).
        let high = num[1] << (n - 128);
        [high, 0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_bumps_per_layer_index() {
        let mut o = Offsets::new();
        let a = o.create(Layer::Utilities);
        let b = o.create(Layer::Utilities);
        let c = o.create(Layer::User);
        assert_eq!(a.index, 0);
        assert_eq!(b.index, 1);
        assert_eq!(c.index, 0);
        assert_eq!(a.layer, Layer::Utilities);
        assert_eq!(c.layer, Layer::User);
    }

    #[test]
    fn record_variant_allocates_sequential_bits() {
        let mut o = Offsets::new();
        let a = o.record_variant("hover", 1);
        let b = o.record_variant("focus", 1);
        let c = o.record_variant("md", 1);
        assert_eq!(a.variants, var_bit(0));
        assert_eq!(b.variants, var_bit(1));
        assert_eq!(c.variants, var_bit(2));
        assert_eq!(o.reserved_variant_bits, 3);
    }

    #[test]
    fn record_variant_reserves_fn_count_bits() {
        let mut o = Offsets::new();
        // `marker` matches upstream with 2 parallel selectors.
        o.record_variant("hover", 1);
        let marker = o.record_variant("marker", 2);
        let next = o.record_variant("focus", 1);
        assert_eq!(marker.variants, var_bit(1));
        // `focus` gets bit 3 because marker reserved bits 1 and 2.
        assert_eq!(next.variants, var_bit(3));
    }

    #[test]
    fn apply_variant_offset_or_s_bits_and_sets_parent_layer() {
        let mut o = Offsets::new();
        o.record_variants(vec![("hover", 1), ("md", 1)]);
        let rule = o.create(Layer::Utilities);
        let hover = o.for_variant("hover", 0).unwrap();
        let md = o.for_variant("md", 0).unwrap();
        let r1 = o.apply_variant_offset(rule, &hover, VariantOption::default());
        let r2 = o.apply_variant_offset(r1.clone(), &md, VariantOption::default());
        assert_eq!(r2.layer, Layer::Variants);
        // parent_layer captures the utility origin once and stays.
        assert_eq!(r2.parent_layer, Layer::Utilities);
        // Both hover (bit 0) and md (bit 1) bits set.
        assert_eq!(r2.variants, [0u128, 0b11u128]);
    }

    #[test]
    fn compare_layer_first() {
        let mut o = Offsets::new();
        let util = o.create(Layer::Utilities);
        let user = o.create(Layer::User);
        assert!(util < user);
        let comp = o.create(Layer::Components);
        assert!(comp < util);
    }

    #[test]
    fn compare_variants_bitmask_as_number() {
        let mut o = Offsets::new();
        o.record_variants(vec![("hover", 1), ("md", 1), ("dark", 1)]);
        let mut mk = |name: &str| -> RuleOffset {
            let mut rule = o.create(Layer::Utilities);
            // Skip applying via for_variant since that bumps index.
            // Inline the bit manipulation to keep index ties.
            let bit = o.variant_offsets[name];
            rule.layer = Layer::Variants;
            rule.parent_layer = Layer::Utilities;
            rule.variants = bit;
            rule
        };
        // hover = bit 0, md = bit 1, dark = bit 2. Sort order in
        // variants axis ascending: hover < md < dark.
        let h = mk("hover");
        let m = mk("md");
        let d = mk("dark");
        assert!(h < m);
        assert!(m < d);
    }

    #[test]
    fn compare_arbitrary_after_non_arbitrary() {
        let mut o = Offsets::new();
        let plain = o.create(Layer::Utilities);
        let arb = o.arbitrary_property("grid-template-columns");
        assert!(plain < arb);
        assert_eq!(plain.arbitrary, 0);
        assert_eq!(arb.arbitrary, 1);
    }

    #[test]
    fn compare_parallel_index_breaks_ties() {
        let mut o = Offsets::new();
        let utility = o.create(Layer::Utilities);
        let keyframes = o.apply_parallel_offset(utility.clone(), 0);
        let animation = o.apply_parallel_offset(utility, 1);
        assert!(keyframes < animation);
    }

    #[test]
    fn sort_remaps_arbitrary_variants_alphabetically() {
        // Register arbitrary variants in NON-alphabetic order.
        let mut o = Offsets::new();
        let v_z = o.record_variant("[z_first]", 1); // bit 0
        let v_a = o.record_variant("[a_second]", 1); // bit 1
                                                     // Create two rules — one with z, one with a.
        let rule_z = {
            let mut r = o.create(Layer::Utilities);
            r.layer = Layer::Variants;
            r.parent_layer = Layer::Utilities;
            r.variants = v_z.variants;
            r
        };
        let rule_a = {
            let mut r = o.create(Layer::Utilities);
            r.layer = Layer::Variants;
            r.parent_layer = Layer::Utilities;
            r.variants = v_a.variants;
            r
        };
        // Pre-remap: z (bit 0) < a (bit 1). Without remapping z
        // would sort first.
        let list = vec![(rule_a.clone(), "a"), (rule_z.clone(), "z")];
        let sorted = o.sort(list);
        // After alphabetic remap, [a_second] should hold the lower
        // bit and emit first.
        let names: Vec<&str> = sorted.iter().map(|(_, t)| *t).collect();
        assert_eq!(names, vec!["a", "z"]);
    }

    #[test]
    fn sort_assigns_property_offset_alphabetically() {
        let mut o = Offsets::new();
        let later = o.arbitrary_property("z-index");
        let earlier = o.arbitrary_property("a-prop");
        let list = vec![(later, "z"), (earlier, "a")];
        let sorted = o.sort(list);
        let names: Vec<&str> = sorted.iter().map(|(_, t)| *t).collect();
        assert_eq!(names, vec!["a", "z"]);
    }

    #[test]
    fn sort_callback_runs_when_options_match() {
        fn compare_min_width(a: &VariantOption, b: &VariantOption) -> Ordering {
            // Parse min-width pixel value from `value` strings like
            // "640px" / "768px" and compare numerically.
            let pa: u32 = a
                .value
                .as_deref()
                .and_then(|s| s.trim_end_matches("px").parse().ok())
                .unwrap_or(0);
            let pb: u32 = b
                .value
                .as_deref()
                .and_then(|s| s.trim_end_matches("px").parse().ok())
                .unwrap_or(0);
            pa.cmp(&pb)
        }

        let mut o = Offsets::new();
        // Register sm and md in REVERSE bit order (md gets bit 0,
        // sm gets bit 1) — without the sort callback md would emit
        // before sm purely by bitmask.
        o.record_variant("md", 1);
        o.record_variant("sm", 1);

        let mut mk = |name: &str, px: &str| -> RuleOffset {
            let rule = o.create(Layer::Utilities);
            let variant = RuleOffset {
                layer: Layer::Variants,
                parent_layer: Layer::Variants,
                arbitrary: 0,
                variants: o.variant_offsets[name],
                parallel_index: 0,
                index: 0,
                property_offset: 0,
                property: String::new(),
                options: SmallVec::new(),
            };
            let option = VariantOption {
                id: 42, // shared id — both screens are same matchVariant group
                sort: Some(compare_min_width),
                value: Some(px.to_string()),
                modifier: None,
                variant: VAR_BITS_ZERO,
            };
            o.apply_variant_offset(rule, &variant, option)
        };

        let sm = mk("sm", "640px");
        let md = mk("md", "768px");
        assert!(
            sm < md,
            "sm (640px) must sort before md (768px) via the sort callback"
        );
    }
}
