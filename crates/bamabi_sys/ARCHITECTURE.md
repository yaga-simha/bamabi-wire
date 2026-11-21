# `bamabi_sys` Architecture & Implementation Guide

`bamabi_sys` (`v0.3.0`, ABI `0x0003_0000`) is the **`#![no_std]`, zero-dependency C-ABI wire contract** at the root of Layer 0 (`bamabi-wire`) in the `bamabi` ecosystem. Every cross-boundary struct, handle, status code, VTable header, byte-offset constant, and POD trait shared between Bevy hosts, proc-macros, polyglot SDK generators, BQL/JIT compilers, and Rust/C99/C++17/Zig guest mods lives here.

---

## 1. Crate Role in Layer 0 & Dependency Policy

`bamabi_sys` enforces two non-negotiable boundary constraints in [`Cargo.toml`](Cargo.toml) and [`src/lib.rs`](src/lib.rs):

1. **Strictly `#![no_std]` with Zero External Dependencies:** `[dependencies]` in `Cargo.toml` is empty—no `std`, no `alloc`, no `bytemuck`, and **never** any `bevy_*` crate. (`proptest` appears only under `[dev-dependencies]` for integration property tests.)
2. **Scoped `unsafe` Policy:** Crate-level lints enforce `unsafe_op_in_unsafe_fn = "deny"`, `improper_ctypes_definitions = "deny"`, and `#![deny(clippy::undocumented_unsafe_blocks, clippy::missing_safety_doc)]`. Every `unsafe` block carries a `// SAFETY:` justification and every `unsafe fn` / `unsafe trait` documents a `# Safety` contract.

### Downstream Consumers

| Consumer | Layer | How It Uses `bamabi_sys` |
|---|---|---|
| `bamabi_macros` | Layer 0 | Emits `ModComponent`, `AbiPod`, `RawInsertable`, `VTableHeader`, `BamFieldDesc`, and `BamRequiredComponent` implementations |
| `bamabi_sdk_gen` | Layer 0 | Reads `offsets`, struct contracts, and constants to generate `include/bamabi.h`, `bamabi.hpp`, and `bamabi.zig` |
| `bamabi_guest_sys` | Layer 0 | Wraps all 68 `BamHostApi` slots with `covers_slot` checks, `extern "C"` panic guards, and validated slice/descriptor helpers |
| `bamabi_guest` | Layer 0 | Exposes safe `#![forbid(unsafe_code)]` Rust guest abstractions over `bamabi_sys` primitives and descriptors |
| `bamabi-tools` (`bamabi_manifest`, `cargo-bam`) | Layer 1 | Reuses `ABI_VERSION` and wire primitives without pulling in Bevy |
| `bamabi-core` (`bamabi_host_sys`, `bamabi_host`) | Layer 2 | Populates the 68-slot `BamHostApi` table, validates `BamManifest` / descriptors via `read_descriptor`, and resolves `ComponentUuid` |
| `bql_jit` / `bevy_bql` | External compiler | Uses `bamabi_sys::offsets::{OFF64_*, OFF32_*}` and `ComponentUuid` SipHash-2-4-128 for AOT/JIT lowering |

---

## 2. Module-by-Module Map

Only [`src/host_api.rs`](src/host_api.rs) is `pub mod host_api`; all other modules are private implementation modules whose public API is re-exported flat at the crate root in [`src/lib.rs`](src/lib.rs).

