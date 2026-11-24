//! Deterministic 128-bit SipHash-2-4-128 component identity, FNV-1a-64 schema fingerprinting,
//! and zero-allocation UUID hex parsing/formatting.

/// Fixed SipHash-2-4-128 domain-separation key half 0 (`"BAM_UUID"` in big-endian ASCII).
pub const SIPHASH_KEY_0: u64 = 0x4241_4d5f_5555_4944;

/// Fixed SipHash-2-4-128 domain-separation key half 1 (`"V0_2SIPH"` in big-endian ASCII).
pub const SIPHASH_KEY_1: u64 = 0x5630_5f32_5349_5048;

/// 64-bit FNV-1a offset basis (`0xcbf2_9ce4_8422_2325`).
pub const FNV1A_64_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// 64-bit FNV-1a prime multiplier (`0x0000_0100_0000_01b3`).
pub const FNV1A_64_PRIME: u64 = 0x0000_0100_0000_01b3;

#[inline]
const fn sipround(v0: u64, v1: u64, v2: u64, v3: u64) -> (u64, u64, u64, u64) {
    let v0 = v0.wrapping_add(v1);
    let v1 = v1.rotate_left(13) ^ v0;
    let v0 = v0.rotate_left(32);
    let v2 = v2.wrapping_add(v3);
    let v3 = v3.rotate_left(16) ^ v2;
    let v0 = v0.wrapping_add(v3);
    let v3 = v3.rotate_left(21) ^ v0;
    let v2 = v2.wrapping_add(v1);
    let v1 = v1.rotate_left(17) ^ v2;
    let v2 = v2.rotate_left(32);
    (v0, v1, v2, v3)
}

#[derive(Copy, Clone)]
struct ConstSipHasher128 {
    v0: u64,
    v1: u64,
    v2: u64,
    v3: u64,
    tail: [u8; 8],
    tail_len: usize,
    total_len: usize,
}

impl ConstSipHasher128 {
    #[inline]
    const fn new(k0: u64, k1: u64) -> Self {
        Self {
            v0: 0x736f_6d65_7073_6575 ^ k0,
            v1: (0x646f_7261_6e64_6f6d ^ k1) ^ 0xee,
            v2: 0x6c79_6765_6e65_7261 ^ k0,
            v3: 0x7465_6462_7974_6573 ^ k1,
            tail: [0u8; 8],
            tail_len: 0,
            total_len: 0,
        }
    }

    #[inline]
    const fn compress_word(mut self, m: u64) -> Self {
        self.v3 ^= m;
        (self.v0, self.v1, self.v2, self.v3) = sipround(self.v0, self.v1, self.v2, self.v3);
        (self.v0, self.v1, self.v2, self.v3) = sipround(self.v0, self.v1, self.v2, self.v3);
        self.v0 ^= m;
        self
    }

    #[inline]
    const fn write(mut self, bytes: &[u8]) -> Self {
        let mut idx = 0;
        self.total_len = self.total_len.wrapping_add(bytes.len());

        if self.tail_len > 0 {
            while idx < bytes.len() && self.tail_len < 8 {
                self.tail[self.tail_len] = bytes[idx];
                self.tail_len += 1;
                idx += 1;
            }
            if self.tail_len == 8 {
                let m = u64::from_le_bytes(self.tail);
                self = self.compress_word(m);
                self.tail = [0u8; 8];
                self.tail_len = 0;
            }
        }

        while idx + 8 <= bytes.len() {
            let m = u64::from_le_bytes([
                bytes[idx],
                bytes[idx + 1],
                bytes[idx + 2],
                bytes[idx + 3],
                bytes[idx + 4],
                bytes[idx + 5],
                bytes[idx + 6],
                bytes[idx + 7],
            ]);
            self = self.compress_word(m);
            idx += 8;
        }

        while idx < bytes.len() {
            self.tail[self.tail_len] = bytes[idx];
            self.tail_len += 1;
            idx += 1;
        }

        self
    }

