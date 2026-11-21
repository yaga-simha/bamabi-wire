# `bamabi-wire` Architecture

`bamabi-wire` is **Layer 0** of the federated **`bamabi`** (`ABI 0.3.0`, `v0.3.0`) native modding framework for Bevy ECS (`v0.20.1`). It defines the complete **zero-Bevy, zero-shared-Rust-ABI** wire contract, procedural macros, polyglot SDK header generator, and safe Rust guest SDK shared across all downstream repositories and non-Rust mod toolchains.

For byte-by-byte struct layouts, constant values, and protocol rules, see the canonical wire specification in [`spec.md`](spec.md).

## 1. Federated Position & Workspace Crate DAG

Across the 5-repository federated `bamabi` stack, `bamabi-wire` sits at the root with zero upstream dependencies:

| Layer | Repository | Role |
|---|---|---|
| **0** | **`bamabi-wire`** *(this repo)* | Zero-Bevy ABI `0.3.0` wire contract, derives, C99/C++17/Zig SDK generator, and Rust guest SDK |
| **1** | [`bamabi-tools`](https://github.com/yaga-simha/bamabi-tools) | Zero-Bevy Ed25519 trust (`bamabi_trust`), `mod.toml` SemVer DAG (`bamabi_manifest`), `cargo-bam` CLI |
| **2** | [`bamabi-core`](https://github.com/yaga-simha/bamabi-core) | Headless `bevy_ecs` + `bevy_app` host runtime (`bamabi_host_sys`, `bamabi_host`, `bamabi_watcher`) |
| **3** | [`bamabi`](https://github.com/yaga-simha/bamabi) | Canonical host facade (`bamabi`) and Bevy `0.20.1` companion crates (`bamabi_bridges`, `bamabi_render`, `bamabi_reflect`) |
| **4** | [`bamabi-showcase`](https://github.com/yaga-simha/bamabi-showcase) | Integration repo: examples (`01`–`10`), benchmarks (`docs/BENCHMARKS.md`), fuzzers, mdBook guide |
| **Overlay** | [`bamabi-dev-overlay`](https://github.com/yaga-simha/bamabi-dev-overlay) | Multi-repo `abi-lock.toml` consistency checker (`sync_check.py`) and local sibling `[patch]` manager (`overlay.py`) |

Inside `bamabi-wire`, the 5 functional library crates and `xtask` form a strictly acyclic DAG that separates `unsafe` FFI machinery (`bamabi_sys`, `bamabi_guest_sys`) from `#![forbid(unsafe_code)]` crates (alongside the non-published `bamabi_docs` documentation hub crate):

```text
                ┌──────────────────────────┐
                │       bamabi_guest       │  #![forbid(unsafe_code)]
                │  Safe Rust .cdylib SDK   │
                └─────┬───────┬────────┬───┘
                      │       │        │ (optional feature: `derive`)
                      ▼       │        ▼
  ┌───────────────────────┐   │   ┌────────────────────┐
  │   bamabi_guest_sys    │   │   │   bamabi_macros    │  #![forbid(unsafe_code)]
  │  Guest unsafe boundary│   │   │  proc-macro2/syn   │
  └───────────┬───────────┘   │   └─────────┬──────────┘
              │               │             │
              │               ▼             │
              │   ┌────────────────────┐    │     ┌────────────────────┐
              └──►│     bamabi_sys     │◄───┴─────│   bamabi_sdk_gen   │  #![forbid(unsafe_code)]
                  │ #![no_std], 0 deps │          │ C99 / C++17 / Zig  │
                  └────────────────────┘          └─────────▲──────────┘
                            ▲                               │
                            └─────────────┬─────────────────┘
                                    ┌─────┴──────┐
                                    │   xtask    │  #![forbid(unsafe_code)]
                                    │  CI/codegen│
                                    └────────────┘
```

### Crate Responsibilities & Policies

| Crate | `#![no_std]` | `#![forbid(unsafe_code)]` | External Deps | Role |
|---|---|---|---|---|
| `bamabi_docs` | Yes | **Yes** | **Zero** | Non-published (`publish = false`) workspace `rustdoc` landing hub (`target/doc/bamabi_docs/index.html`) embedding `README.md`, `ARCHITECTURE.md`, `DEVELOPMENT.md`, `TESTING.md`, `spec.md`, and the curated crate index |
| `bamabi_sys` | Yes | No (`unsafe` traits/slice views) | **Zero** | ABI `0.3.0` (`0x0003_0000`) `#[repr(C)]`/`#[repr(transparent)]` primitives, 68-slot `BamHostApi`, `offsets`, `ComponentUuid` (SipHash-2-4-128), `ExtensibleStruct`, descriptors, `ModComponent`/`AbiPod`/`RawInsertable` |
| `bamabi_macros` | Proc-macro | **Yes** | `syn`, `quote`, `proc-macro2`, `proc-macro-crate` | `#[derive(ModComponent, AbiPod, GuestComponent)]`, compile-time zero-padding checks, hygienic VTables, `SCHEMA_FINGERPRINT`, `BAM_FIELDS`, `REQUIRED_COMPONENTS`, `C_DECL`/`ZIG_DECL` |
| `bamabi_sdk_gen` | `std` | **Yes** | None (`bamabi_sys` only) | Renders `include/bamabi.h` (C99), `bamabi.hpp` (C++17), `bamabi.zig` (Zig), and C99 component headers from `bamabi_sys` metadata |
| `bamabi_guest_sys` | `std` | No (guest `unsafe` boundary) | None (`bamabi_sys` only) | `GuestInstance`, `extern "C"` panic guards (`ReentrantCell`), checked 68/68 `Host` wrappers, Bevy `0.20` `wire_types` POD mirrors, validated iterators, commands, migrations |
| `bamabi_guest` | `std` | **Yes** | None (`bamabi_sys`, `bamabi_guest_sys`, opt. `bamabi_macros`) | Ergonomic safe Rust `.cdylib` SDK (`GuestPlugin`, `GuestApp`, `BevySchedules`, `conditions::*`, `1..=16`-term `QueryTuple`, `Or<(T0..T7)>`, `GuestTransaction`, `ObserverBuilder`, `bridges`, `export_mod!`) |
| `xtask` | `std` | **Yes** | None (`bamabi_sys`, `bamabi_macros`, `bamabi_sdk_gen`) | Local & CI runner (`fmt`, `lint`, `test`, `c-test`, `header`, `gen-component-headers`, `audit`, `miri`, `kani`, `nightly`, `ci`) |

## 2. Core Invariants: Zero-Bevy & Zero-Shared-Rust-ABI

1. **Zero `bevy*` Dependencies:** Verified on every `cargo xtask lint` run by inspecting `cargo tree --workspace` (`verify_zero_bevy`) and enforced in [`deny.toml`](deny.toml). Layer 0 never couples guest mods or compiler backends (`bevy_bql`) to Bevy's internal Rust types or compile times.
2. **Zero Shared Rust ABI:** Rust has no stable cross-compiler ABI. Every cross-boundary struct is `#[repr(C)]` or `#[repr(transparent)]` with zero implicit padding, verified on 64-bit and 32-bit targets in both Rust (`offset_of!`) and C99 (`BAM_STATIC_ASSERT`).
3. **No Rust Fat Pointers or Niche-Unsafe Primitives Across `extern "C"`:**
   - Strings and slices pass as `StrView` (`FfiSlice<u8>`) / `FfiSlice<T> { ptr, len }` or NUL-terminated `*const c_char`. Empty C slices `(NULL, 0)` normalize via `NonNull::dangling()`.
   - Booleans use `#[repr(transparent)] pub struct FfiBool(pub u8)` (`0` = false, non-zero = true) rather than Rust `bool` or `<stdbool.h>`.
   - Enums and flags use `#[repr(transparent)]` `u32` newtypes (`FfiResult`, `AccessMode`, `ScheduleId`, `BamEventKind`, `BamFieldKind`, `FfiAssetLoadState`).
   - All foreign function pointers use `Option<unsafe extern "C" fn(...)>` so all-bits-zero is guaranteed `None`.
4. **Prefix Extensibility (`ExtensibleStruct`) & `BamHostApi` Slot Negotiation:**
   - Every extensible descriptor (`BamManifest`, `BamHostApi`, `BamQueryDesc`, `BamSystemDesc`, `BamObserverDesc`, `BamTriggerDesc`, `ComponentDesc`, `BamFieldDesc`, `BamRequiredComponent`, `BamRelationDesc`, `BamLayoutInfo`, `BamScheduleSetDesc`, `VTableHeader`) places `pub struct_size: u32` at byte offset `0` and defines frozen `MIN_V0_2_SIZE` / `MIN_V0_3_SIZE` constants via `offset_of!`.
   - `BamHostApi` freezes `MIN_V0_2_SIZE = 224` B (64-bit) / `116` B (32-bit) for the 27 core slots, `224..408` / `116..208` for the 23 extended `0.2.x` slots, and `MIN_V0_3_SIZE = 552` B (64-bit) / `280` B (32-bit) for all **68/68** live slots (`SLOT_COUNT = 68`).
   - Guests check `api.struct_size >= BamHostApi::MIN_V0_2_SIZE` and probe extended slots with `BamHostApi::covers_slot(offset, slot_size)` (mirroring `bamabi_sys::offsets::{OFF64_*, OFF32_*}`), returning `FfiResult::UNSUPPORTED` if an older host table omits an optional slot.

## 3. ABI `0.3.0` (`0x0003_0000`) Host-Guest Lifecycle

`abi-lock.toml` pins `bam_abi_version = "0x0003_0000"`. Under Cargo SemVer rules (`abi_version_is_compatible`), pre-`1.0` requires `host_major == 0 && guest_major == 0 && host_minor == guest_minor` (patch differences are compatible).

```text
  Host (exclusive &mut World)                   Guest (.cdylib)
  ───────────────────────────                   ───────────────
  1. Load library (shadow copy)
  2. Call bamabi_manifest() ───────────────────► returns static &'static BamManifest
     Validate struct_size, ABI version, flags
  3. Open LoadTransaction
  4. Call bamabi_init(api, load_ctx, &mut inst)► GuestInstance::init(...)
       - resolve/register components & relations   - validates BamHostApi & copies 68 slots
       - intern symbols, bind named objects        - registers components, systems, observers,
       - register systems, schedule sets, mounts     schedule sets, relations
       - if Err: rollback LoadTransaction          - if Err: drops partial GuestInstance, no shutdown
       - writes *mut BamInstance to instance_out ◄──
  5. Schedule Execution (ComputeTaskPool / Main Thread)
       - run_if(&user_data) / BamSystemFn(sys_ctx) ► borrow BamIter & BamCommands
       - direct dense/sparse columns & resources     enqueue deferred commands / transactions
       - observers & triggers                        emit telemetry / faults via FfiResult
  6. Optional Hot Reload (world.reload_mod(id))
       - if save_state: snapshot via BamStateWriterFn ──► serializes state into host Vec<u8>
       - unload old session -> load replacement -> restore_state(&blob)
       - unchanged schema_fingerprint AbiPod columns retain storage (0 B copied)
       - changed AbiPod schemas migrate via migrate_component_column (BamRowMigrateFn)
  7. Unload / World::drop
       - disarm schedules, DROP_PAGES, hooks, observers
       - Call bamabi_shutdown(instance) ───────────► frees GuestInstance & closures in LIFO order
       - dlclose / FreeLibrary
```

### Opaque Handle Lifetimes

Every opaque handle issued by the host is strictly scoped and must never be cached past its window:

- `BamLoadCtx*`: valid only during `bamabi_init`.
- `const BamHostCtx*`: valid during `bamabi_init` and inside any `BamSystemFn` invocation on the owning `World`.
- `BamSystemCtx*`: valid only for a single `BamSystemFn` invocation.
- `BamIter*`: valid only for the enclosing `BamSystemFn`; column/entity pointers remain valid only until the next `iter_next_chunk` or `iter_reset`, or system return.
- `BamCommands*`: valid only for the enclosing `BamSystemFn`.
- `BamInstance*`: owned by the guest heap from successful return of `bamabi_init` until passed once to `bamabi_shutdown(instance)`.

## 4. Memory Isolation & Panic Boundary

1. **Owner-Frees Allocator Isolation:** Dynamic libraries link independent allocators (`glibc`, `mimalloc`, `jemalloc`, Windows `HeapAlloc`). Host allocations (`World` archetype columns, `String`/`Vec` component payloads, `CommandArena` slabs, `RegistrySnapshot`) are freed exclusively by the host; guest allocations (`GuestInstance`, boxed system/`run_if`/teardown closures) are allocated in `bamabi_init` and freed exclusively by the guest in `bamabi_shutdown`.
2. **Padding-Safe Copies & Self-Aliasing Protection:** Raw `AbiPod` component payloads cross FFI without forming `&[u8]` over uninitialized padding bytes. Generated `String`/`Vec<T>` VTable setters detect overlapping incoming slices and buffer before mutating.
3. **Strict `extern "C"` (`nounwind`) Panic Boundary:**
   - Every callback slot in `bamabi_sys` and generated VTables is `unsafe extern "C" fn(...)` (LLVM `nounwind`), never `extern "C-unwind"`. The host never wraps foreign calls in `catch_unwind`.
   - Under Rust's default `panic = "unwind"`, `bamabi_guest_sys` wraps every guest entry point (`catch_guest_result`, `catch_guest_bool`, `catch_guest_unit`) and guards closure state via `ReentrantCell` (`UnsafeCell<F>` + `AtomicBool`), dropping panic payloads on the guest allocator and returning `FfiResult::PANIC_CAUGHT` (`3`) (or `ACCESS_CONFLICT` on re-entry).
   - Under `panic = "abort"`, panics terminate the process deterministically at the panic site with zero UB.

## 5. Polyglot C99 / C++17 / Zig SDK Pipeline

`bamabi_sys` is the single source of truth for wire types, `ComponentUuid` SipHash-2-4-128 constants, and `BamHostApi` offsets (`bamabi_sys::offsets`):

1. **`bamabi_sdk_gen::header_gen`** reads `bamabi_sys::offsets` and ABI constants to render ISO C99 [`include/bamabi.h`](include/bamabi.h) (`BAM_EXPORT`, `BAM_RESTRICT`, all 68 `BAM_HOST_API_OFFSET_*`, `bam_uuid_from_parts`, and 64-bit/32-bit `BAM_STATIC_ASSERT` checks). Parity is enforced by `cargo xtask header --check` and `crates/bamabi_sdk_gen/tests/header_parity.rs`.
2. **`bamabi_sdk_gen::sdk`** embeds and exports the polyglot bundle (`export_sdk`): `bamabi.h` (C99), `bamabi.hpp` (C++17 68/68 slot view wrappers + RAII `ScopedTransaction`), and `bamabi.zig` (Zig `b.addTranslateC` `"bam_c"` 68/68 slot views + lossless 18-code `BamError`).
3. **`bamabi_macros` + `bamabi_sdk_gen::component_header` + `xtask/src/component_headers.rs`**: `#[derive(ModComponent)]` emits compile-time `ModComponent::C_DECL` and `ZIG_DECL` constants; `cargo xtask gen-component-headers` collects `section_for::<T>()` into [`include/generated/testbed_components.h`](include/generated/testbed_components.h).

## 6. How Everything Ties Together: End-to-End Walkthrough

To see how all 6 workspace crates interlock without a single `bevy_*` dependency, trace one concrete feature end-to-end: defining a `#[derive(ModComponent, GuestComponent)]` component (`Health`), running a system that queries and updates it via deferred commands (`GuestCommands`), exporting polyglot C99/C++17/Zig headers, and passing `cargo xtask ci`:

```text
  1. bamabi_sys        ──► Wire types, offsets (68 slots), ComponentUuid, AbiPod & ModComponent traits
         │
         ▼
  2. bamabi_macros     ──► #[derive(ModComponent, GuestComponent)]: zero-pad checks, VTableHeader + C_DECL/ZIG_DECL
         │
         ├──► 3. bamabi_sdk_gen  ──► include/bamabi.h, bamabi.hpp, bamabi.zig, include/generated/testbed_components.h
         │
         ├──► 4. bamabi_guest_sys ──► Host (68-slot covers_slot), GuestInstance, ReentrantCell, catch_guest_result
         │          │
         │          ▼
         └──► 5. bamabi_guest    ──► #![forbid(unsafe_code)] GuestPlugin, GuestApp, QueryTuple, GuestCommands
                    │
                    ▼
             6. xtask            ──► header --check, gen-component-headers --check, lint, test, c-test (ASan/UBSan)
```

### Step 1 (`bamabi_sys`): Wire Primitives, 68-Slot `offsets`, and Identity Hashing

Everything begins in [`bamabi_sys`](crates/bamabi_sys/README.md) (`#![no_std]`, zero dependencies):

- **Deterministic wire primitives:** Components, queries, and commands cross `extern "C"` using `#[repr(C)]` / `#[repr(transparent)]` types with no Rust-ABI assumptions: `FfiEntity`, `BamComponent` (1-based `u32` handle), `FfiResult` (18-code `u32` status), `FfiBool` (`u8`), `FfiOption<T>`, and `FfiSlice<T>` / `StrView`.
- **Single-source-of-truth offsets (`bamabi_sys::offsets`):** `BamHostApi` declares `struct_size`, `abi_version`, and all **68** host function-pointer slots (`MIN_V0_2_SIZE = 224` B / `116` B; `MIN_V0_3_SIZE = 552` B / `280` B). `bamabi_sys::offsets` computes `OFF64_*`, `OFF32_*`, `SIZE_64 = 552`, `SIZE_32 = 280`, and `SLOT_COUNT = 68` via `core::mem::offset_of!`.
- **Const UUID & Schema Fingerprint:** `ComponentUuid::from_parts(name, size, align, field_schema_hash)` hashes component identity in `const` using keyed SipHash-2-4-128 (`SIPHASH_KEY_0`, `SIPHASH_KEY_1`), while `ModComponent::SCHEMA_FINGERPRINT` carries a 64-bit FNV-1a structural hash (`FNV1A_64_OFFSET_BASIS`, `FNV1A_64_PRIME`).
- **Safety contracts:** `RawInsertable`, `AbiPod`, and `ModComponent` define the trait contract enforced at compile time (`assert_abi_pod::<T>()`).

### Step 2 (`bamabi_macros`): Compile-Time Layout Proofs, VTables & Polyglot Decls

When a developer writes a cross-language POD component (or a rich `String`/`Vec` host component):

```rust,ignore
#[repr(C)]
#[derive(Copy, Clone, ModComponent, GuestComponent)]
#[mod_component(name = "combat::Health", summary_tick)]
#[guest_component(name = "combat::Health", summary_tick)]
pub struct Health {
    pub current: i32,
    pub max: i32,
}
```

[`bamabi_macros`](crates/bamabi_macros/README.md) (`#![forbid(unsafe_code)]`) expands both derives inside hygienic `const _: ()` blocks:

- **Zero-padding & type firewall:** `#[derive(ModComponent)]` and `#[derive(AbiPod)]` verify `#[repr(C)]` (rejecting `#[repr(packed)]`, `bool` in POD structs, `usize`, `u128`, and `Drop` types) and emit compile-time assertions that `size_of::<Health>() == sum(field sizes)` and every field satisfies `assert_abi_pod`.
- **Traits, `BAM_FIELDS` & `REQUIRED_COMPONENTS`:** Generates `impl ModComponent for Health` with `CANONICAL_NAME = "combat::Health"`, `UUID`, `SCHEMA_FINGERPRINT`, `BAM_FIELDS: &'static [BamFieldDesc]`, `REQUIRED_COMPONENTS: &'static [BamRequiredComponent]`, `IS_ABI_POD = true`, `C_DECL`, and `ZIG_DECL`.
- **Hygienic VTable & `"bam.v0.*"` symbols:** Emits a static `HealthVTable` prefixed by `VTableHeader` (`struct_size`, `abi_version`, `uuid`) plus `unsafe extern "C" fn` field/row accessors (for non-POD types like `String`/`Vec<T>`, generated setters check pointer overlap for self-aliasing safety and validate `String` UTF-8) and `SymbolEntry` exports under `"bam.v0.*"`.
- **Guest metadata (`#[derive(GuestComponent)]`):** Emits `bamabi_guest::GuestComponent` with `NAME = c"combat::Health"`, `FIELD_SCHEMA_HASH`, `FLAGS = BAM_COMPONENT_FLAG_SUMMARY_TICK`, and the matching `UUID` (or use `#[mod_component(..., guest)]` to emit both in one attribute).

### Step 3 (`bamabi_sdk_gen`): Zero-Drift C99, C++17 & Zig Synthesis

[`bamabi_sdk_gen`](crates/bamabi_sdk_gen/README.md) (`#![forbid(unsafe_code)]`, depends only on `bamabi_sys`) turns Rust wire metadata into polyglot headers without duplicating constants:

- **Core ABI header (`header_gen::render_c99_header`):** Reads `bamabi_sys::offsets` and a 20-struct layout specification table (`wire_struct_specs`), verifies `verify_host_rust_layouts()` and `BAM_HOST_API_SLOT_COUNT == 68` before emitting a single byte, and renders [`include/bamabi.h`](include/bamabi.h) with all 68 `BAM_HOST_API_OFFSET_*` macros, `bam_uuid_from_parts`, `bam_host_api_covers_slot`, `bam_host_api_copy_bounded`, and 64-bit/32-bit `BAM_STATIC_ASSERT` checks.
- **Polyglot bundle (`sdk::export_sdk`):** Emits `bamabi.h` (C99), `bamabi.hpp` (C++17 68/68 slot view wrappers + RAII `ScopedTransaction`), and `bamabi.zig` (Zig `0.16` `b.addTranslateC` `"bam_c"` bindings + lossless 18-code `BamError`) so C/C++/Zig mods see the exact same `BamHostApi` table as Rust mods.
- **Component declarations (`component_header`):** `section_for::<Health>()` reads `(Health::CANONICAL_NAME, Health::IS_ABI_POD, Health::C_DECL)` and `render_component_header` wraps it in `#ifndef` guards, `#include "../bamabi.h"`, and `extern "C"` blocks (used to generate [`include/generated/testbed_components.h`](include/generated/testbed_components.h)).

### Step 4 (`bamabi_guest_sys`): Host Table Validation, Panic Boundary & Data Plane

[`bamabi_guest_sys`](crates/bamabi_guest_sys/README.md) isolates every guest-side `unsafe` operation behind fail-closed runtime checks:

- **Instance & table handshake (`bamabi_init`):** `init_guest_instance` allocates a heap-owned per-World `GuestInstance` (zero process-global mutable state). `Host::from_load_ctx` copies `min(struct_size, size_of::<BamHostApi>())` bytes via `read_descriptor::<BamHostApi>`, enforces `abi_version_is_compatible` and the 27 core `0.2` slots, and probes all 41 extended/v0.3 slots via `BamHostApi::covers_slot` (returning `FfiResult::UNSUPPORTED` if absent).
- **Registration & VTables:** `ValidatedLoadCtx::register_pod_component` populates `ComponentDesc` (with `schema_fingerprint`, `fields`, and `required_init_descs`) and invokes `BamHostApi::register_component` to obtain a 1-based `BamComponent` handle. Host-exported VTables can also be resolved and verified (`VTableHeader` size/version/UUID) via `Host::resolve_vtable`.
- **Lock-free re-entrancy & panic containment:** `register_system_closure` wraps each `FnMut` system closure in `ReentrantCell<F>` (`UnsafeCell` + `AtomicBool` `Acquire`/`Release` + `ReleaseGuard`), rejecting re-entrant execution with `FfiResult::ACCESS_CONFLICT`. Its `extern "C"` trampoline (`BamSystemFn`) executes inside `panic::catch_guest_result` (`catch_unwind`), dropping any panic payload on the guest heap and returning `FfiResult::PANIC_CAUGHT` (`3`) instead of unwinding across `extern "C"`.
- **Validated iteration & deferred commands:** Inside the system tick, `ValidatedIter` checks `BamColumnInfo` (`mode`, `size`, `align`, `uuid`, storage flags) and `ChunkColumnDispenser` prevents mutable aliasing across query terms; `ValidatedCommands::insert` validates `BamLayoutInfo` (and `SpawnEntry::new::<T>` checks `(size, align)` on batched spawns) before calling `BamHostApi::cmd_insert` on `*mut BamCommands`.

### Step 5 (`bamabi_guest`): Safe `#![forbid(unsafe_code)]` Mod Authoring

[`bamabi_guest`](crates/bamabi_guest/README.md) builds an idiomatic, `#![forbid(unsafe_code)]` Rust API on top of `bamabi_guest_sys` and `bamabi_sys`:

- **Plugin & entry-point export:** Implementing `GuestPlugin` and invoking `export_mod!(plugin: CombatMod)` exports `bamabi_manifest`, `bamabi_init`, `bamabi_shutdown`, and `bamabi_resolve_symbol` backed by per-World `GuestInstance` ownership.
- **Component, query & deferred command wiring:** Inside `GuestPlugin::build(&mut self, app: &mut GuestApp<'_>)`:
  - `app.register_component::<Health>()` (or `register_component_with_options`) forwards `Health`'s `GuestComponent` metadata to `ValidatedLoadCtx`.
  - `app.system(c"regen").schedule(ScheduleId::UPDATE).query_typed::<(FfiEntity, &mut Health)>().commands().build(|mut ctx| { ... })` builds `BamQueryDesc` and `BamSystemDesc`, exposes safe `ChunkSliceMut` columns via `ctx.single_query::<(FfiEntity, &mut Health)>()`, and enqueues deferred mutations via `ctx.commands()` (`GuestCommands::insert::<Health>(entity, &updated)` or `GuestEntityCommands`).

### Step 6 (`xtask`): Codegen Sync & Multi-Tier CI Gate

[`xtask`](xtask/README.md) (`#![forbid(unsafe_code)]`) ties the entire workspace together in one reproducible verification loop (`cargo xtask ci`):

1. **`xtask/src/component_headers.rs`:** Collects `section_for::<T>()` for 19 canonical `#[derive(ModComponent)]` fixtures and keeps [`include/generated/testbed_components.h`](include/generated/testbed_components.h) synchronized (`cargo xtask gen-component-headers`).
2. **Byte-parity checks:** `cargo xtask header --check` verifies [`include/bamabi.h`](include/bamabi.h) matches `bamabi_sdk_gen::render_c99_header()` byte-for-byte; `cargo xtask gen-component-headers --check` verifies `testbed_components.h`.
3. **Static & isolation lints (`cargo xtask lint`):** Confirms `#![forbid(unsafe_code)]` in `bamabi_docs`, `bamabi_macros`, `bamabi_sdk_gen`, `bamabi_guest`, and `xtask`; confirms zero `bevy*` crates in `cargo tree --workspace`; runs `clippy -D warnings` and `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`.
4. **Rust + C99 + sanitizer execution:** `cargo xtask test` runs `--no-default-features`, `--all-features`, and `--doc`; `cargo xtask c-test` validates both generated headers under strict `-std=c99 -Wall -Wextra -Werror -pedantic -fsyntax-only` and compiles/runs [`tests/standalone_c_test.c`](tests/standalone_c_test.c) under AddressSanitizer and UndefinedBehaviorSanitizer.

## 7. Crate Onboarding Index

Each workspace crate includes a `README.md`, an `ARCHITECTURE.md` deep dive, and a `TESTS.md` verification catalog, and is browsable locally in `rustdoc` via `cargo docs --open` (alias for `cargo doc --workspace --no-deps --open`, landing on `target/doc/bamabi_docs/index.html`):

| Crate | README | Architecture | Tests & Proofs |
|---|---|---|---|
| `bamabi_docs` | [`crates/bamabi_docs/README.md`](crates/bamabi_docs/README.md) | [`crates/bamabi_docs/ARCHITECTURE.md`](crates/bamabi_docs/ARCHITECTURE.md) | [`crates/bamabi_docs/TESTS.md`](crates/bamabi_docs/TESTS.md) |
| `bamabi_sys` | [`crates/bamabi_sys/README.md`](crates/bamabi_sys/README.md) | [`crates/bamabi_sys/ARCHITECTURE.md`](crates/bamabi_sys/ARCHITECTURE.md) | [`crates/bamabi_sys/TESTS.md`](crates/bamabi_sys/TESTS.md) |
| `bamabi_macros` | [`crates/bamabi_macros/README.md`](crates/bamabi_macros/README.md) | [`crates/bamabi_macros/ARCHITECTURE.md`](crates/bamabi_macros/ARCHITECTURE.md) | [`crates/bamabi_macros/TESTS.md`](crates/bamabi_macros/TESTS.md) |
| `bamabi_sdk_gen` | [`crates/bamabi_sdk_gen/README.md`](crates/bamabi_sdk_gen/README.md) | [`crates/bamabi_sdk_gen/ARCHITECTURE.md`](crates/bamabi_sdk_gen/ARCHITECTURE.md) | [`crates/bamabi_sdk_gen/TESTS.md`](crates/bamabi_sdk_gen/TESTS.md) |
| `bamabi_guest_sys` | [`crates/bamabi_guest_sys/README.md`](crates/bamabi_guest_sys/README.md) | [`crates/bamabi_guest_sys/ARCHITECTURE.md`](crates/bamabi_guest_sys/ARCHITECTURE.md) | [`crates/bamabi_guest_sys/TESTS.md`](crates/bamabi_guest_sys/TESTS.md) |
| `bamabi_guest` | [`crates/bamabi_guest/README.md`](crates/bamabi_guest/README.md) | [`crates/bamabi_guest/ARCHITECTURE.md`](crates/bamabi_guest/ARCHITECTURE.md) | [`crates/bamabi_guest/TESTS.md`](crates/bamabi_guest/TESTS.md) |
| `xtask` | [`xtask/README.md`](xtask/README.md) | [`xtask/ARCHITECTURE.md`](xtask/ARCHITECTURE.md) | [`xtask/TESTS.md`](xtask/TESTS.md) |

Related workspace guides: [`DEVELOPMENT.md`](DEVELOPMENT.md) and [`TESTING.md`](TESTING.md).