| Module | Path | Contents |
|---|---|---|
| **Crate root** | [`src/lib.rs`](src/lib.rs) | `#![no_std]`, `#![warn(missing_docs)]`, `#![deny(...)]` lints, incremental crate docs, flat `pub use` re-exports |
| **Versioning** | [`src/version.rs`](src/version.rs) | `ABI_VERSION` (`0x0003_0000`), `ABI_VERSION_{MAJOR,MINOR,PATCH}`, `pack_abi_version`, `unpack_abi_version`, `abi_version_{major,minor,patch}`, `abi_version_is_compatible` |
| **Primitives** | [`src/primitives/mod.rs`](src/primitives/mod.rs) | `FfiResult` (`18` status codes: `0..=16` and `99`) and re-exports of primitive submodules |
| ↳ *Bool* | [`src/primitives/bool.rs`](src/primitives/bool.rs) | `FfiBool(pub u8)` (`FALSE = 0`, `TRUE = 1`; any non-zero byte is `true`) |
| ↳ *Option* | [`src/primitives/option.rs`](src/primitives/option.rs) | `FfiOption<T> { is_some: FfiBool, value: T }` (`none()` via zeroed `MaybeUninit<T>` for `T: AbiPod`) |
| ↳ *Slices & Strings* | [`src/primitives/slice.rs`](src/primitives/slice.rs) | `FfiSlice<T> { ptr, len }`, `StrView = FfiSlice<u8>`, `validate`, `as_slice`, `try_as_slice`, `as_str`, `try_as_str` |
| ↳ *Handles & Entities* | [`src/primitives/entity.rs`](src/primitives/entity.rs) | `BamComponent(u32)` (1-based, `INVALID = 0`), `BamSystem(u32)` / `BamSystemSet(u32)` / `BamSymbol(u32)` (`INVALID = u32::MAX`), `FfiEntity(u64)` (`NULL = 0`), `FfiChildOf`, `FfiDisabled`, `BAM_{CHILD_OF,CHILDREN,DISABLED}_NAME` |
| ↳ *Assets* | [`src/primitives/asset.rs`](src/primitives/asset.rs) | `FfiAssetHandle` (`KIND_NONE`, `KIND_INDEX`, `KIND_UUID`), `FfiAssetLoadState` (`NOT_LOADED..=FAILED`) |
| ↳ *UUIDs & Hashing* | [`src/primitives/uuid.rs`](src/primitives/uuid.rs) | `ConstSipHasher128`, `siphash24_128`, `SIPHASH_KEY_{0,1}`, `ComponentUuid`, `FNV1A_64_{OFFSET_BASIS,PRIME}`, `fnv1a_64_*` |
| **Extensibility** | [`src/extensible.rs`](src/extensible.rs) | `ExtensibleStruct` trait, `read_extensible_struct`, `read_descriptor` |
| **Access & Schedules** | [`src/access.rs`](src/access.rs) | `MAX_TERMS_PER_QUERY = 32`, `MAX_QUERIES_PER_SYSTEM = 8`, `AccessMode(u32)` (`0..=14`), `ScheduleId(u32)` (`0..=5`, `CUSTOM_BASE = 1024`), `BamTermDesc` |
| **Components & Relations** | [`src/component.rs`](src/component.rs) | `ComponentDesc`, `BAM_COMPONENT_*`, `VTableHeader`, `MappedWireVTable<W>`, `MappedStringVTable`, `BamFieldKind`, `BamFieldDesc`, `BamRequiredComponent`, `BAM_REQUIRED_*`, `BamRelationDesc`, `BAM_RELATION_*`, `BamLayoutInfo`, `BamRowMigrateFn`, `BamScheduleSetDesc`, `BAM_SCHEDULE_SET_*` |
| **Systems & Observers** | [`src/system.rs`](src/system.rs) | `BamHostCtx`, `BamLoadCtx`, `BamSystemCtx`, `BamSystemFn`, `BamRunIfFn`, `BamQueryDesc`, `BamSystemDesc`, `BAM_SYSTEM_*`, `BamEventKind`, `BamObserverFn`, `BamObserverMutFn`, `BamObserverDesc`, `BAM_OBSERVER_*` |
| **Commands & Triggers** | [`src/commands.rs`](src/commands.rs) | `BamCommands`, `BamTriggerDesc`, `BAM_TRIGGER_*`, and `BamCmd*Fn` function typedefs |
| **Iteration & Ticks** | [`src/iter.rs`](src/iter.rs) | `BamIter`, `BAM_STORAGE_FLAG_*`, `BAM_MAX_CHANGE_AGE`, `BamChangeTicks`, `BamColumnInfo`, and `BamIter*Fn` / `BamRes*Fn` / `BamMsg*Fn` / `BamParForEachChunkFn` typedefs |
| **Host API & Offsets** | [`src/host_api.rs`](src/host_api.rs) | `BamInstance`, `BamManifest`, `BamHostApi` (68 slots), `BAM_HOST_API_SLOT_COUNT = 68`, `pub mod offsets`, and `offset_tests` |
| **Traits** | [`src/traits.rs`](src/traits.rs) | `RawInsertable`, `AbiPod`, `assert_abi_pod`, `assert_raw_insertable`, `SymbolEntry`, `ModComponent` |
| **Unit Tests** | [`src/tests/mod.rs`](src/tests/mod.rs), [`src/tests/layout.rs`](src/tests/layout.rs) | `#[cfg(test)]` layout, validation, versioning, SipHash, slice, and ABI 0.2/0.3 unit tests |
| **Property Tests** | [`tests/proptest_wire.rs`](tests/proptest_wire.rs) | `proptest` suites comparing `siphash24_128`, streaming `ComponentUuid`, and `try_parse_hex` against independent references |
| **Kani Proofs** | [`src/kani_proofs.rs`](src/kani_proofs.rs) | `#[cfg(kani)]` formal proof harnesses for `PO-W1`..`PO-W4` and `PO-W6`..`PO-W8` |

