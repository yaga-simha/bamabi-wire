# `bamabi_sdk_gen` Test & Verification Guide

This document catalogs every unit, parity, polyglot compiler, and standalone C99 test covering `bamabi_sdk_gen` and its generated ABI `0.3.0` headers, explaining **why** each test exists and **what** invariant, parity, undefined-behavior (UB), or aliasing property it proves.

---

## 1. Quick Run Commands

```bash
# Run all Rust unit, parity, and polyglot compiler tests in bamabi_sdk_gen
cargo test -p bamabi_sdk_gen --all-features

# Run doctests in src/lib.rs
cargo test --doc -p bamabi_sdk_gen --all-features

# Run strict header parity gates
cargo xtask header --check
cargo xtask gen-component-headers --check

# Run C99 syntax checks + standalone_c_test.c (regular + ASan/UBSan)
cargo xtask c-test
cargo xtask c-test --no-asan    # skip sanitizer build if clang is absent
cargo xtask c-test --asan-only  # run ASan + UBSan only
```

---

## 2. Unit Tests in `crates/bamabi_sdk_gen/src/`

### 2.1 `header_gen::tests` (`crates/bamabi_sdk_gen/src/header_gen.rs`)

| Test | Why It Exists | Invariants & Properties Proved |
|---|---|---|
| `header_generation_is_deterministic_and_bam_prefixed` | Guarantees `render_c99_header()` is reproducible across runs and free of legacy `v0.1` unprefixed symbols or missing ABI `0.3.0` macros. | 1. **Byte-for-byte determinism:** Two consecutive `render_c99_header()` calls produce identical strings.<br>2. **ABI `0.3.0` constants & helpers:** Emits `BAM_ABI_VERSION 0x00030000u`, `BAM_HOST_API_SLOT_COUNT 68u`, `BAM_DISABLED_NAME`, `bam_asset_load_state_is_loading`, `bam_event_kind_is_lifecycle`, `bam_schedule_is_valid`, and `bam_disabled_uuid(void)`.<br>3. **Export & aliasing portability:** Emits `BAM_EXPORT` (`__declspec(dllexport)` on Windows) and `BAM_RESTRICT` (`__restrict` on MSVC, `restrict` in C99+, empty otherwise).<br>4. **68-slot macro completeness:** Every slot in `host_api_slot_names()` (`68` slots) has `#define BAM_HOST_API_OFFSET_<UPPER> offsetof(BamHostApi, <slot>)`.<br>5. **64/32-bit layout assertions & floor constants:** Emits `sizeof(BamHostApi) == 552u` (64-bit) / `280u` (32-bit), `sizeof(BamComponentDesc) == 176u` (64-bit) / `104u` (32-bit), `BAM_HOST_API_MIN_V0_2_SIZE == 224u`, `BAM_HOST_API_MIN_V0_3_SIZE == 552u`.<br>6. **No `v0.1` regression:** Excludes removed unprefixed typedefs (`HostApi`, `SystemDescriptor`, `RawChunkIter`, `ScheduleLabelId`).<br>7. **Cross-language `SipHash-2-4-128` golden parity:** Asserts `ComponentUuid::from_parts("combat_mod::Health", 4, 4, 0)` and `ComponentUuid::from_canonical_name("my_game::Inventory")` match the exact 16-byte golden vectors tested in `tests/standalone_c_test.c`. |

### 2.2 `sdk::tests` (`crates/bamabi_sdk_gen/src/sdk.rs`)

