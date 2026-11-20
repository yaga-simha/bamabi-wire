//! Property-based tests for `bamabi_sys` wire primitives (`siphash24_128`, `ConstSipHasher128`
//! multi-chunk hashing via `ComponentUuid`, and `ComponentUuid::try_parse_hex`).

use bamabi_sys::{ComponentUuid, SIPHASH_KEY_0, SIPHASH_KEY_1, siphash24_128};
use proptest::prelude::*;

/// Independent reference SipHash-2-4-128 implementation (Aumasson & Bernstein, DOUBLE mode).
fn ref_sipround(v: &mut [u64; 4]) {
    v[0] = v[0].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(13);
    v[1] ^= v[0];
    v[0] = v[0].rotate_left(32);

    v[2] = v[2].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(16);
    v[3] ^= v[2];

    v[0] = v[0].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(21);
    v[3] ^= v[0];

    v[2] = v[2].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(17);
    v[1] ^= v[2];
    v[2] = v[2].rotate_left(32);
}

fn ref_siphash24_128(data: &[u8], k0: u64, k1: u64) -> [u8; 16] {
    let mut v = [
        0x736f_6d65_7073_6575_u64 ^ k0,
        0x646f_7261_6e64_6f6d_u64 ^ k1 ^ 0xee_u64,
        0x6c79_6765_6e65_7261_u64 ^ k0,
        0x7465_6462_7974_6573_u64 ^ k1,
    ];

    let mut chunks = data.chunks_exact(8);
    for block in &mut chunks {
        let m = u64::from_le_bytes(block.try_into().expect("8-byte chunk"));
        v[3] ^= m;
        for _ in 0..2 {
            ref_sipround(&mut v);
        }
        v[0] ^= m;
    }

    let rem = chunks.remainder();
    let mut last_block = ((data.len() as u64) & 0xff) << 56;
    for (idx, &byte) in rem.iter().enumerate() {
        last_block |= u64::from(byte) << (idx * 8);
    }
    v[3] ^= last_block;
    for _ in 0..2 {
        ref_sipround(&mut v);
    }
    v[0] ^= last_block;

    v[2] ^= 0xee;
    for _ in 0..4 {
        ref_sipround(&mut v);
    }
    let h0 = v[0] ^ v[1] ^ v[2] ^ v[3];

    v[1] ^= 0xdd;
    for _ in 0..4 {
        ref_sipround(&mut v);
    }
    let h1 = v[0] ^ v[1] ^ v[2] ^ v[3];

    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&h0.to_le_bytes());
    out[8..].copy_from_slice(&h1.to_le_bytes());
    out
}