---

## 3. Memory Layouts & Zero-Padding Rules

Every cross-boundary struct in `bamabi_sys` is either `#[repr(transparent)]` over a single integer primitive or `#[repr(C)]` with **zero implicit compiler padding**:

1. **No Rust `enum` or `bool` Across FFI:**
   - Status and mode types (`FfiResult`, `AccessMode`, `ScheduleId`, `BamEventKind`, `BamFieldKind`, `FfiAssetLoadState`) are `#[repr(transparent)]` `u32` newtypes. Arbitrary foreign integers cannot cause invalid-discriminant UB.
   - `FfiBool` is `#[repr(transparent)]` `u8` (`0` = false, non-zero = true), preventing Rust `bool` UB when C/C++/Zig passes `2..=255`.
2. **Opaque Zero-Sized Handles:**
   - `BamInstance`, `BamHostCtx`, `BamLoadCtx`, `BamSystemCtx`, `BamCommands`, and `BamIter` are `#[repr(C)]` structs containing `_private: [u8; 0]` and `_marker: PhantomData<(*mut u8, PhantomPinned)>`. They are `!Send`, `!Sync`, `!Unpin`, and zero-sized, so foreign pointers are passed strictly as `*const` / `*mut` opaque pointers without constructing Rust references to host/guest internals.
3. **Explicit `reserved*` Padding Fields:**
   - Whenever natural `#[repr(C)]` field alignment would introduce an alignment gap, explicit `reserved0`..`reserved2` fields fill the exact gap and `validate()` rejects non-zero reserved words with `FfiResult::INVALID_ARGUMENT`.
   - For example, `BamTermDesc` (`12` B, `align 4`) packs `component: u32` (`0..4`), `mode: u32` (`4..8`), `flags: u16` (`8..10`), `or_group: u8` (`10`), and `reserved0: u8` (`11`).
   - `FfiAssetHandle` (`32` B, `align 4`) packs `kind: u8` (`0`), `reserved0: [u8; 3]` (`1..4`), `asset_type: u32` (`4..8`), `index: u32` (`8..12`), `generation: u32` (`12..16`), and `uuid: ComponentUuid` (`16..32`, `align 1`).
4. **32-bit `u64` Alignment Awareness (`i686` vs `wasm32`):**
   - On `i686-unknown-linux-gnu` (SysV i386 ABI), `align_of::<u64>()` is `4`, whereas on `wasm32-unknown-unknown` and 64-bit targets, `align_of::<u64>()` is `8`.
   - Structs containing `u64` (`ComponentDesc::schema_fingerprint` at offset `80` on 32-bit / `136` on 64-bit; `BamLayoutInfo::schema_fingerprint` at offset `24` on both 32-bit and 64-bit) position `u64` at 8-byte-aligned offsets so **zero** implicit padding is inserted on either `i686` or `wasm32`, and compile-time `#[cfg(target_pointer_width = "32")]` assertions verify `align_of::<Self>() == core::mem::align_of::<u64>()`.
   - `ComponentUuid` stores `[u8; 16]` (`size = 16`, `align = 1`) rather than `u128` or `[u64; 2]` for the same reason: deterministic 1-byte alignment across all C99/C++/Zig compilers and 32/64-bit ABIs.
