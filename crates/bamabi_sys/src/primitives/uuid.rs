//! Deterministic 128-bit component identity via `const` SipHash-2-4-128 and layout fingerprinting.

/// Fixed SipHash-2-4-128 domain-separation key half 0 (`"BAM_UUID"` in big-endian ASCII).
pub const SIPHASH_KEY_0: u64 = bamabi_schema::SIPHASH_KEY_0;

/// Fixed SipHash-2-4-128 domain-separation key half 1 (`"V0_2SIPH"` in big-endian ASCII).
pub const SIPHASH_KEY_1: u64 = bamabi_schema::SIPHASH_KEY_1;

/// Computes a `const` 128-bit SipHash-2-4 digest over `data` using `(k0, k1)`.
#[inline]
pub const fn siphash24_128(data: &[u8], k0: u64, k1: u64) -> [u8; 16] {
    bamabi_schema::siphash24_128(data, k0, k1)
}

/// Unique 128-bit component identifier (`typedef struct BamComponentUuid { uint8_t bytes[16]; } BamComponentUuid;`).
///
/// Stored as `[u8; 16]` (`size = 16`, `align = 1` across every 32-bit and 64-bit C ABI, avoiding
/// cross-compiler `u128` or 32-bit `uint64_t` alignment divergence).
///
/// # Example
///
/// ```
/// use bamabi_sys::ComponentUuid;
///
/// let mod_a_health = ComponentUuid::from_parts("mod_a::Health", 4, 4, 0);
/// let mod_b_health = ComponentUuid::from_parts("mod_b::Health", 4, 4, 0);
/// let mod_a_health_f64 = ComponentUuid::from_parts("mod_a::Health", 8, 8, 0);
/// assert_ne!(mod_a_health, mod_b_health);
/// assert_ne!(mod_a_health, mod_a_health_f64);
///
/// let explicit = ComponentUuid::try_parse_hex("00112233-4455-6677-8899-aabbccddeeff")
///     .expect("valid UUID hex");
/// assert_eq!(explicit.to_u128(), 0x00112233_4455_6677_8899_aabbccddeeff);
/// assert_eq!(explicit.to_string(), "00112233-4455-6677-8899-aabbccddeeff");
/// ```
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ComponentUuid {
    /// Raw 16-byte UUID representation.
    pub bytes: [u8; 16],
}

impl ComponentUuid {
    /// All-zero UUID constant.
    pub const ZERO: Self = Self { bytes: [0u8; 16] };