/// Independent reference UUID hex parser matching canonical 36-char (`8-4-4-4-12`) or 32-char hex.
fn ref_parse_uuid_hex(input: &str) -> Option<ComponentUuid> {
    if !input.is_ascii() {
        return None;
    }
    let compact: String = match input.len() {
        32 => input.to_owned(),
        36 => {
            let bytes = input.as_bytes();
            if bytes[8] != b'-' || bytes[13] != b'-' || bytes[18] != b'-' || bytes[23] != b'-' {
                return None;
            }
            let mut s = String::with_capacity(32);
            s.push_str(&input[..8]);
            s.push_str(&input[9..13]);
            s.push_str(&input[14..18]);
            s.push_str(&input[19..23]);
            s.push_str(&input[24..36]);
            s
        }
        _ => return None,
    };
    if !compact.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&compact[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(ComponentUuid::from_bytes(out))
}

fn format_uuid_hex(bytes: [u8; 16], hyphenated: bool, upper_nibbles: [bool; 32]) -> String {
    const LOWER: &[u8; 16] = b"0123456789abcdef";
    const UPPER: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(if hyphenated { 36 } else { 32 });
    let mut nibble_idx = 0usize;
    for (byte_idx, &b) in bytes.iter().enumerate() {
        if hyphenated && matches!(byte_idx, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        let hi = usize::from(b >> 4);
        let lo = usize::from(b & 0x0f);
        let hi_table = if upper_nibbles[nibble_idx] {
            UPPER
        } else {
            LOWER
        };
        let lo_table = if upper_nibbles[nibble_idx + 1] {
            UPPER
        } else {
            LOWER
        };
        s.push(char::from(hi_table[hi]));
        s.push(char::from(lo_table[lo]));
        nibble_idx += 2;
    }
    s
}

#[derive(Debug, Clone)]
enum HexMutation {
    ReplaceChar { idx: usize, ch: char },
    InsertChar { idx: usize, ch: char },
    DeleteChar { idx: usize },
    SwapChars { a: usize, b: usize },
    Truncate { len: usize },
}

fn mutation_strategy() -> impl Strategy<Value = HexMutation> {
    let weird_chars = prop_oneof![
        any::<char>(),
        prop::sample::select(vec![
            '0', '9', 'a', 'f', 'g', 'z', 'A', 'F', 'G', '-', '_', ' ', '\0', 'é', '€', '🦀',
        ]),
    ];
    prop_oneof![
        (0usize..48, weird_chars.clone())
            .prop_map(|(idx, ch)| HexMutation::ReplaceChar { idx, ch }),
        (0usize..48, weird_chars).prop_map(|(idx, ch)| HexMutation::InsertChar { idx, ch }),
        (0usize..48).prop_map(|idx| HexMutation::DeleteChar { idx }),
        (0usize..48, 0usize..48).prop_map(|(a, b)| HexMutation::SwapChars { a, b }),
        (0usize..40).prop_map(|len| HexMutation::Truncate { len }),
    ]
}

fn apply_mutation(s: &mut String, m: HexMutation) {
    let mut chars: Vec<char> = s.chars().collect();
    match m {
        HexMutation::ReplaceChar { idx, ch } if !chars.is_empty() => {
            let i = idx % chars.len();
            chars[i] = ch;
        }
        HexMutation::InsertChar { idx, ch } => {
            let i = if chars.is_empty() {
                0
            } else {
                idx % (chars.len() + 1)
            };
            chars.insert(i, ch);
        }
        HexMutation::DeleteChar { idx } if !chars.is_empty() => {
            let i = idx % chars.len();
            chars.remove(i);
        }
        HexMutation::SwapChars { a, b } if !chars.is_empty() => {
            let ia = a % chars.len();
            let ib = b % chars.len();
            chars.swap(ia, ib);
        }
        HexMutation::Truncate { len } => {
            chars.truncate(len);
        }
        _ => {}
    }
    *s = chars.into_iter().collect();
}

fn proptest_config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig {
            cases: 4,
            failure_persistence: None,
            ..ProptestConfig::default()
        }
    } else {
        ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(proptest_config())]

    #[test]
    fn siphash24_128_matches_reference(
        k0 in any::<u64>(),
        k1 in any::<u64>(),
        data in prop::collection::vec(any::<u8>(), 0..=300),
    ) {
        let actual = siphash24_128(&data, k0, k1);
        let expected = ref_siphash24_128(&data, k0, k1);
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn const_sip_hasher_multi_chunk_from_parts_matches_flat_oneshot(
        canonical_name in any::<String>(),
        size in any::<usize>(),
        align in any::<usize>(),
        field_schema_hash in any::<u64>(),
    ) {
        let streamed = ComponentUuid::from_parts(&canonical_name, size, align, field_schema_hash);
        prop_assert_eq!(
            streamed,
            ComponentUuid::from_namespaced(&canonical_name, size, align, field_schema_hash)
        );
        prop_assert_eq!(
            ComponentUuid::from_canonical_name(&canonical_name),
            ComponentUuid::from_parts(&canonical_name, 0, 0, 0)
        );

        let mut flat = Vec::with_capacity(canonical_name.len() + 1 + 24);
        flat.extend_from_slice(canonical_name.as_bytes());
        flat.push(0xFF);
        flat.extend_from_slice(&(size as u64).to_le_bytes());
        flat.extend_from_slice(&(align as u64).to_le_bytes());
        flat.extend_from_slice(&field_schema_hash.to_le_bytes());

        let oneshot = siphash24_128(&flat, SIPHASH_KEY_0, SIPHASH_KEY_1);
        let ref_digest = ref_siphash24_128(&flat, SIPHASH_KEY_0, SIPHASH_KEY_1);
        prop_assert_eq!(streamed.bytes, oneshot);
        prop_assert_eq!(oneshot, ref_digest);
    }

    #[test]
    fn const_sip_hasher_multi_chunk_field_schema_matches_flat_oneshot(
        init_hash in any::<u64>(),
        fields in prop::collection::vec((any::<String>(), any::<String>(), any::<usize>()), 1..=8),
    ) {
        let mut prev = init_hash;
        for (field_name, field_type_tag, field_offset) in fields {
            let next = ComponentUuid::hash_field_schema(prev, &field_name, &field_type_tag, field_offset);
            let mut flat = Vec::with_capacity(18 + field_name.len() + field_type_tag.len());
            flat.extend_from_slice(&prev.to_le_bytes());
            flat.extend_from_slice(field_name.as_bytes());
            flat.push(0xFE);
            flat.extend_from_slice(field_type_tag.as_bytes());
            flat.push(0xFD);
            flat.extend_from_slice(&(field_offset as u64).to_le_bytes());

            let oneshot = siphash24_128(&flat, SIPHASH_KEY_0, SIPHASH_KEY_1);
            let ref_digest = ref_siphash24_128(&flat, SIPHASH_KEY_0, SIPHASH_KEY_1);
            prop_assert_eq!(oneshot, ref_digest);
            let expected_u64 = u64::from_le_bytes(oneshot[..8].try_into().expect("8 bytes"));
            prop_assert_eq!(next, expected_u64);
            prev = next;
        }
    }

    #[test]
    fn try_parse_hex_valid_roundtrip(
        bytes in any::<[u8; 16]>(),
        hyphenated in any::<bool>(),
        upper_nibbles in any::<[bool; 32]>(),
    ) {
        let formatted = format_uuid_hex(bytes, hyphenated, upper_nibbles);
        let parsed = ComponentUuid::try_parse_hex(&formatted);
        let expected = ComponentUuid::from_bytes(bytes);
        prop_assert_eq!(parsed, Some(expected));
        prop_assert_eq!(ComponentUuid::parse_hex(&formatted), expected);
        prop_assert_eq!(ComponentUuid::from_u128(expected.to_u128()), expected);
    }

    #[test]
    fn try_parse_hex_arbitrary_utf8_matches_reference(
        s in prop_oneof![
            any::<String>(),
            "[0-9a-fA-F\\-gGzZé€🦀]{28,40}",
        ],
    ) {
        let actual = ComponentUuid::try_parse_hex(&s);
        let expected = ref_parse_uuid_hex(&s);
        prop_assert_eq!(actual, expected);
        if let Some(uuid) = actual {
            prop_assert!(s.is_ascii());
            prop_assert!(s.len() == 32 || s.len() == 36);
            let canonical_lower = format_uuid_hex(uuid.bytes, s.len() == 36, [false; 32]);
            prop_assert_eq!(s.to_ascii_lowercase(), canonical_lower);
        }
    }

    #[test]
    fn try_parse_hex_mutated_valid_strings(
        bytes in any::<[u8; 16]>(),
        hyphenated in any::<bool>(),
        upper_nibbles in any::<[bool; 32]>(),
        mutations in prop::collection::vec(mutation_strategy(), 1..=6),
    ) {
        let mut s = format_uuid_hex(bytes, hyphenated, upper_nibbles);
        for m in mutations {
            apply_mutation(&mut s, m);
        }
        prop_assert_eq!(ComponentUuid::try_parse_hex(&s), ref_parse_uuid_hex(&s));
    }
}