5. **Compile-Time `const` Assertions:**
   - Every wire module ends with a `const _: () = { ... };` block asserting exact `size_of`, `align_of`, `offset_of!`, and `MIN_V0_2_SIZE` / `MIN_V0_3_SIZE` invariants under both `target_pointer_width = "64"` and `target_pointer_width = "32"`.

### Complete Wire Struct Layout Table

| Type | 64-bit Size / Align | 32-bit Size / Align | `MIN_V0_2_SIZE` (64 / 32) | `MIN_V0_3_SIZE` (64 / 32) |
|---|---|---|---|---|
| `FfiBool` | 1 B / 1 B | 1 B / 1 B | — | — |
| `BamComponent`, `BamSystem`, `BamSystemSet`, `BamSymbol`, `FfiResult`, `AccessMode`, `ScheduleId`, `BamEventKind`, `BamFieldKind`, `FfiAssetLoadState` | 4 B / 4 B | 4 B / 4 B | — | — |
| `FfiEntity`, `FfiChildOf` | 8 B / 8 B | 8 B / `align_of::<u64>()` | — | — |
| `FfiDisabled` | 0 B / 1 B | 0 B / 1 B | — | — |
| `ComponentUuid` | 16 B / 1 B | 16 B / 1 B | — | — |
| `BamChangeTicks` | 16 B / 4 B | 16 B / 4 B | — | — |
| `FfiAssetHandle` | 32 B / 4 B | 32 B / 4 B | — | — |
| `FfiSlice<T>`, `StrView` | 16 B / 8 B | 8 B / 4 B | — | — |
| `BamTermDesc` | 12 B / 4 B | 12 B / 4 B | — | — |
| `BamColumnInfo` | 48 B / 8 B | 40 B / 4 B | — | — |
| `BamQueryDesc` | 24 B / 8 B | 16 B / 4 B | 24 / 16 | — |
| `VTableHeader` | 40 B / 8 B | 32 B / 4 B | 40 / 32 | — |
| `BamManifest` | 48 B / 8 B | 32 B / 4 B | 32 / 24 | — |
| `BamTriggerDesc` | 64 B / 8 B | 48 B / 4 B | 64 / 48 | — |
| `BamObserverDesc` | 88 B / 8 B | 64 B / 4 B | 48 / 36 | — |
| `BamSystemDesc` | 168 B / 8 B | 92 B / 4 B | 104 / 60 | — |
| `ComponentDesc` | 176 B / 8 B | 104 B / `align_of::<u64>()` | 64 / 44 | 176 / 104 |
| `BamFieldDesc` | 64 B / 8 B | 36 B / 4 B | `MIN_V0_3_SIZE` | 64 / 36 |
| `BamRequiredComponent` | 64 B / 8 B | 48 B / 4 B | `MIN_V0_3_SIZE` | 64 / 48 |
| `BamRelationDesc` | 56 B / 8 B | 44 B / 4 B | `MIN_V0_3_SIZE` | 56 / 44 |
| `BamLayoutInfo` | 64 B / 8 B | 48 B / `align_of::<u64>()` | `MIN_V0_3_SIZE` | 64 / 48 |
| `BamScheduleSetDesc` | 48 B / 8 B | 36 B / 4 B | `MIN_V0_3_SIZE` | 48 / 36 |
| `BamHostApi` | 552 B / 8 B | 280 B / 4 B | 224 / 116 | 552 / 280 |

---

## 4. `BamHostApi` 68-Slot Freeze & `offsets` Module

`BamHostApi` in [`src/host_api.rs`](src/host_api.rs) is the single versioned function table passed from host to guest in `bamabi_init(api, load_ctx, instance_out)`.

### Layout & Frozen Boundaries

- **Header (`8` bytes):** `struct_size: u32` at offset `0`, `abi_version: u32` at offset `4`.
- **Slots `0..27` (Mandatory Core `0.2`):** `get_proc_address` through `cmd_remove`. Terminates at `BamHostApi::MIN_V0_2_SIZE`:
  - `224` bytes on 64-bit (`8 + 27 * 8`).
  - `116` bytes on 32-bit (`8 + 27 * 4`).