    #[inline]
    const fn finish(mut self) -> [u8; 16] {
        let mut b = ((self.total_len as u64) & 0xff) << 56;
        let mut i = 0;
        while i < self.tail_len {
            b |= (self.tail[i] as u64) << (8 * i);
            i += 1;
        }
        self = self.compress_word(b);

        self.v2 ^= 0xee;
        (self.v0, self.v1, self.v2, self.v3) = sipround(self.v0, self.v1, self.v2, self.v3);
        (self.v0, self.v1, self.v2, self.v3) = sipround(self.v0, self.v1, self.v2, self.v3);
        (self.v0, self.v1, self.v2, self.v3) = sipround(self.v0, self.v1, self.v2, self.v3);
        (self.v0, self.v1, self.v2, self.v3) = sipround(self.v0, self.v1, self.v2, self.v3);
        let h0 = self.v0 ^ self.v1 ^ self.v2 ^ self.v3;

        self.v1 ^= 0xdd;
        (self.v0, self.v1, self.v2, self.v3) = sipround(self.v0, self.v1, self.v2, self.v3);
        (self.v0, self.v1, self.v2, self.v3) = sipround(self.v0, self.v1, self.v2, self.v3);
        (self.v0, self.v1, self.v2, self.v3) = sipround(self.v0, self.v1, self.v2, self.v3);
        (self.v0, self.v1, self.v2, self.v3) = sipround(self.v0, self.v1, self.v2, self.v3);
        let h1 = self.v0 ^ self.v1 ^ self.v2 ^ self.v3;

        let b0 = h0.to_le_bytes();
        let b1 = h1.to_le_bytes();
        [
            b0[0], b0[1], b0[2], b0[3], b0[4], b0[5], b0[6], b0[7], b1[0], b1[1], b1[2], b1[3],
            b1[4], b1[5], b1[6], b1[7],
        ]
    }
}

/// Computes a `const` 128-bit SipHash-2-4 digest over `data` using `(k0, k1)`.
#[inline]
#[must_use]
pub const fn siphash24_128(data: &[u8], k0: u64, k1: u64) -> [u8; 16] {
    ConstSipHasher128::new(k0, k1).write(data).finish()
}

/// Returns the initial 64-bit FNV-1a accumulator (`FNV1A_64_OFFSET_BASIS`).
#[inline]
#[must_use]
pub const fn fnv1a_64_init() -> u64 {
    FNV1A_64_OFFSET_BASIS
}

/// Folds `bytes` into an ongoing 64-bit FNV-1a hash in `const` or runtime contexts.
#[inline]
#[must_use]
pub const fn fnv1a_64_update_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(FNV1A_64_PRIME);
        i += 1;
    }
    hash
}

/// Folds little-endian `value` bytes into an ongoing 64-bit FNV-1a hash.
#[inline]
#[must_use]
pub const fn fnv1a_64_update_u64(hash: u64, value: u64) -> u64 {
    fnv1a_64_update_bytes(hash, &value.to_le_bytes())
}

/// Computes the 64-bit FNV-1a digest of a UTF-8 string slice.
#[inline]
#[must_use]
pub const fn fnv1a_64_str(s: &str) -> u64 {
    fnv1a_64_update_bytes(fnv1a_64_init(), s.as_bytes())
}

/// Accumulates one `(field_name, field_type_tag, field_offset)` tuple into a deterministic
/// 64-bit `field_schema_hash` for `#[repr(C)]` `AbiPod` structs and enums.
#[inline]
#[must_use]
pub const fn hash_field_schema(
    prev_hash: u64,
    field_name: &str,
    field_type_tag: &str,
    field_offset: u64,
) -> u64 {
    let prev_le = prev_hash.to_le_bytes();
    let offset_le = field_offset.to_le_bytes();
    let digest = ConstSipHasher128::new(SIPHASH_KEY_0, SIPHASH_KEY_1)
        .write(&prev_le)
        .write(field_name.as_bytes())
        .write(&[0xFE])
        .write(field_type_tag.as_bytes())
        .write(&[0xFD])
        .write(&offset_le)
        .finish();
    u64::from_le_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
}

