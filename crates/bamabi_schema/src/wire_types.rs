//! Canonical wire primitive and well-known type specification table.
//!
//! Serves as the single source of truth for `(size, align, BamFieldKindId, canonical_tag)`
//! across `bamabi_macros`, `bamabi_sdk_gen`, `bamabi_reflect`, and `strata_sema`.

/// Target ABI pointer width and 64-bit scalar alignment model.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct TargetLayout {
    /// Size and alignment of platform pointers, `usize`, and `isize` in bytes (`4` or `8`).
    pub ptr_bytes: usize,
    /// Natural alignment of `u64`, `i64`, and `f64` in bytes (`4` on i686 SysV, `8` on 64-bit/ARMv7/Wasm32).
    pub u64_align: usize,
}

impl TargetLayout {
    /// Standard 64-bit targets (`x86_64`, `aarch64`, `riscv64` — `ptr = 8`, `u64_align = 8`).
    pub const PTR64: Self = Self {
        ptr_bytes: 8,
        u64_align: 8,
    };

    /// 32-bit targets with 8-byte `u64` alignment (`wasm32`, `armv7`, `x32` — `ptr = 4`, `u64_align = 8`).
    pub const PTR32_U64_ALIGN8: Self = Self {
        ptr_bytes: 4,
        u64_align: 8,
    };

    /// 32-bit x86 SysV targets (`i686-unknown-linux-gnu` — `ptr = 4`, `u64_align = 4`).
    pub const PTR32_U64_ALIGN4: Self = Self {
        ptr_bytes: 4,
        u64_align: 4,
    };

    /// Host compilation target layout derived via `size_of::<usize>()` and `align_of::<u64>()`.
    pub const HOST: Self = Self {
        ptr_bytes: core::mem::size_of::<usize>(),
        u64_align: core::mem::align_of::<u64>(),
    };

    /// Constructs a `TargetLayout` from explicit `(ptr_bytes, u64_align)` parameters.
    #[inline]
    #[must_use]
    pub const fn new(ptr_bytes: usize, u64_align: usize) -> Self {
        Self {
            ptr_bytes,
            u64_align,
        }
    }
}

/// Wire classification code matching `bamabi_sys::BamFieldKind`.
#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct BamFieldKindId(pub u32);

impl BamFieldKindId {
    /// Unclassified or unknown field type (`0`).
    pub const UNKNOWN: Self = Self(0);
    /// `bool` / `FfiBool` (`1`).
    pub const BOOL: Self = Self(1);
    /// Unsigned 8-bit integer `u8` (`2`).
    pub const U8: Self = Self(2);
    /// Signed 8-bit integer `i8` (`3`).
    pub const I8: Self = Self(3);
    /// Unsigned 16-bit integer `u16` (`4`).
    pub const U16: Self = Self(4);
    /// Signed 16-bit integer `i16` (`5`).
    pub const I16: Self = Self(5);
    /// Unsigned 32-bit integer `u32` (`6`).
    pub const U32: Self = Self(6);
    /// Signed 32-bit integer `i32` (`7`).
    pub const I32: Self = Self(7);
    /// Unsigned 64-bit integer `u64` (`8`).
    pub const U64: Self = Self(8);
    /// Signed 64-bit integer `i64` (`9`).
    pub const I64: Self = Self(9);
    /// 32-bit IEEE 754 float `f32` (`10`).
    pub const F32: Self = Self(10);
    /// 64-bit IEEE 754 float `f64` (`11`).
    pub const F64: Self = Self(11);
    /// Platform pointer-sized unsigned integer `usize` (`12`).
    pub const USIZE: Self = Self(12);
    /// Platform pointer-sized signed integer `isize` (`13`).
    pub const ISIZE: Self = Self(13);
    /// Host-owned UTF-8 `String` exposed via `StrView` (`14`).
    pub const STRING: Self = Self(14);
    /// Host-owned typed `Vec<T>` exposed via `FfiSlice<T>` (`15`).
    pub const VEC: Self = Self(15);
    /// Cross-language `FfiOption<T>` (`16`).
    pub const FFI_OPTION: Self = Self(16);
    /// Nested `#[repr(C)]` struct or array (`17`).
    pub const NESTED: Self = Self(17);
    /// Opaque / skipped / non-inspectable field (`18`).
    pub const OPAQUE: Self = Self(18);