- **Slots `27..50` (Extended `0.2.x`):** `par_for_each_chunk` through `cmd_spawn_scene` (23 slots at offsets `224..408` on 64-bit / `116..208` on 32-bit).
- **Slots `50..68` (ABI `0.3.0`):** `log_diagnostic` through `migrate_component_column` (18 slots at offsets `408..552` on 64-bit / `208..280` on 32-bit). Terminates at `BamHostApi::MIN_V0_3_SIZE == size_of::<BamHostApi>()`:
  - `552` bytes on 64-bit (`8 + 68 * 8`).
  - `280` bytes on 32-bit (`8 + 68 * 4`).

### Safe Header Probing & `covers_slot`

Because an older host could pass a `BamHostApi` allocation smaller than `size_of::<BamHostApi>()` (down to `MIN_V0_2_SIZE`), guests **must not** form `&BamHostApi` directly from a raw pointer until `struct_size` has been checked:

1. Call `BamHostApi::raw_struct_size(api)` and `BamHostApi::raw_abi_version(api)` (which read via `core::ptr::addr_of!` without constructing `&BamHostApi`).
2. Copy into a local `BamHostApi` with `read_descriptor::<BamHostApi>(api.cast())`, which zero-fills any trailing slots beyond `struct_size`.
3. Validate mandatory core slots and SemVer compatibility via `validate_for_guest(ABI_VERSION)` (checks `struct_size >= MIN_V0_2_SIZE`, `abi_version_is_compatible`, and `has_all_v0_2_slots()`).
4. Before invoking any extended `0.2.x` or `0.3.0` slot, verify `api.covers_slot(offset, size_of::<Option<Fn>>())` **and** unwrap the `Option<unsafe extern "C" fn(...)>`.

### The `offsets` Module

`bamabi_sys::offsets` (defined by `define_host_api_offsets!` in [`src/host_api.rs`](src/host_api.rs)) exports three constant families for all 70 fields (`struct_size`, `abi_version`, and slots `0..68`), plus `SLOT_COUNT = 68`, `SIZE_64 = 552`, and `SIZE_32 = 280`:

- `<FIELD>`: Target-native byte offset (`core::mem::offset_of!(BamHostApi, <field>)`).
- `OFF64_<FIELD>`: Architecture-independent 64-bit offset (`8 + 8 * slot_idx`).
- `OFF32_<FIELD>`: Architecture-independent 32-bit offset (`8 + 4 * slot_idx`).

Compile-time assertions inside `define_host_api_offsets!` guarantee `OFF64_k == 8 + 8 * k`, `OFF32_k == 8 + 4 * k`, and exact parity with the active target's `core::mem::offset_of!`.

---

## 5. `const` SipHash-2-4-128 `ComponentUuid` & FNV-1a Fingerprinting

[`src/primitives/uuid.rs`](src/primitives/uuid.rs) provides two deterministic, `const`-evaluatable hash primitives with zero external dependencies:

### `ComponentUuid` via Keyed SipHash-2-4-128

`ConstSipHasher128` implements Aumasson & Bernstein's SipHash-2-4 in `DOUBLE` (128-bit output) mode using fixed domain-separation keys:

- `SIPHASH_KEY_0 = 0x4241_4d5f_5555_4944` (`"BAM_UUID"`)
- `SIPHASH_KEY_1 = 0x5630_5f32_5349_5048` (`"V0_2SIPH"`)

Key constructors:

- `ComponentUuid::from_parts(canonical_name, size, align, field_schema_hash)` (alias `from_namespaced`): hashes `canonical_name.as_bytes() || [0xFF] || (size as u64).to_le_bytes() || (align as u64).to_le_bytes() || field_schema_hash.to_le_bytes()`.
- `ComponentUuid::from_canonical_name(canonical_name)`: calls `from_parts(canonical_name, 0, 0, 0)` for opaque/VTable-only `#[repr(Rust)]` components whose layout is intentionally hidden across compilers.
- `ComponentUuid::hash_field_schema(prev_hash, field_name, field_type_tag, field_offset)`: folds `(prev_le || field_name || 0xFE || field_type_tag || 0xFD || offset_le)` into a 64-bit schema hash for `#[repr(C)]` `AbiPod` structs.
- `ComponentUuid::try_parse_hex` / `parse_hex`: `const` parser accepting canonical 36-char `"8-4-4-4-12"` or compact 32-char upper/lower hex UUIDs.