| Test | Why It Exists | Invariants & Properties Proved |
|---|---|---|
| `export_sdk_writes_all_three_abi_0_3_headers` | Verifies the `export_sdk` filesystem bundle consumed by `cargo bam export-sdk` (`bamabi-tools`) writes all three files with `ABI 0.3.0` constructs and Zig `0.16` `addTranslateC` conventions. | 1. `export_sdk` creates the target directory and writes exactly 3 files: `bamabi.h`, `bamabi.hpp`, `bamabi.zig`.<br>2. `bamabi.h` contains `0x00030000u`, `BAM_RESTRICT restrict`, `BAM_RESULT_CYCLE_DETECTED`, relation flags (`CASCADE_DELETE`, `ACYCLIC`, `CLEANUP_REMOVE`), and schedule-set flags (`SEQUENCE`, `SEQUENCE_WEAK`, `PARALLEL`, `BEFORE_WEAK`, `AFTER_WEAK`).<br>3. `bamabi.hpp` contains `BAM_HOST_API_OFFSET_CMD_CHECKPOINT` and `BAM_RESTRICT`.<br>4. `bamabi.zig` targets `ABI 0.3.0`, uses `addTranslateC` + `@import("bam_c")`, omits legacy `@cImport`/`@cInclude`, and defines `BAM_RESTRICT_QUALIFIER` and `BAM_RESULT_CYCLE_DETECTED`. |
| `all_68_host_api_offsets_and_restrict_mirrored_across_c_cpp_and_zig` | Prevents C99, C++17, and Zig offset constants or non-aliasing qualifiers from drifting out of sync when slots are added or renamed. | 1. **Aliasing parity:** Verifies `BAM_RESTRICT` in C99 (`__restrict` / `restrict`), `T* BAM_RESTRICT dst` in `CPP17_HEADER`, and `noalias dst: [*]T` in `ZIG_SDK`.<br>2. **Header field parity:** Checks `STRUCT_SIZE` and `ABI_VERSION` offset definitions in both `CPP17_HEADER` (`offsetof`) and `ZIG_SDK` (`@offsetOf`).<br>3. **Exhaustive 68/68 slot offset parity:** Iterates all 68 slots from `host_api_slot_names()` and asserts each has `#define BAM_HOST_API_OFFSET_<UPPER>` in C99, `inline constexpr std::size_t <UPPER>` in C++17, and both `offsets.<UPPER>` + `BAM_HOST_API_OFFSET_<UPPER>` in Zig. |
| `cpp17_and_zig_views_wrap_all_68_slots_and_all_18_result_codes` | Ensures no `BamHostApi` slot is left unwrapped in either high-level language SDK and no `BamResult` status code is dropped in Zig. | 1. **68/68 view method coverage:** Verifies `CPP17_HEADER` exposes `ScopedTransaction` and calls `api_.<slot>(` or `api_-><slot>(` for all 68 slots, and `ZIG_SDK` accesses `self.api.<slot>` for all 68 slots.<br>2. **18/18 `BamResult` coverage:** Verifies `ZIG_SDK` handles all 18 `BAM_RESULT_*` codes (`OK` through `INTERNAL_ERROR`, including `CYCLE_DETECTED`). |

---

## 3. Integration Tests in `crates/bamabi_sdk_gen/tests/`

### 3.1 `header_parity.rs` (`crates/bamabi_sdk_gen/tests/header_parity.rs`)