    /// Returns `true` if `self` is within `0..=18`.
    #[inline]
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.0 <= Self::OPAQUE.0
    }

    /// Returns the uppercase Rust constant name (`"BOOL"`, `"U32"`, ..., `"OPAQUE"`, or `"UNKNOWN"`).
    #[inline]
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BOOL => "BOOL",
            Self::U8 => "U8",
            Self::I8 => "I8",
            Self::U16 => "U16",
            Self::I16 => "I16",
            Self::U32 => "U32",
            Self::I32 => "I32",
            Self::U64 => "U64",
            Self::I64 => "I64",
            Self::F32 => "F32",
            Self::F64 => "F64",
            Self::USIZE => "USIZE",
            Self::ISIZE => "ISIZE",
            Self::STRING => "STRING",
            Self::VEC => "VEC",
            Self::FFI_OPTION => "FFI_OPTION",
            Self::NESTED => "NESTED",
            Self::OPAQUE => "OPAQUE",
            _ => "UNKNOWN",
        }
    }
}

/// Canonical wire specification for a built-in scalar or well-known ABI type.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct WireTypeSpec {
    /// Normalized canonical type tag (`"u32"`, `"FfiEntity"`, `"FfiAssetHandle"`, ...).
    pub canonical_tag: &'static str,
    /// Target-dependent byte size.
    pub size: usize,
    /// Target-dependent byte alignment.
    pub align: usize,
    /// Canonical [`BamFieldKindId`].
    pub kind: BamFieldKindId,
    /// `true` iff bit-exact zero-padding-free `AbiPod` wire transfer is valid.
    pub is_abi_pod: bool,
    /// `true` iff direct `RawInsertable` wire storage transfer is valid.
    pub is_raw_insertable: bool,
    /// `true` for Rust primitive `bool` (`FfiBool` marshaling).
    pub is_bool: bool,
    /// `true` for host-heap-owned `String`.
    pub is_host_string: bool,
}

#[inline]
const fn str_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut i = 0;
    while i < ab.len() {
        if ab[i] != bb[i] {
            return false;
        }
        i += 1;
    }
    true
}

#[inline]
const fn spec(
    canonical_tag: &'static str,
    size: usize,
    align: usize,
    kind: BamFieldKindId,
    is_abi_pod: bool,
    is_raw_insertable: bool,
) -> WireTypeSpec {
    WireTypeSpec {
        canonical_tag,
        size,
        align,
        kind,
        is_abi_pod,
        is_raw_insertable,
        is_bool: false,
        is_host_string: false,
    }
}

/// Exhaustive list of canonical built-in primitive and well-known wire type names supported by
/// [`lookup_wire_type`].
pub const WIRE_TYPE_NAMES: &[&str] = &[
    "bool",
    "u8",
    "i8",
    "u16",
    "i16",
    "u32",
    "i32",
    "f32",
    "u64",
    "i64",
    "f64",
    "usize",
    "isize",
    "u128",
    "i128",
    "FfiBool",
    "FfiEntity",
    "TargetEntity",
    "BamComponent",
    "BamSystem",
    "BamSystemSet",
    "BamSymbol",
    "FfiAssetLoadState",
    "BamEventKind",
    "BamFieldKind",
    "FfiAssetHandle",
    "FfiChildOf",
    "ComponentUuid",
    "BamChangeTicks",
    "FfiDisabled",
    "BamRowRange",
    "BamTimeInfo",
    "String",
];