### 64-bit FNV-1a Schema Fingerprinting

`FNV1A_64_OFFSET_BASIS` (`0xcbf2_9ce4_8422_2325`), `FNV1A_64_PRIME` (`0x0000_0100_0000_01b3`), `fnv1a_64_init`, `fnv1a_64_update_bytes`, `fnv1a_64_update_u64`, and `fnv1a_64_str` provide deterministic `const` 64-bit FNV-1a hashing used for `ModComponent::SCHEMA_FINGERPRINT` and `ComponentDesc::schema_fingerprint` (enabling zero-copy hot-reload retention when fingerprints match and `migrate_component_column` when they differ).

---

## 6. Prefix Extensibility (`ExtensibleStruct` + `VTableHeader`)

[`src/extensible.rs`](src/extensible.rs) enables forward- and backward-compatible struct extension across minor/additive ABI updates without reallocations or versioned struct proliferation:

### How `ExtensibleStruct` Works

A type implementing `unsafe trait ExtensibleStruct: Copy + 'static` guarantees:
1. It is `#[repr(C)]` with `struct_size: u32` at byte offset `0`.
2. `size_of::<u32>() <= Self::MIN_V0_2_SIZE <= size_of::<Self>()`.
3. **Every field is valid when all-bits-zero** (integers, `#[repr(transparent)]` integer newtypes, `[u8; 16]`, raw pointers, and `Option<unsafe extern "C" fn(...)>`—never `bool`, bare `fn`, `NonNull`, or `&T`).

`read_extensible_struct::<T>(ptr, struct_size)` (and `read_descriptor::<T>(ptr)`, which reads `struct_size` unaligned from offset `0`):
- Returns `Err(FfiResult::NULL_POINTER)` if `ptr` is null.
- Returns `Err(FfiResult::INVALID_VERSION)` if `struct_size < T::MIN_V0_2_SIZE`.
- Zero-initializes a stack `MaybeUninit<T>` and copies `min(struct_size, size_of::<T>())` bytes from `ptr`:
  - **Older sender (`struct_size < size_of::<T>()`):** newer trailing fields are zero-filled (`None` / `0` / `null`).
  - **Newer sender (`struct_size > size_of::<T>()`):** unknown trailing bytes are ignored without out-of-bounds reads.

All **13** `ExtensibleStruct` implementors in `bamabi_sys`:
`BamManifest`, `BamHostApi`, `BamSystemDesc`, `BamObserverDesc`, `BamTriggerDesc`, `BamQueryDesc`, `ComponentDesc`, `VTableHeader`, `BamFieldDesc`, `BamRequiredComponent`, `BamRelationDesc`, `BamLayoutInfo`, and `BamScheduleSetDesc`.

### `VTableHeader`

`VTableHeader` (`struct_size: u32`, `abi_version: u32`, `uuid: ComponentUuid`, `size: usize`, `align: usize`; `40` B on 64-bit / `32` B on 32-bit) sits at byte offset `0` of every `<Struct>VTable` generated by `#[derive(ModComponent)]`, as well as `MappedWireVTable<W>` and `MappedStringVTable`. `VTableHeader::validate(expected_min_vtable_size, expected_uuid)` enforces `struct_size >= max(expected_min_vtable_size, MIN_V0_2_SIZE)`, ABI SemVer compatibility, and `uuid == expected_uuid`.

---

## 7. Unsafe Trait Contracts (`ModComponent`, `AbiPod`, `RawInsertable`)

[`src/traits.rs`](src/traits.rs) separates three levels of type capability:

1. **`unsafe trait RawInsertable: Sized + Send + Sync + 'static`**
   - *Contract:* No heap allocations, `!core::mem::needs_drop::<Self>()`, and no invalid bit-pattern hazards (`bool`, `char`, `enum`, `NonZero*`, references, and bare `fn` pointers are forbidden).
   - *Scope:* Same-process raw byte moves and unaligned reads. Includes all `AbiPod` types **plus** `u128`, `i128`, `usize`, and `isize`.