    /// Constructs a `ComponentUuid` directly from a 16-byte array.
    #[inline]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self { bytes }
    }

    /// Borrows the raw 16-byte UUID array.
    #[inline]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.bytes
    }

    /// Constructs a `ComponentUuid` from a `u128` in standard big-endian UUID byte order.
    #[inline]
    pub const fn from_u128(value: u128) -> Self {
        Self {
            bytes: value.to_be_bytes(),
        }
    }

    /// Converts this `ComponentUuid` into a `u128` in standard big-endian UUID byte order.
    #[inline]
    pub const fn to_u128(self) -> u128 {
        u128::from_be_bytes(self.bytes)
    }

    /// Returns `true` if all 16 bytes are zero.
    #[inline]
    pub const fn is_zero(&self) -> bool {
        let mut i = 0;
        while i < 16 {
            if self.bytes[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Formats this UUID as a canonical 36-byte lowercase ASCII `"xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"` array.
    #[inline]
    #[must_use]
    pub const fn format_hex(&self) -> [u8; 36] {
        bamabi_schema::format_uuid_hyphenated(&self.bytes)
    }

    /// Formats this UUID as a compact 32-byte lowercase hexadecimal ASCII array without hyphens.
    #[inline]
    #[must_use]
    pub const fn format_hex_compact(&self) -> [u8; 32] {
        bamabi_schema::format_uuid_compact(&self.bytes)
    }

    /// Computes a deterministic 128-bit UUID from raw canonical name bytes and layout parts
    /// without requiring a UTF-8 check on `canonical_name`.
    #[inline]
    pub const fn from_parts_bytes(
        canonical_name: &[u8],
        size: usize,
        align: usize,
        field_schema_hash: u64,
    ) -> Self {
        Self {
            bytes: bamabi_schema::compute_component_uuid_bytes_raw(
                canonical_name,
                size as u64,
                align as u64,
                field_schema_hash,
            ),
        }
    }

    /// Computes a deterministic 128-bit UUID via `const` SipHash-2-4-128 over:
    /// - `canonical_name.as_bytes()` (`"<namespace>::<Name>"`) + `0xFF` separator,
    /// - `(size as u64).to_le_bytes()`,
    /// - `(align as u64).to_le_bytes()`,
    /// - `field_schema_hash.to_le_bytes()` (`0` for opaque VTable-only components).
    #[inline]
    pub const fn from_parts(
        canonical_name: &str,
        size: usize,
        align: usize,
        field_schema_hash: u64,
    ) -> Self {
        Self {
            bytes: bamabi_schema::compute_component_uuid_bytes(
                canonical_name,
                size as u64,
                align as u64,
                field_schema_hash,
            ),
        }
    }

    /// Alias for [`Self::from_parts`] computing SipHash-2-4-128 over a namespaced component name
    /// and its `(size, align, field_schema_hash)` layout fingerprint.
    #[inline]
    pub const fn from_namespaced(
        canonical_name: &str,
        size: usize,
        align: usize,
        field_schema_hash: u64,
    ) -> Self {
        Self::from_parts(canonical_name, size, align, field_schema_hash)
    }

    /// Computes a deterministic UUID for an opaque or VTable-only component whose host layout is
    /// intentionally hidden across compilers (`size = 0, align = 0, field_schema_hash = 0`).
    #[inline]
    pub const fn from_canonical_name(canonical_name: &str) -> Self {
        Self::from_parts(canonical_name, 0, 0, 0)
    }

    /// Accumulates one `(field_name, field_type_tag, field_offset)` tuple into a deterministic
    /// 64-bit `field_schema_hash` for `#[repr(C)]` `AbiPod` structs.
    #[inline]
    pub const fn hash_field_schema(
        prev_hash: u64,
        field_name: &str,
        field_type_tag: &str,
        field_offset: usize,
    ) -> u64 {
        bamabi_schema::hash_field_schema(prev_hash, field_name, field_type_tag, field_offset as u64)
    }

    /// Parses a UUID from either canonical 36-character hyphenated `"8-4-4-4-12"` hex form or
    /// contiguous 32-character hex form, returning `None` if malformed.
    #[inline]
    pub const fn try_parse_hex(hex: &str) -> Option<Self> {
        match bamabi_schema::try_parse_uuid_hex(hex) {
            Some(bytes) => Some(Self { bytes }),
            None => None,
        }
    }
}

impl core::fmt::Display for ComponentUuid {
    #[inline]
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let buf = self.format_hex();
        let hex_str = core::str::from_utf8(&buf).unwrap_or("00000000-0000-0000-0000-000000000000");
        f.write_str(hex_str)
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for ComponentUuid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let buf = self.format_hex();
        let s = core::str::from_utf8(&buf).map_err(|_| {
            <S::Error as serde::ser::Error>::custom("invalid UTF-8 in UUID hex buffer")
        })?;
        serializer.serialize_str(s)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ComponentUuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct UuidVisitor;

        impl serde::de::Visitor<'_> for UuidVisitor {
            type Value = ComponentUuid;

            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("a 36-character hyphenated or 32-character hex ComponentUuid")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                ComponentUuid::try_parse_hex(v)
                    .ok_or_else(|| E::custom("invalid ComponentUuid hex string"))
            }
        }

        deserializer.deserialize_str(UuidVisitor)
    }
}

const _: () = {
    assert!(core::mem::size_of::<ComponentUuid>() == 16);
    assert!(core::mem::align_of::<ComponentUuid>() == 1);
};

/// 64-bit FNV-1a offset basis (`0xcbf2_9ce4_8422_2325`).
pub const FNV1A_64_OFFSET_BASIS: u64 = bamabi_schema::FNV1A_64_OFFSET_BASIS;

/// 64-bit FNV-1a prime multiplier (`0x0000_0100_0000_01b3`).
pub const FNV1A_64_PRIME: u64 = bamabi_schema::FNV1A_64_PRIME;

/// Returns the initial 64-bit FNV-1a accumulator (`FNV1A_64_OFFSET_BASIS`).
#[inline]
#[must_use]
pub const fn fnv1a_64_init() -> u64 {
    bamabi_schema::fnv1a_64_init()
}

/// Folds `bytes` into an ongoing 64-bit FNV-1a hash in `const` or runtime contexts.
#[inline]
#[must_use]
pub const fn fnv1a_64_update_bytes(hash: u64, bytes: &[u8]) -> u64 {
    bamabi_schema::fnv1a_64_update_bytes(hash, bytes)
}

/// Folds little-endian `value` bytes into an ongoing 64-bit FNV-1a hash.
#[inline]
#[must_use]
pub const fn fnv1a_64_update_u64(hash: u64, value: u64) -> u64 {
    bamabi_schema::fnv1a_64_update_u64(hash, value)
}

/// Computes the 64-bit FNV-1a digest of a UTF-8 string slice.
#[inline]
#[must_use]
pub const fn fnv1a_64_str(s: &str) -> u64 {
    bamabi_schema::fnv1a_64_str(s)
}
