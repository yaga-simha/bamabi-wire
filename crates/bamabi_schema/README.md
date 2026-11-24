# `bamabi_schema`

Zero-dependency, `#![no_std]`, `#![forbid(unsafe_code)]` single source of truth for:
- Canonical wire primitive & well-known type metadata (`size`, `align`, `BamFieldKind`, canonical tags)
- Target-aware `(ptr_bytes, u64_align)` `#[repr(C)]` layout solver & explicit zero-padding synthesis (`_pad*`)
- Authoritative `compute_component_uuid` (SipHash-2-4-128) and `compute_schema_fingerprint` (FNV-1a-64)
- Parameterized 32-bit and 64-bit `#[repr(C)]` wire struct layout and `BamHostApi` slot metadata (`abi_meta`)