2. **`unsafe trait AbiPod: RawInsertable + Copy + Send + Sync + 'static`**
   - *Contract:* Deterministic non-packed `#[repr(C)]` or `#[repr(transparent)]` cross-language Plain Old Data with zero implicit padding and universal bit validity across all fields.
   - *Implemented for:* `u8`, `i8`, `u16`, `i16`, `u32`, `i32`, `u64`, `i64`, `f32`, `f64`, `FfiBool`, `BamComponent`, `BamSystem`, `BamSystemSet`, `BamSymbol`, `FfiEntity`, `FfiChildOf`, `FfiDisabled`, `BamChangeTicks`, `ComponentUuid`, `FfiAssetHandle`, `FfiAssetLoadState`, `BamEventKind`, `BamFieldKind`, `[T; N] where T: AbiPod`, and `FfiOption<T> where T: AbiPod`.
   - *Explicitly excluded:* `u128`/`i128` (no ISO C99 equivalent; 8- vs 16-byte alignment divergence), `usize`/`isize` (pointer-width dependent), `bool`/`char` (invalid bit patterns).
3. **`pub trait ModComponent: Sized + 'static`** (safe trait)
   - *Contract:* Deliberately has **no** `bevy_ecs`, `RawInsertable`, or `Default` supertraits so `#[repr(Rust)]` components containing `String`, `Vec<T>`, `bool`, skipped fields, or opaque data can still expose a [`VTableHeader`]-prefixed VTable, `CANONICAL_NAME`, `COMPONENT_CSTR`, `UUID`, `SCHEMA_FINGERPRINT`, `BAM_FIELDS`, `REQUIRED_COMPONENTS`, `C_DECL`, `ZIG_DECL`, and `symbols()`.

---

## 8. Recipe: Adding a New Wire Struct or `BamHostApi` Slot Safely

Because ABI `0.3.0` (`0x0003_0000`) is **frozen**, do **not** reorder existing fields, change existing slot indices/offsets, or shrink `MIN_V0_2_SIZE` / `MIN_V0_3_SIZE`. Follow this checklist when extending the wire contract for a new minor/major revision:

### Adding a New `BamHostApi` Slot

1. **Decide ABI bump:** Under pre-1.0 rules (`ABI_VERSION_MAJOR == 0`), adding a slot changes `size_of::<BamHostApi>()` / `SLOT_COUNT` and requires bumping `ABI_VERSION_MINOR` in `src/version.rs` (and `abi-lock.toml` / downstream lockfiles). Under post-1.0 (`major >= 1`), append strictly at the end of `BamHostApi` while keeping `MIN_V0_2_SIZE` / `MIN_V0_3_SIZE` frozen.
2. **Define the `unsafe extern "C" fn` typedef:** Place the typedef in the appropriate domain module (`host_api.rs`, `commands.rs`, `iter.rs`, etc.) with `# Safety` documentation and re-export it from `src/lib.rs`.
3. **Append to `BamHostApi`:** Add `pub <slot_name>: Option<<SlotFn>>` at the end of `BamHostApi` in `src/host_api.rs` and update `BAM_HOST_API_SLOT_COUNT` + `has_all_*` helpers.
4. **Update `offsets` and `const` assertions:**
   - Add the `(idx, field, NATIVE, OFF64_*, 8 + 8*idx, OFF32_*, 8 + 4*idx)` entry to `define_host_api_offsets!` in `src/host_api.rs` and update `SIZE_64` / `SIZE_32`.
   - Update `const _: () = { ... }` offset/size assertions in `src/host_api.rs`.
5. **Update tests & Kani:** Add the new slot to `populated_host_api()`, `all_slots` in `offset_tests`, and `abi_v0_*` layout tests in `src/tests/layout.rs`.
6. **Synchronize downstream crates:** Regenerate `include/bamabi.h` / `bamabi.hpp` / `bamabi.zig` via `bamabi_sdk_gen` (`cargo xtask header` and `cargo xtask gen-component-headers`), add the guest wrapper in `bamabi_guest_sys`, and implement the host slot in `bamabi_host_sys`.

### Adding a New Wire Struct (or Appending to an `ExtensibleStruct`)