/// Looks up a built-in primitive or well-known `bamabi` wire type by unqualified name for `target`.
///
/// Guarantees deterministic `(size, align, BamFieldKindId, canonical_tag)` parity across
/// `bamabi_macros`, `bamabi_sdk_gen`, `bamabi_reflect`, and `strata_sema`, including
/// `FfiAssetHandle` (`size = 32, align = 4, BamFieldKindId::NESTED, "FfiAssetHandle"`),
/// `FfiEntity`, `TargetEntity`, `FfiChildOf`, `ComponentUuid`, `BamChangeTicks`, `FfiDisabled`,
/// `BamRowRange`, and `BamTimeInfo`.
#[must_use]
pub const fn lookup_wire_type(name: &str, target: TargetLayout) -> Option<WireTypeSpec> {
    if str_eq(name, "bool") {
        let mut s = spec("bool", 1, 1, BamFieldKindId::BOOL, false, false);
        s.is_bool = true;
        return Some(s);
    }
    if str_eq(name, "u8") {
        return Some(spec("u8", 1, 1, BamFieldKindId::U8, true, true));
    }
    if str_eq(name, "i8") {
        return Some(spec("i8", 1, 1, BamFieldKindId::I8, true, true));
    }
    if str_eq(name, "u16") {
        return Some(spec("u16", 2, 2, BamFieldKindId::U16, true, true));
    }
    if str_eq(name, "i16") {
        return Some(spec("i16", 2, 2, BamFieldKindId::I16, true, true));
    }
    if str_eq(name, "u32") {
        return Some(spec("u32", 4, 4, BamFieldKindId::U32, true, true));
    }
    if str_eq(name, "i32") {
        return Some(spec("i32", 4, 4, BamFieldKindId::I32, true, true));
    }
    if str_eq(name, "f32") {
        return Some(spec("f32", 4, 4, BamFieldKindId::F32, true, true));
    }
    if str_eq(name, "u64") {
        return Some(spec(
            "u64",
            8,
            target.u64_align,
            BamFieldKindId::U64,
            true,
            true,
        ));
    }
    if str_eq(name, "i64") {
        return Some(spec(
            "i64",
            8,
            target.u64_align,
            BamFieldKindId::I64,
            true,
            true,
        ));
    }
    if str_eq(name, "f64") {
        return Some(spec(
            "f64",
            8,
            target.u64_align,
            BamFieldKindId::F64,
            true,
            true,
        ));
    }
    if str_eq(name, "usize") {
        return Some(spec(
            "usize",
            target.ptr_bytes,
            target.ptr_bytes,
            BamFieldKindId::USIZE,
            false,
            true,
        ));
    }
    if str_eq(name, "isize") {
        return Some(spec(
            "isize",
            target.ptr_bytes,
            target.ptr_bytes,
            BamFieldKindId::ISIZE,
            false,
            true,
        ));
    }
    if str_eq(name, "u128") {
        return Some(spec("u128", 16, 16, BamFieldKindId::NESTED, false, true));
    }
    if str_eq(name, "i128") {
        return Some(spec("i128", 16, 16, BamFieldKindId::NESTED, false, true));
    }
    if str_eq(name, "FfiBool") {
        return Some(spec("FfiBool", 1, 1, BamFieldKindId::BOOL, true, true));
    }
    if str_eq(name, "FfiEntity") {
        return Some(spec(
            "FfiEntity",
            8,
            target.u64_align,
            BamFieldKindId::U64,
            true,
            true,
        ));
    }
    if str_eq(name, "TargetEntity") {
        return Some(spec(
            "TargetEntity",
            8,
            target.u64_align,
            BamFieldKindId::U64,
            true,
            true,
        ));
    }
    if str_eq(name, "BamComponent") {
        return Some(spec("BamComponent", 4, 4, BamFieldKindId::U32, true, true));
    }
    if str_eq(name, "BamSystem") {
        return Some(spec("BamSystem", 4, 4, BamFieldKindId::U32, true, true));
    }
    if str_eq(name, "BamSystemSet") {
        return Some(spec("BamSystemSet", 4, 4, BamFieldKindId::U32, true, true));
    }
    if str_eq(name, "BamSymbol") {
        return Some(spec("BamSymbol", 4, 4, BamFieldKindId::U32, true, true));
    }
    if str_eq(name, "FfiAssetLoadState") {
        return Some(spec(
            "FfiAssetLoadState",
            4,
            4,
            BamFieldKindId::U32,
            true,
            true,
        ));
    }
    if str_eq(name, "BamEventKind") {
        return Some(spec("BamEventKind", 4, 4, BamFieldKindId::U32, true, true));
    }
    if str_eq(name, "BamFieldKind") {
        return Some(spec("BamFieldKind", 4, 4, BamFieldKindId::U32, true, true));
    }
    if str_eq(name, "FfiAssetHandle") {
        return Some(spec(
            "FfiAssetHandle",
            32,
            4,
            BamFieldKindId::NESTED,
            true,
            true,
        ));
    }
    if str_eq(name, "FfiChildOf") {
        return Some(spec(
            "FfiChildOf",
            8,
            target.u64_align,
            BamFieldKindId::NESTED,
            true,
            true,
        ));
    }
    if str_eq(name, "ComponentUuid") {
        return Some(spec(
            "ComponentUuid",
            16,
            1,
            BamFieldKindId::NESTED,
            true,
            true,
        ));
    }
    if str_eq(name, "BamChangeTicks") {
        return Some(spec(
            "BamChangeTicks",
            16,
            4,
            BamFieldKindId::NESTED,
            true,
            true,
        ));
    }
    if str_eq(name, "FfiDisabled") {
        return Some(spec(
            "FfiDisabled",
            0,
            1,
            BamFieldKindId::NESTED,
            true,
            true,
        ));
    }
    if str_eq(name, "BamRowRange") {
        return Some(spec(
            "BamRowRange",
            target.ptr_bytes * 2,
            target.ptr_bytes,
            BamFieldKindId::NESTED,
            true,
            true,
        ));
    }
    if str_eq(name, "BamTimeInfo") {
        return Some(spec(
            "BamTimeInfo",
            32,
            target.u64_align,
            BamFieldKindId::NESTED,
            true,
            true,
        ));
    }
    if str_eq(name, "String") {
        let mut s = spec(
            "String",
            target.ptr_bytes * 3,
            target.ptr_bytes,
            BamFieldKindId::STRING,
            false,
            false,
        );
        s.is_host_string = true;
        return Some(s);
    }
    None
}

