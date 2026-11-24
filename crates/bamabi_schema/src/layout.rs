//! Target-aware `#[repr(C)]` layout arithmetic, padding detection, and `_pad*` synthesis.

use core::fmt;

/// Error returned when `#[repr(C)]` layout computation overflows or encounters invalid alignment.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// Size or offset calculation overflowed `usize`.
    Overflow,
    /// Alignment was zero or not a power of two.
    InvalidAlignment,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => f.write_str("repr(C) layout arithmetic overflow"),
            Self::InvalidAlignment => f.write_str("repr(C) field alignment must be a power of two"),
        }
    }
}

impl core::error::Error for LayoutError {}

/// Rounds `offset` up to the next multiple of `align` (`align <= 1` returns `Some(offset)`),
/// returning `None` on `usize` overflow or non-power-of-two `align > 1`.
#[inline]
#[must_use]
pub const fn align_up(offset: usize, align: usize) -> Option<usize> {
    if align <= 1 {
        return Some(offset);
    }
    if !align.is_power_of_two() {
        return None;
    }
    let Some(bumped) = offset.checked_add(align - 1) else {
        return None;
    };
    Some(bumped & !(align - 1))
}

/// Computes `(pad_before, aligned_offset, next_cursor)` for placing a field of size `field_size`
/// and alignment `field_align` at `cursor`, returning `None` on overflow or invalid alignment.
#[inline]
#[must_use]
pub const fn compute_field_placement(
    cursor: usize,
    field_size: usize,
    field_align: usize,
) -> Option<(usize, usize, usize)> {
    let eff_align = if field_align == 0 { 1 } else { field_align };
    let Some(aligned_off) = align_up(cursor, eff_align) else {
        return None;
    };
    let pad_before = aligned_off - cursor;
    let Some(next_cursor) = aligned_off.checked_add(field_size) else {
        return None;
    };
    Some((pad_before, aligned_off, next_cursor))
}

/// Computes `#[repr(C)]` `(total_size, max_align, has_implicit_padding)` from `(size, align)` pairs
/// without heap allocation.
#[must_use]
pub const fn compute_struct_size_and_padding(
    fields: &[(usize, usize)],
) -> Option<(usize, usize, bool)> {
    let mut cursor = 0usize;
    let mut max_align = 1usize;
    let mut has_padding = false;
    let mut i = 0usize;
    while i < fields.len() {
        let (size, raw_align) = fields[i];
        let align = if raw_align == 0 { 1 } else { raw_align };
        if !align.is_power_of_two() {
            return None;
        }
        if align > max_align {
            max_align = align;
        }
        let Some((pad_before, _, next_cursor)) = compute_field_placement(cursor, size, align)
        else {
            return None;
        };
        if pad_before > 0 {
            has_padding = true;
        }
        cursor = next_cursor;
        i += 1;
    }
    let Some(total_size) = align_up(cursor, max_align) else {
        return None;
    };
    if total_size > cursor {
        has_padding = true;
    }
    Some((total_size, max_align, has_padding))
}

#[cfg(feature = "alloc")]
mod alloc_solver {
    extern crate alloc;