1. **Choose layout & padding:** Use `#[repr(C)]` with `pub struct_size: u32` at offset `0` (if extensible), order fields to eliminate implicit alignment gaps, and insert explicit `reserved0`..`reservedN` zero-checked fields wherever gaps would occur.
2. **Ensure zero-validity:** If implementing `unsafe impl ExtensibleStruct`, verify every field type is valid when zero-filled (use `Option<unsafe extern "C" fn(...)>`, `FfiBool`, integer newtypes, raw pointers, `[u8; 16]`).
3. **Implement `validate(&self) -> FfiResult`:** Reject `struct_size < MIN_*_SIZE` (`INVALID_VERSION`), unknown flag bits (`(flags & !KNOWN_FLAGS) != 0` -> `UNSUPPORTED`), non-zero `reserved*` (`INVALID_ARGUMENT`), and invalid pointers (`NULL_POINTER`).
4. **Add dual-target `const` assertions:** Assert `size_of`, `align_of`, and every `offset_of!` under both `#[cfg(target_pointer_width = "64")]` and `#[cfg(target_pointer_width = "32")]`.
5. **Add tests & Kani proof:** Add unit layout/validation tests in `src/tests/layout.rs` and, if `ExtensibleStruct`, add a `po_w3_extensible_harness!(...)` entry in `src/kani_proofs.rs`.

---

## 9. Setup, Building, Using, Testing & Performance Notes

### Setup & Building

- **Edition / MSRV:** Rust edition `2024`; crate `rust-version = "1.87"` (workspace `1.95` in root `Cargo.toml`).
- **32-bit & Wasm cross-targets (optional for local checks):**
  ```bash
  rustup target add i686-unknown-linux-gnu wasm32-unknown-unknown
  ```
- **Build & cross-target layout verification:**
  ```bash
  cargo check -p bamabi_sys
  cargo check -p bamabi_sys --target i686-unknown-linux-gnu
  cargo check -p bamabi_sys --target wasm32-unknown-unknown
  cargo clippy -p bamabi_sys --all-targets --all-features -- -D warnings
  RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_sys --no-deps --all-features
  ```

### Using `bamabi_sys` as a Dependency

Because `bamabi_sys` is `#![no_std]` with zero dependencies, both `std` and `#![no_std]` crates can depend on it directly:

```toml
[dependencies]
bamabi_sys = { git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main" }
```

### Testing & Verification

```bash
cargo fmt -p bamabi_sys -- --check
cargo test -p bamabi_sys --all-features
cargo test --doc -p bamabi_sys --all-features

# Dual-model Miri (Tree Borrows + Stacked Borrows)
MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_sys
MIRIFLAGS="-Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_sys

# Kani formal verification (PO-W1..PO-W4, PO-W6..PO-W8)
cargo kani -p bamabi_sys
```

See [`TESTS.md`](TESTS.md) for a complete per-test and per-proof breakdown.

### Benchmarking & Performance Notes

- **No Criterion Benchmarks in Layer 0:** `bamabi_sys` contains **no** Criterion or runtime micro-benchmark harnesses (`[[bench]]`); end-to-end ECS frame-budget and hot-reload benchmarks live in the Layer 4 **`bamabi-showcase`** repository (`docs/BENCHMARKS.md`).
- **Zero-Cost Compile-Time Layout Verification:** All struct size, alignment, and field-offset checks execute in `const _: () = { ... };` blocks at compile time with zero runtime overhead.
- **Zero-Allocation Hot-Path Wire Design:** Pre-resolved `BamComponent` / `BamSystem` / `BamSystemSet` / `BamSymbol` `u32` handles, declaration-indexed query/resource accessors (`sys_query`, `res_ptr`, `iter_column_*`), and non-owning `FfiSlice<T>` / `StrView` views avoid runtime hashing, string parsing, and per-frame heap allocations across the ABI.
- **Property-Test Scaling (`proptest_wire.rs`):** Default `proptest` case counts (`256` cases per property) can be scaled via `PROPTEST_CASES=N cargo test -p bamabi_sys --test proptest_wire`; under `cfg(miri)`, `proptest_config()` automatically caps runs at `cases: 4` so Miri execution remains fast.