/// Computes a deterministic 128-bit component UUID from raw `(canonical_name, size, align, field_schema_hash)` bytes.
#[inline]
#[must_use]
pub const fn compute_component_uuid_bytes_raw(
    canonical_name: &[u8],
    size: u64,
    align: u64,
    field_schema_hash: u64,
) -> [u8; 16] {
    let size_le = size.to_le_bytes();
    let align_le = align.to_le_bytes();
    let schema_le = field_schema_hash.to_le_bytes();
    ConstSipHasher128::new(SIPHASH_KEY_0, SIPHASH_KEY_1)
        .write(canonical_name)
        .write(&[0xFF])
        .write(&size_le)
        .write(&align_le)
        .write(&schema_le)
        .finish()
}

/// Computes a deterministic 128-bit component UUID from `(canonical_name, size, align, field_schema_hash)`.
#[inline]
#[must_use]
pub const fn compute_component_uuid_bytes(
    canonical_name: &str,
    size: u64,
    align: u64,
    field_schema_hash: u64,
) -> [u8; 16] {
    compute_component_uuid_bytes_raw(canonical_name.as_bytes(), size, align, field_schema_hash)
}

/// Descriptor for one field passed into [`compute_component_uuid`] or [`compute_schema_fingerprint`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SchemaFieldInput<'a> {
    /// Numeric `BamFieldKind` value (`0..=18`).
    pub kind_code: u32,
    /// Field identifier (or tuple index string).
    pub name: &'a str,
    /// Canonical normalized type tag (matching [`crate::wire_types`]).
    pub type_tag: &'a str,
    /// Byte offset within the enclosing `#[repr(C)]` struct (`0` for non-POD / skipped placeholders).
    pub offset: u64,
    /// Byte size of the field.
    pub size: u64,
    /// Byte alignment of the field.
    pub align: u64,
    /// Whether this field participates in the `AbiPod` `field_schema_hash` (`true` for non-skipped fields).
    pub include_in_pod_hash: bool,
}