    use super::{LayoutError, align_up, compute_field_placement};
    use crate::hash::{
        SchemaFieldInput, compute_component_uuid, compute_schema_fingerprint,
        format_uuid_hyphenated,
    };
    use crate::wire_types::BamFieldKindId;
    use alloc::format;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    /// Raw input field passed into [`solve_repr_c_layout`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RawFieldInput {
        /// Original field name.
        pub name: String,
        /// Canonical field type tag.
        pub type_tag: String,
        /// Canonical `BamFieldKindId`.
        pub kind: BamFieldKindId,
        /// Field byte size.
        pub size: usize,
        /// Field byte alignment.
        pub align: usize,
        /// Whether this field satisfies `AbiPod`.
        pub is_abi_pod: bool,
        /// Whether this field satisfies `RawInsertable`.
        pub is_raw_insertable: bool,
    }

    /// Single solved `#[repr(C)]` field (either user field or synthesized zero-padding `_pad*` array).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SolvedFieldLayout {
        /// Field identifier (or `<pad_prefix><index>` when `is_padding == true`).
        pub name: String,
        /// Canonical field type tag (`"[u8;N]"` when `is_padding == true`).
        pub type_tag: String,
        /// Canonical `BamFieldKindId` (`BamFieldKindId::NESTED` when `is_padding == true`).
        pub kind: BamFieldKindId,
        /// Byte offset inside the enclosing `#[repr(C)]` struct.
        pub offset: usize,
        /// Byte size.
        pub size: usize,
        /// Byte alignment.
        pub align: usize,
        /// `true` iff this field was synthesized as explicit zero padding.
        pub is_padding: bool,
    }

    /// Fully solved `#[repr(C)]` struct layout with synthesized padding, UUID, and `SCHEMA_FINGERPRINT`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SolvedStructLayout {
        /// Canonical namespaced component/struct name (`"<namespace>::<Name>"`).
        pub canonical_name: String,
        /// Ordered fields including synthesized explicit padding fields.
        pub fields: Vec<SolvedFieldLayout>,
        /// Total solved byte size.
        pub size: usize,
        /// Total solved byte alignment.
        pub align: usize,
        /// `true` iff all fields are `AbiPod` (padding bytes are made explicit).
        pub is_abi_pod: bool,
        /// `true` iff all fields are `RawInsertable`.
        pub is_raw_insertable: bool,
        /// Total number of synthesized padding bytes.
        pub explicit_pad_bytes: usize,
        /// Deterministic 128-bit SipHash-2-4-128 component UUID bytes.
        pub uuid_bytes: [u8; 16],
        /// Canonical hyphenated UUID string (`"8-4-4-4-12"`).
        pub uuid_hex: String,
        /// Deterministic 64-bit FNV-1a `SCHEMA_FINGERPRINT`.
        pub schema_fingerprint: u64,
    }

    /// Solves a `#[repr(C)]` struct layout, inserting explicit zero-padding `[u8; N]` fields
    /// named `<pad_prefix><index>` wherever alignment gaps or trailing tail padding occur.
    ///
    /// # Errors
    /// Returns [`LayoutError::InvalidAlignment`] if any field alignment is not a power of two, or
    /// [`LayoutError::Overflow`] if cumulative offsets overflow `usize`.
    pub fn solve_repr_c_layout(
        canonical_name: &str,
        raw_fields: &[RawFieldInput],
        component_flags: u32,
        pad_prefix: &str,
    ) -> Result<SolvedStructLayout, LayoutError> {
        let mut solved_fields = Vec::with_capacity(raw_fields.len());
        let mut cursor = 0usize;
        let mut max_align = 1usize;
        let mut pad_idx = 0usize;
        let mut explicit_pad_bytes = 0usize;
        let mut is_abi_pod = true;
        let mut is_raw_insertable = true;

        for f in raw_fields {
            let eff_align = if f.align == 0 { 1 } else { f.align };
            if !eff_align.is_power_of_two() {
                return Err(LayoutError::InvalidAlignment);
            }
            is_abi_pod &= f.is_abi_pod;
            is_raw_insertable &= f.is_raw_insertable;
            max_align = max_align.max(eff_align);

            let (pad_len, aligned_off, next_cursor) =
                compute_field_placement(cursor, f.size, eff_align).ok_or(LayoutError::Overflow)?;

            if pad_len > 0 {
                explicit_pad_bytes = explicit_pad_bytes
                    .checked_add(pad_len)
                    .ok_or(LayoutError::Overflow)?;
                solved_fields.push(SolvedFieldLayout {
                    name: format!("{pad_prefix}{pad_idx}"),
                    type_tag: format!("[u8;{pad_len}]"),
                    kind: BamFieldKindId::NESTED,
                    offset: cursor,
                    size: pad_len,
                    align: 1,
                    is_padding: true,
                });
                pad_idx += 1;
            }

            solved_fields.push(SolvedFieldLayout {
                name: f.name.clone(),
                type_tag: f.type_tag.clone(),
                kind: f.kind,
                offset: aligned_off,
                size: f.size,
                align: eff_align,
                is_padding: false,
            });
            cursor = next_cursor;
        }

        let total_size = align_up(cursor, max_align).ok_or(LayoutError::Overflow)?;
        if total_size > cursor {
            let pad_len = total_size - cursor;
            explicit_pad_bytes = explicit_pad_bytes
                .checked_add(pad_len)
                .ok_or(LayoutError::Overflow)?;
            solved_fields.push(SolvedFieldLayout {
                name: format!("{pad_prefix}{pad_idx}"),
                type_tag: format!("[u8;{pad_len}]"),
                kind: BamFieldKindId::NESTED,
                offset: cursor,
                size: pad_len,
                align: 1,
                is_padding: true,
            });
        }

        let hash_inputs: Vec<SchemaFieldInput<'_>> = solved_fields
            .iter()
            .map(|sf| SchemaFieldInput {
                kind_code: sf.kind.0,
                name: sf.name.as_str(),
                type_tag: sf.type_tag.as_str(),
                offset: sf.offset as u64,
                size: sf.size as u64,
                align: sf.align as u64,
                include_in_pod_hash: true,
            })
            .collect();

        let uuid_bytes = compute_component_uuid(
            canonical_name,
            total_size as u64,
            max_align as u64,
            is_abi_pod,
            &hash_inputs,
        );
        let hex_arr = format_uuid_hyphenated(&uuid_bytes);
        let uuid_hex = core::str::from_utf8(&hex_arr)
            .unwrap_or_default()
            .to_string();
        let schema_fingerprint = compute_schema_fingerprint(
            canonical_name,
            total_size as u64,
            max_align as u64,
            component_flags,
            &hash_inputs,
        );

        Ok(SolvedStructLayout {
            canonical_name: canonical_name.to_string(),
            fields: solved_fields,
            size: total_size,
            align: max_align,
            is_abi_pod,
            is_raw_insertable,
            explicit_pad_bytes,
            uuid_bytes,
            uuid_hex,
            schema_fingerprint,
        })
    }
}

#[cfg(feature = "alloc")]
pub use alloc_solver::{RawFieldInput, SolvedFieldLayout, SolvedStructLayout, solve_repr_c_layout};