/// Returns `true` if `name` is a Rust standard-library container or smart pointer whose layout
/// is unstable across compilers and therefore banned from direct C-ABI wire exposure.
#[must_use]
pub const fn is_banned_rust_wire_type(name: &str) -> bool {
    str_eq(name, "Box")
        || str_eq(name, "Rc")
        || str_eq(name, "Arc")
        || str_eq(name, "Option")
        || str_eq(name, "Result")
        || str_eq(name, "HashMap")
        || str_eq(name, "BTreeMap")
        || str_eq(name, "HashSet")
        || str_eq(name, "BTreeSet")
        || str_eq(name, "Cow")
        || str_eq(name, "VecDeque")
}

/// Canonicalizes a nested struct/enum type path by stripping any module qualifiers and returning
/// the final segment identifier if valid.
#[must_use]
pub fn canonical_leaf_ident(path: &str) -> &str {
    let trimmed = path.trim();
    match trimmed.rsplit("::").next() {
        Some(last) if !last.is_empty() => last.trim(),
        _ => trimmed,
    }
}

#[cfg(feature = "alloc")]
extern crate alloc;

/// Formats canonical nested `FfiOption<T>` type tag (`"FfiOption<inner>"`).
#[cfg(feature = "alloc")]
#[must_use]
pub fn canonical_ffi_option_tag(inner_tag: &str) -> alloc::string::String {
    alloc::format!("FfiOption<{}>", inner_tag.trim())
}

/// Formats canonical `Vec<T>` type tag (`"Vec<elem>"`).
#[cfg(feature = "alloc")]
#[must_use]
pub fn canonical_vec_tag(elem_tag: &str) -> alloc::string::String {
    alloc::format!("Vec<{}>", elem_tag.trim())
}

/// Formats canonical fixed-size array `[T; N]` type tag (`"[elem;len]"`).
#[cfg(feature = "alloc")]
#[must_use]
pub fn canonical_array_tag(elem_tag: &str, len_expr: &str) -> alloc::string::String {
    let clean_len: alloc::string::String =
        len_expr.chars().filter(|c| !c.is_whitespace()).collect();
    alloc::format!("[{};{}]", elem_tag.trim(), clean_len)
}