/// Computes the deterministic `field_schema_hash` and 128-bit UUID bytes for a struct component.
///
/// When `is_abi_pod` is `false` (opaque, skipped, or host-heap-owning components), the UUID
/// hashes `(canonical_name, 0, 0, 0)` to match [`compute_component_uuid_bytes(canonical_name, 0, 0, 0)`].
#[inline]
#[must_use]
pub const fn compute_component_uuid(
    canonical_name: &str,
    size: u64,
    align: u64,
    is_abi_pod: bool,
    fields: &[SchemaFieldInput<'_>],
) -> [u8; 16] {
    if !is_abi_pod {
        return compute_component_uuid_bytes(canonical_name, 0, 0, 0);
    }
    let mut schema_hash = 0u64;
    let mut i = 0;
    while i < fields.len() {
        let f = &fields[i];
        if f.include_in_pod_hash {
            schema_hash = hash_field_schema(schema_hash, f.name, f.type_tag, f.offset);
        }
        i += 1;
    }
    compute_component_uuid_bytes(canonical_name, size, align, schema_hash)
}

/// Computes the deterministic 64-bit FNV-1a `SCHEMA_FINGERPRINT` for a struct component.
///
/// Matches the canonical sequence shared by `bamabi_macros`, `bamabi_sdk_gen`, `bamabi_reflect`,
/// and `strata_sema`.
#[inline]
#[must_use]
pub const fn compute_schema_fingerprint(
    canonical_name: &str,
    size: u64,
    align: u64,
    component_flags: u32,
    fields: &[SchemaFieldInput<'_>],
) -> u64 {
    let mut h = fnv1a_64_init();
    h = fnv1a_64_update_bytes(h, canonical_name.as_bytes());
    h = fnv1a_64_update_u64(h, size);
    h = fnv1a_64_update_u64(h, align);
    h = fnv1a_64_update_u64(h, component_flags as u64);
    let mut i = 0;
    while i < fields.len() {
        let f = &fields[i];
        h = fnv1a_64_update_u64(h, f.kind_code as u64);
        h = fnv1a_64_update_bytes(h, f.name.as_bytes());
        h = fnv1a_64_update_bytes(h, f.type_tag.as_bytes());
        h = fnv1a_64_update_u64(h, f.offset);
        h = fnv1a_64_update_u64(h, f.size);
        h = fnv1a_64_update_u64(h, f.align);
        i += 1;
    }
    h
}

/// Enum variant descriptor passed into [`compute_enum_schema_hash`] and [`compute_enum_schema_fingerprint`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct EnumVariantSchemaInput<'a> {
    /// Variant identifier.
    pub name: &'a str,
    /// Explicit or sequential enum discriminant.
    pub discriminant: i64,
    /// Optional variant payload `(field_name, canonical_type_tag)` pairs.
    pub fields: &'a [(&'a str, &'a str)],
}

/// Computes the deterministic `field_schema_hash` for a `#[repr(u8/u16/i32/u32)]` enum component.
#[must_use]
pub const fn compute_enum_schema_hash(
    int_repr_tag: &str,
    variants: &[EnumVariantSchemaInput<'_>],
) -> u64 {
    let mut schema_hash = hash_field_schema(0, "__enum_tag", int_repr_tag, 0);
    let mut vi = 0;
    while vi < variants.len() {
        let v = &variants[vi];
        schema_hash = hash_field_schema_i64_variant(schema_hash, v.name, v.discriminant);
        let mut fi = 0;
        while fi < v.fields.len() {
            let (f_name, f_tag) = v.fields[fi];
            schema_hash = hash_variant_field_schema(schema_hash, v.name, f_name, f_tag, fi as u64);
            fi += 1;
        }
        vi += 1;
    }
    schema_hash
}

/// Folds an enum variant `(variant_name, discriminant)` using `"variant:<discriminant>"` tag formatting.
#[inline]
const fn hash_field_schema_i64_variant(
    prev_hash: u64,
    variant_name: &str,
    discriminant: i64,
) -> u64 {
    let mut buf = [0u8; 32];
    let prefix = b"variant:";
    let mut len = prefix.len();
    let mut pi = 0;
    while pi < prefix.len() {
        buf[pi] = prefix[pi];
        pi += 1;
    }
    let mut abs = discriminant.unsigned_abs();
    if discriminant < 0 {
        buf[len] = b'-';
        len += 1;
    }
    let mut digits = [0u8; 20];
    let mut dlen = 0;
    if abs == 0 {
        digits[0] = b'0';
        dlen = 1;
    } else {
        while abs > 0 {
            digits[dlen] = b'0' + (abs % 10) as u8;
            abs /= 10;
            dlen += 1;
        }
    }
    while dlen > 0 {
        dlen -= 1;
        buf[len] = digits[dlen];
        len += 1;
    }
    // SAFETY invariant avoided cleanly: build tag slice using `core::str::from_utf8` without unsafe.
    let mut tag_str = "";
    if let Ok(s) = core::str::from_utf8(buf.split_at(len).0) {
        tag_str = s;
    }
    hash_field_schema(prev_hash, variant_name, tag_str, 0)
}

/// Folds a variant payload field `(variant_name::field_name, field_type_tag, field_index)` without allocation.
#[inline]
const fn hash_variant_field_schema(
    prev_hash: u64,
    variant_name: &str,
    field_name: &str,
    field_type_tag: &str,
    field_index: u64,
) -> u64 {
    let prev_le = prev_hash.to_le_bytes();
    let offset_le = field_index.to_le_bytes();
    let digest = ConstSipHasher128::new(SIPHASH_KEY_0, SIPHASH_KEY_1)
        .write(&prev_le)
        .write(variant_name.as_bytes())
        .write(b"::")
        .write(field_name.as_bytes())
        .write(&[0xFE])
        .write(field_type_tag.as_bytes())
        .write(&[0xFD])
        .write(&offset_le)
        .finish();
    u64::from_le_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
}

/// Computes the deterministic 64-bit FNV-1a `SCHEMA_FINGERPRINT` for an enum component.
#[must_use]
pub const fn compute_enum_schema_fingerprint(
    canonical_name: &str,
    size: u64,
    align: u64,
    component_flags: u32,
    int_repr_tag: &str,
    variants: &[EnumVariantSchemaInput<'_>],
) -> u64 {
    let mut h = fnv1a_64_init();
    h = fnv1a_64_update_bytes(h, canonical_name.as_bytes());
    h = fnv1a_64_update_u64(h, size);
    h = fnv1a_64_update_u64(h, align);
    h = fnv1a_64_update_u64(h, component_flags as u64);
    h = fnv1a_64_update_bytes(h, int_repr_tag.as_bytes());
    let mut vi = 0;
    while vi < variants.len() {
        let v = &variants[vi];
        h = fnv1a_64_update_bytes(h, v.name.as_bytes());
        h = fnv1a_64_update_u64(h, v.discriminant as u64);
        let mut fi = 0;
        while fi < v.fields.len() {
            let (f_name, f_tag) = v.fields[fi];
            h = fnv1a_64_update_bytes(h, f_name.as_bytes());
            h = fnv1a_64_update_bytes(h, f_tag.as_bytes());
            fi += 1;
        }
        vi += 1;
    }
    h
}

#[inline]
const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[inline]
const fn lower_hex_digit(nibble: u8) -> u8 {
    match nibble & 0x0f {
        0..=9 => b'0' + (nibble & 0x0f),
        _ => b'a' + ((nibble & 0x0f) - 10),
    }
}

/// Formats 16 UUID bytes into canonical lowercase 36-byte `"xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"` UTF-8 ASCII.
#[inline]
#[must_use]
pub const fn format_uuid_hyphenated(bytes: &[u8; 16]) -> [u8; 36] {
    let mut out = [b'0'; 36];
    let mut dst = 0;
    let mut i = 0;
    while i < 16 {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            out[dst] = b'-';
            dst += 1;
        }
        out[dst] = lower_hex_digit(bytes[i] >> 4);
        out[dst + 1] = lower_hex_digit(bytes[i] & 0x0f);
        dst += 2;
        i += 1;
    }
    out
}

/// Formats 16 UUID bytes into compact lowercase 32-byte hexadecimal UTF-8 ASCII without hyphens.
#[inline]
#[must_use]
pub const fn format_uuid_compact(bytes: &[u8; 16]) -> [u8; 32] {
    let mut out = [b'0'; 32];
    let mut i = 0;
    while i < 16 {
        out[i * 2] = lower_hex_digit(bytes[i] >> 4);
        out[i * 2 + 1] = lower_hex_digit(bytes[i] & 0x0f);
        i += 1;
    }
    out
}

/// Parses a UUID from either canonical 36-character `"8-4-4-4-12"` or 32-character compact hex.
#[inline]
#[must_use]
pub const fn try_parse_uuid_hex(hex: &str) -> Option<[u8; 16]> {
    let src = hex.as_bytes();
    let hyphenated = match src.len() {
        36 => {
            if src[8] != b'-' || src[13] != b'-' || src[18] != b'-' || src[23] != b'-' {
                return None;
            }
            true
        }
        32 => false,
        _ => return None,
    };

    let mut out = [0u8; 16];
    let mut src_idx = 0;
    let mut byte_idx = 0;
    while byte_idx < 16 {
        if hyphenated && (src_idx == 8 || src_idx == 13 || src_idx == 18 || src_idx == 23) {
            src_idx += 1;
        }
        let Some(hi) = hex_nibble(src[src_idx]) else {
            return None;
        };
        let Some(lo) = hex_nibble(src[src_idx + 1]) else {
            return None;
        };
        out[byte_idx] = (hi << 4) | lo;
        src_idx += 2;
        byte_idx += 1;
    }
    Some(out)
}