| Test | Why It Exists | Invariants & Properties Proved |
|---|---|---|
| `checked_in_c99_header_matches_render_c99_header_byte_for_byte` | Acts as the primary CI zero-drift gate between `crates/bamabi_sys` and the checked-in `include/bamabi.h` consumed by C/C++/Zig tools. | Reads `include/bamabi.h` (returning early if absent in packaged standalone builds outside a repo checkout), asserts exact byte-for-byte equality with `render_c99_header()` with a line-diff diagnostic on mismatch, and verifies `write_or_check_header(repo_root, true)` succeeds. |
| `generated_testbed_components_header_includes_parent_bamabi_h` | Ensures generated component headers under `include/generated/` resolve `include/bamabi.h` via a relative include without requiring custom include path overrides. | When `include/generated/testbed_components.h` exists, proves it contains `#include "../bamabi.h"`. (Full byte-parity of `testbed_components.h` against `ModComponent::C_DECL` is enforced by `cargo xtask gen-component-headers --check` and `xtask`'s `generated_testbed_header_matches_checked_in_file` test.) |

### 3.2 `sdk_compile_tests.rs` (`crates/bamabi_sdk_gen/tests/sdk_compile_tests.rs`)

Both tests gracefully skip (`return`) if `c++` or `zig` is absent from `PATH`, and fail loudly with compiler diagnostics if the tool is installed and compilation or execution fails.

| Test | Why It Exists | Invariants & Properties Proved |
|---|---|---|
| `exported_cpp17_header_compiles_and_passes_strict_checks` | Proves `bamabi.h` + `bamabi.hpp` compile cleanly under strict ISO C++17 warnings-as-errors and behave correctly at runtime. | Exports the SDK to a temp directory and compiles + executes a mock C++17 harness under `-std=c++17 -Wall -Wextra -Wpedantic -Werror` proving:<br>1. `bam::offsets::*` `static_assert`s pass on the host target.<br>2. `LoadContextView`, `HostContextView`, `SystemContextView`, `ChunkView`, and `CommandsView` invoke slots cleanly.<br>3. Missing/out-of-bounds extension slots (`get_proc_address`, `asset_load`, `intern_symbol`, `bind_named_object`) return `nullptr` / `BAM_RESULT_UNSUPPORTED` rather than dereferencing unpopulated slots.<br>4. `ChunkView` iteration (`next_chunk`, `entities`, `column_readonly`, `column_mutable`, `column_mutable_untracked`, `reset`) reads and mutates typed columns cleanly.<br>5. `ScopedTransaction` automatically rolls back (`token == 1`) on scope exit without `commit()`, and suppresses rollback (`token == 2`) when `commit()` is called. |
| `exported_zig_sdk_compiles_and_passes_with_translate_c` | Proves `bamabi.h` translates cleanly through Zig `0.16` `b.addTranslateC` (`"bam_c"`) and `bamabi.zig` compiles and passes unit tests. | Exports the SDK to a temp directory, generates a `build.zig` with `b.addTranslateC(.{ .root_source_file = b.path("bamabi.h") })`, and runs `zig build test` proving:<br>1. **Comptime offset checks pass:** `offsets.STRUCT_SIZE == 0`, `ABI_VERSION == 4`, `GET_PROC_ADDRESS == 8`, and tail slot ends at `@sizeOf(c.BamHostApi)`.<br>2. **Lossless 18-code `BamResult` round-trip:** `bam.checkResult(BAM_RESULT_OK)` succeeds and all 17 non-OK `BamResult` codes map to `BamError` and round-trip back to the exact `c.BamResult` via `bam.mapError(err)`.<br>3. **All 68 slot wrappers compile semantically:** References every method across `HostContextView`, `LoadContextView`, `ChunkView`, `CommandsView`, and `SystemContextView`.<br>4. **Null/unsupported prefix guards:** Zero-initialized `BamHostApi` returns `null` / `error.Unsupported` across nullable and optional slots (`getProcAddress`, `internSymbol`, `columnReadonly`, `columnMutable`, `columnMutableUntracked`, `resPtr`, `resMutPtr`, `msgReadBatch`, `msgWriteBatch`, `entityGet`, `entityGetMut`). |

---

## 4. Standalone C99 Conformance, Layout & Sanitizer Suite (`tests/standalone_c_test.c`)

`cargo xtask c-test` executes three layers of C99 verification:

1. **Header Syntax-Only Conformance (`-std=c99 -Wall -Wextra -Werror -pedantic -fsyntax-only`):** Compiles both `include/bamabi.h` and `include/generated/testbed_components.h` standalone as pure ISO C99 headers, proving zero non-standard extensions, zero missing includes, and passing all `BAM_STATIC_ASSERT` size/align/offset checks on the host (`64`-bit or `32`-bit).
2. **Strict C99 Compilation & Execution (`tests/standalone_c_test.c`):** Compiles and runs the mock host + guest harness under `-std=c99 -Wall -Wextra -Werror -pedantic`.
3. **ASan + UBSan Sanitizer Run (`-O1 -g -fno-omit-frame-pointer -fsanitize=address,undefined -fno-sanitize-recover=all`):** Recompiles and executes `tests/standalone_c_test.c` under AddressSanitizer and UndefinedBehaviorSanitizer with `halt_on_error=1`, proving zero out-of-bounds reads/writes, zero misaligned accesses, zero signed overflow, and zero strict-aliasing UB across C99 wire structs and inline helpers.

### Runtime Test Functions in `tests/standalone_c_test.c`

| Function | Why It Exists | Invariants & Properties Proved |
|---|---|---|
| `test_versions_and_result_names` | Validates `ABI 0.3.0` version helpers, error-name formatting, bitflag identities, and `BamStrView` null safety. | 1. `BAM_ABI_VERSION == 0x00030000u` and `bam_version_{major,minor,patch}` extract `(0, 3, 0)`.<br>2. `bam_version_is_compatible` enforces `0.x` exact minor equality (`0.3` rejects `0.2` and `0.4`, accepts `0.3.7`) and `1.x` minor backwards compatibility (`1.2` host accepts `1.1` guest, rejects `1.2` guest on `1.1` host).<br>3. `bam_result_name` maps `OK`, `UNSUPPORTED`, `TYPE_MISMATCH`, `CYCLE_DETECTED`, `INTERNAL_ERROR`, and unknown codes (`"UNKNOWN"`).<br>4. `BAM_RELATION_FLAG_CASCADE_DELETE == BAM_RELATION_FLAG_CASCADE_DESPAWN` and `BAM_RELATION_KNOWN_FLAGS` / `BAM_SCHEDULE_SET_KNOWN_FLAGS` cover all expected bits.<br>5. `bam_str_view(NULL)` produces `{ NULL, 0 }` without calling `strlen(NULL)` (no UB). |
| `test_cross_language_siphash_uuids` | Proves C99 `SipHash-2-4-128` produces bit-exact component UUIDs matching Rust `bamabi_sys::ComponentUuid`. | Compares `bam_uuid_from_parts("combat_mod::Health", 4, 4, 0)` and `bam_uuid_from_canonical_name("my_game::Inventory")` byte-for-byte against the golden 16-byte digests verified in Rust unit tests, and checks namespace separation (`ui_mod::Health` != `combat_mod::Health`) via `bam_uuid_equals`. |
| `test_extensible_host_api_copy_bounded` | Proves forward/backward prefix extensibility of `BamHostApi` without reading past the host's `struct_size` or leaving uninitialized trailing slots. | Copies a smaller host `BamHostApi` into a larger `FutureGuestHostApiV03` pre-filled with `0xAA` poison bytes via `bam_host_api_copy_bounded`, asserting trailing slots (`future_slot_a`, `future_slot_b`) are zeroed to `NULL` and copied slots remain intact; also verifies under-sized tables (`struct_size = 8`) fail with `BAM_RESULT_INVALID_VERSION`. |
| `test_end_to_end_c99_lifecycle_and_vtable` | Exercises a full C99 guest lifecycle against the 27 baseline `BamHostApi` slots and custom VTables. | Covers: `BamManifest` version/size check -> `mock_guest_init` -> `bam_host_api_copy_bounded` -> component registration & resolution -> `BamSystemDesc` registration with `run_if` and schedule ordering -> `get_proc_address("bam.v0.game::MockPosition.vtable")` -> `resolve_component` failure + `last_error` retrieval -> `iter_column_info`, `iter_next_chunk`, dense `iter_column_mutable_untracked` + `iter_mark_changed_range`, sparse `iter_column_rows_mutable`, singleton `res_ptr`, `iter_reset` -> `cmd_spawn` + `cmd_insert` -> drop hook & `shutdown`. |
| `test_hierarchy_ticks_layout_and_observer_descriptors` | Verifies hierarchy names, `u32` tick wraparound arithmetic, dynamic component descriptors, and 64-bit entity packing in observers. | 1. `BamChildOf` is 8 bytes and `BAM_CHILD_OF_NAME == "bevy_ecs::hierarchy::ChildOf"`.<br>2. `bam_ticks_is_added` / `bam_ticks_is_changed` handle `NULL` safely (`0`) and wrap cleanly across the `0xFFFFFFFEu` -> `2u` `uint32_t` boundary.<br>3. `bam_desc_component_with_layout` computes matching `BamComponentUuid` and populates `struct_size`.<br>4. `bam_observer_desc` splits `0x11223344AABBCCDDull` into `entity_lo = 0xAABBCCDDu` and `entity_hi = 0x11223344u`.<br>5. Verifies `BAM_STORAGE_FLAG_EVENT == 0x10u`, `BAM_STORAGE_FLAG_ENTITY_EVENT == 0x20u`, `BamSystemDesc.in_sets`, and zero-initialized extension slots. |
| `test_extended_wire_descriptors` | Verifies all `ABI 0.3.0` macros, `BAM_OPTION_STRUCT`, `BamAssetHandle`, `BamAccessMode`, inline predicates, and 68-slot `BamHostApi` coverage checks. | 1. `BAM_STATIC_ASSERT` on `BAM_OPTION_STRUCT(uint32_t, BamOptionU32)` (8 B, `is_some` at 0, `value` at 4) and `BamOptionEntity`.<br>2. `BamAssetHandle` (`none`, `from_index`, `from_uuid`) and `BamAssetLoadState` predicates.<br>3. `BAM_ACCESS_ALLOW` (`12`), `HAS` (`13`), `REMOVED_READ` (`14`), `bam_term_desc_or`.<br>4. Extended `BamComponentDesc`, `BamObserverDesc`, `BamTriggerDesc`, `BamSystemDesc` (`after_weak`).<br>5. Inline predicates (`bam_disabled_uuid`, `bam_entity_is_valid`, `bam_asset_load_state_is_*`, `bam_event_kind_is_lifecycle`, `bam_schedule_is_valid`, `bam_field_kind_is_valid`).<br>6. `BAM_HOST_API_SLOT_COUNT == 68u` and `bam_host_api_covers_slot` on `LOG_DIAGNOSTIC` and `MIGRATE_COMPONENT_COLUMN`. |
