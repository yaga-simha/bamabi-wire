//! Multi-language SDK exporter (`bamabi.h`, `bamabi.hpp`, `bamabi.zig`).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use bamabi_sys::FfiResult;

/// Canonical ordered list of all 18 `bamabi_sys::FfiResult` status codes.
const ALL_FFI_RESULTS: [FfiResult; 18] = [
    FfiResult::OK,
    FfiResult::INVALID_COMPONENT,
    FfiResult::ACCESS_CONFLICT,
    FfiResult::PANIC_CAUGHT,
    FfiResult::NULL_POINTER,
    FfiResult::INVALID_UTF8,
    FfiResult::PERMISSION_DENIED,
    FfiResult::INVALID_VERSION,
    FfiResult::INVALID_ARGUMENT,
    FfiResult::OUT_OF_RANGE,
    FfiResult::NOT_FOUND,
    FfiResult::INVALID_STATE,
    FfiResult::INVALID_ENTITY,
    FfiResult::CAPACITY_EXCEEDED,
    FfiResult::UNSUPPORTED,
    FfiResult::TYPE_MISMATCH,
    FfiResult::CYCLE_DETECTED,
    FfiResult::INTERNAL_ERROR,
];

/// Converts a `SCREAMING_SNAKE_CASE` identifier (such as `FfiResult::name()`) to `PascalCase`.
fn screaming_snake_to_pascal(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for part in name.split('_') {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            for ch in chars {
                out.push(ch.to_ascii_lowercase());
            }
        }
    }
    out
}

/// Generates the canonical strict ISO C99 header directly from `bamabi_sys`.
///
/// Intended purpose: provides a convenience wrapper for CLI and test callers that need the
/// rendered `bamabi.h` text without handling layout-assertion errors.
///
/// What it does: delegates to [`crate::render_c99_header`] and returns an inline C comment error
/// sentinel if layout validation fails.
#[must_use]
pub fn c99_header() -> String {
    crate::render_c99_header()
        .unwrap_or_else(|err| format!("/* render_c99_header error: {err} */\n"))
}

/// Synthesizes the ergonomic ISO C++17 RAII wrapper header over `bamabi.h` (`ABI 0.4.0`).
///
/// Programmatically emits all `bam::offsets::*` compile-time constants directly from
/// `header_gen::host_api_slot_names` so C++17 offset definitions cannot drift from
/// `bamabi_sys` or `include/bamabi.h`.
#[must_use]
pub fn cpp17_header() -> String {
    let slots = crate::header_gen::host_api_slot_names();
    let last_slot = slots.last().copied().unwrap_or("cmd_spawn_batch_ex");
    let last_upper = last_slot.to_ascii_uppercase();

    let mut out = String::with_capacity(32 * 1024);
    out.push_str(CPP17_HEADER_PREFIX);
    for slot in &slots {
        let upper = slot.to_ascii_uppercase();
        let _ = writeln!(
            out,
            "inline constexpr std::size_t {upper} = BAM_HOST_API_OFFSET_{upper};"
        );
    }
    let _ = writeln!(
        out,
        "\n\
static_assert(STRUCT_SIZE == 0u, \"BamHostApi::struct_size must be at offset 0\");\n\
static_assert(ABI_VERSION == 4u, \"BamHostApi::abi_version must be at offset 4\");\n\
static_assert(GET_PROC_ADDRESS == 8u, \"BamHostApi slot 0 must be at offset 8\");\n\
static_assert(\n\
    {last_upper} + sizeof(BamHostApi::{last_slot}) == sizeof(BamHostApi),\n\
    \"BamHostApi tail slot must end at sizeof(BamHostApi)\"\n\
);"
    );
    out.push_str(CPP17_HEADER_BODY);
    out
}

/// Synthesizes the Zig SDK wrapper over `bamabi.h` (`ABI 0.4.0` `b.addTranslateC` `"bam_c"` module).
///
/// Programmatically emits all `offsets.*` / `BAM_HOST_API_OFFSET_*` constants directly from
/// `header_gen::host_api_slot_names` and the 18-code `BamError` / `checkResult` /
/// `mapError` mappings directly from `bamabi_sys::FfiResult` metadata.
#[must_use]
pub fn zig_sdk() -> String {
    let slots = crate::header_gen::host_api_slot_names();
    let slot_count = slots.len();
    let last_slot = slots.last().copied().unwrap_or("cmd_spawn_batch_ex");
    let last_upper = last_slot.to_ascii_uppercase();

    let mut out = String::with_capacity(36 * 1024);
    let _ = write!(
        out,
        r#"//! Zig SDK bindings for `bamabi` (`ABI 0.4.0`).
//!
//! Expects `build.zig` to translate `bamabi.h` via `b.addTranslateC(...)`
//! and attach the resulting module as `"bam_c"` (`mod.addImport("bam_c", bam_c_mod)`).

pub const c = @import("bam_c");

/// Zig expresses C99/MSVC `BAM_RESTRICT` non-aliasing guarantees on SoA column pointers via the
/// built-in `noalias` parameter qualifier (for example:
/// `fn integrate(noalias pos: [*]Pos, noalias vel: [*]const Vel, len: usize) void`).
pub const BAM_RESTRICT_QUALIFIER = "noalias";

/// Compile-time byte offsets for `c.BamHostApi` header fields and all {slot_count} function-pointer slots,
/// mirroring `bamabi_sys::offsets` and `BAM_HOST_API_OFFSET_*` via `@offsetOf`.
pub const offsets = struct {{
    pub const SLOT_COUNT: usize = {slot_count};
    pub const STRUCT_SIZE: usize = @offsetOf(c.BamHostApi, "struct_size");
    pub const ABI_VERSION: usize = @offsetOf(c.BamHostApi, "abi_version");

"#
    );

    for slot in &slots {
        let upper = slot.to_ascii_uppercase();
        let _ = writeln!(
            out,
            "    pub const {upper}: usize = @offsetOf(c.BamHostApi, \"{slot}\");"
        );
    }
    out.push_str("};\n\n");

    for slot in &slots {
        let upper = slot.to_ascii_uppercase();
        let _ = writeln!(
            out,
            "pub const BAM_HOST_API_OFFSET_{upper}: usize = offsets.{upper};"
        );
    }

    let _ = write!(
        out,
        r#"
comptime {{
    if (offsets.STRUCT_SIZE != 0 or offsets.ABI_VERSION != 4 or offsets.GET_PROC_ADDRESS != 8) {{
        @compileError("BamHostApi header offsets mismatch");
    }}
    if (offsets.{last_upper} + @sizeOf(usize) != @sizeOf(c.BamHostApi)) {{
        @compileError("BamHostApi tail slot offset mismatch");
    }}
}}

/// Returns `true` when `api.struct_size` covers a slot of `slot_size` bytes at `slot_offset`.
pub inline fn coversHostApiSlot(api: *const c.BamHostApi, slot_offset: usize, slot_size: usize) bool {{
    const sz: usize = @as(usize, api.struct_size);
    return slot_offset <= sz and slot_size <= (sz - slot_offset);
}}

/// Copies `len` elements between non-overlapping SoA columns using Zig's `noalias`
/// parameter qualifier (the Zig equivalent of C99/MSVC `BAM_RESTRICT`).
pub fn copyColumnNoAlias(
    comptime T: type,
    noalias dst: [*]T,
    noalias src: [*]const T,
    len: usize,
) void {{
    @memcpy(dst[0..len], src[0..len]);
}}

pub const BamError = error{{
"#
    );

    for res in ALL_FFI_RESULTS {
        if res.is_ok() {
            continue;
        }
        if res == FfiResult::INTERNAL_ERROR {
            out.push_str("    OutOfMemory,\n");
        }
        let pascal = screaming_snake_to_pascal(res.name());
        let _ = writeln!(out, "    {pascal},");
    }
    out.push_str("};\n\n");

    out.push_str("pub fn checkResult(res: c.BamResult) BamError!void {\n");
    out.push_str("    return switch (res) {\n");
    for res in ALL_FFI_RESULTS {
        let name = res.name();
        if res.is_ok() {
            let _ = writeln!(out, "        c.BAM_RESULT_{name} => {{}},");
        } else {
            let pascal = screaming_snake_to_pascal(name);
            let _ = writeln!(out, "        c.BAM_RESULT_{name} => error.{pascal},");
        }
    }
    out.push_str("        else => error.InternalError,\n");
    out.push_str("    };\n");
    out.push_str("}\n\n");

    out.push_str("pub fn mapError(err: BamError) c.BamResult {\n");
    out.push_str("    return switch (err) {\n");
    for res in ALL_FFI_RESULTS {
        if res.is_ok() {
            continue;
        }
        let name = res.name();
        let pascal = screaming_snake_to_pascal(name);
        if res == FfiResult::INTERNAL_ERROR {
            let _ = writeln!(
                out,
                "        error.OutOfMemory, error.{pascal} => c.BAM_RESULT_{name},"
            );
        } else {
            let _ = writeln!(out, "        error.{pascal} => c.BAM_RESULT_{name},");
        }
    }
    out.push_str("    };\n");
    out.push_str("}\n");

    out.push_str(ZIG_SDK_BODY);
    out
}

/// Ergonomic ISO C++17 RAII wrapper header over `bamabi.h` (`ABI 0.4.0`), lazily synthesized via
/// [`cpp17_header`].
pub static CPP17_HEADER: LazyLock<String> = LazyLock::new(cpp17_header);

/// Zig SDK wrapper over `bamabi.h` (`ABI 0.4.0` `b.addTranslateC` `"bam_c"` module), lazily
/// synthesized via [`zig_sdk`].
pub static ZIG_SDK: LazyLock<String> = LazyLock::new(zig_sdk);

const CPP17_HEADER_PREFIX: &str = r#"#ifndef BAMABI_HPP
#define BAMABI_HPP

#include "bamabi.h"

#include <cstddef>
#include <cstdint>
#include <exception>

#ifndef BAM_RESTRICT
#  if defined(_MSC_VER)
#    define BAM_RESTRICT __restrict
#  elif defined(__GNUC__) || defined(__clang__)
#    define BAM_RESTRICT __restrict__
#  else
#    define BAM_RESTRICT
#  endif
#endif

namespace bam {

namespace offsets {

inline constexpr std::size_t SLOT_COUNT = BAM_HOST_API_SLOT_COUNT;
inline constexpr std::size_t STRUCT_SIZE = offsetof(BamHostApi, struct_size);
inline constexpr std::size_t ABI_VERSION = offsetof(BamHostApi, abi_version);

"#;

const CPP17_HEADER_BODY: &str = r#"
} // namespace offsets

inline bool is_ok(BamResult res) noexcept {
    return res == BAM_RESULT_OK;
}

inline uint64_t fnv1a_64_init() noexcept {
    return bam_fnv1a_64_init();
}

inline uint64_t fnv1a_64_update_bytes(uint64_t hash, const uint8_t* bytes, std::size_t len) noexcept {
    return bam_fnv1a_64_update_bytes(hash, bytes, len);
}

inline uint64_t fnv1a_64_update_u64(uint64_t hash, uint64_t value) noexcept {
    return bam_fnv1a_64_update_u64(hash, value);
}

inline uint64_t fnv1a_64_str(const char* s) noexcept {
    return bam_fnv1a_64_str(s);
}

inline uint64_t fnv1a_64_hash_system(const char* mod_name, const char* system_name) noexcept {
    return bam_fnv1a_64_hash_system(mod_name, system_name);
}

inline uint64_t hash_field_schema(
    uint64_t prev_hash,
    const char* field_name,
    const char* field_type_tag,
    uint64_t field_offset
) noexcept {
    return bam_hash_field_schema(prev_hash, field_name, field_type_tag, field_offset);
}

inline bool host_api_covers_slot(
    const BamHostApi& api,
    std::size_t slot_offset,
    std::size_t slot_size
) noexcept {
    return bam_host_api_covers_slot(&api, slot_offset, slot_size) != 0;
}

template <typename T>
inline void copy_column_restricted(
    T* BAM_RESTRICT dst,
    const T* BAM_RESTRICT src,
    std::size_t count
) noexcept {
    for (std::size_t i = 0u; i < count; ++i) {
        dst[i] = src[i];
    }
}

template <typename Fn>
inline BamResult guard_boundary(Fn&& fn) noexcept {
    try {
        return fn();
    } catch (const std::exception&) {
        return BAM_RESULT_PANIC_CAUGHT;
    } catch (...) {
        return BAM_RESULT_PANIC_CAUGHT;
    }
}

class HostContextView {
public:
    explicit HostContextView(const BamHostApi& api, const BamHostCtx* ctx) noexcept
        : api_(api), ctx_(ctx) {}

    bool valid() const noexcept { return ctx_ != nullptr; }
    const BamHostCtx* raw() const noexcept { return ctx_; }

    const void* get_proc_address(const char* name) const noexcept {
        if (ctx_ == nullptr || api_.get_proc_address == nullptr) {
            return nullptr;
        }
        return api_.get_proc_address(ctx_, name);
    }

    size_t last_error(char* buf, size_t cap) const noexcept {
        if (ctx_ == nullptr || api_.last_error == nullptr) {
            if (buf != nullptr && cap > 0u) {
                buf[0] = '\0';
            }
            return 0u;
        }
        return api_.last_error(ctx_, buf, cap);
    }

    BamResult resolve_component(
        const char* name,
        BamComponent& out,
        const BamComponentUuid* uuid = nullptr
    ) const noexcept {
        if (ctx_ == nullptr || api_.resolve_component == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_.resolve_component(ctx_, name, uuid, &out);
    }

    BamResult asset_load(
        BamComponent asset_type,
        BamStrView path,
        BamAssetHandle& out_handle
    ) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_ASSET_LOAD, sizeof(BamAssetLoadFn)) ||
            api_.asset_load == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.asset_load(ctx_, asset_type, &path, &out_handle);
    }

    BamResult asset_load(
        BamComponent asset_type,
        const char* path,
        BamAssetHandle& out_handle
    ) const noexcept {
        return asset_load(asset_type, bam_str_view(path), out_handle);
    }

    BamResult asset_get_load_state(
        const BamAssetHandle& handle,
        BamAssetLoadState& out_state
    ) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_ASSET_GET_LOAD_STATE, sizeof(BamAssetGetLoadStateFn)) ||
            api_.asset_get_load_state == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.asset_get_load_state(ctx_, &handle, &out_state);
    }

    BamResult asset_release(const BamAssetHandle& handle) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_ASSET_RELEASE, sizeof(BamAssetReleaseFn)) ||
            api_.asset_release == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.asset_release(ctx_, &handle);
    }

    BamResult log_diagnostic(uint32_t level, BamStrView msg) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_LOG_DIAGNOSTIC, sizeof(BamLogDiagnosticFn)) ||
            api_.log_diagnostic == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.log_diagnostic(ctx_, level, &msg);
    }

    BamResult log_diagnostic(uint32_t level, const char* msg) const noexcept {
        return log_diagnostic(level, bam_str_view(msg));
    }

    BamResult report_guest_fault(BamStrView msg) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_REPORT_GUEST_FAULT, sizeof(BamReportGuestFaultFn)) ||
            api_.report_guest_fault == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.report_guest_fault(ctx_, &msg);
    }

    BamResult report_guest_fault(const char* msg) const noexcept {
        return report_guest_fault(bam_str_view(msg));
    }

    BamResult resolve_component_by_uuid(const BamComponentUuid& uuid, BamComponent& out) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_RESOLVE_COMPONENT_BY_UUID, sizeof(BamResolveComponentByUuidFn)) ||
            api_.resolve_component_by_uuid == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.resolve_component_by_uuid(ctx_, &uuid, &out);
    }

    BamResult component_layout_info(BamComponent component, BamLayoutInfo& out) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_COMPONENT_LAYOUT_INFO, sizeof(BamComponentLayoutInfoFn)) ||
            api_.component_layout_info == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.component_layout_info(ctx_, component, &out);
    }

    BamResult intern_symbol(BamStrView symbol, BamSymbol& out) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_INTERN_SYMBOL, sizeof(BamInternSymbolFn)) ||
            api_.intern_symbol == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.intern_symbol(ctx_, &symbol, &out);
    }

    BamResult intern_symbol(const char* symbol, BamSymbol& out) const noexcept {
        return intern_symbol(bam_str_view(symbol), out);
    }

    BamResult bind_named_object(BamStrView name, BamEntity entity) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_BIND_NAMED_OBJECT, sizeof(BamBindNamedObjectFn)) ||
            api_.bind_named_object == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.bind_named_object(ctx_, &name, entity);
    }

    BamResult bind_named_object(const char* name, BamEntity entity) const noexcept {
        return bind_named_object(bam_str_view(name), entity);
    }

    BamResult resolve_named_object(BamStrView name, BamEntity& out_entity) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_RESOLVE_NAMED_OBJECT, sizeof(BamResolveNamedObjectFn)) ||
            api_.resolve_named_object == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.resolve_named_object(ctx_, &name, &out_entity);
    }

    BamResult resolve_named_object(const char* name, BamEntity& out_entity) const noexcept {
        return resolve_named_object(bam_str_view(name), out_entity);
    }

    BamResult resolve_symbol_str(BamSymbol symbol, BamStrView& out_str) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_RESOLVE_SYMBOL_STR, sizeof(BamResolveSymbolStrFn)) ||
            api_.resolve_symbol_str == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.resolve_symbol_str(ctx_, symbol, &out_str);
    }

private:
    const BamHostApi& api_;
    const BamHostCtx* ctx_;
};

class LoadContextView {
public:
    explicit LoadContextView(const BamHostApi& api, BamLoadCtx* ctx) noexcept
        : api_(api), ctx_(ctx) {}

    bool valid() const noexcept { return ctx_ != nullptr; }
    BamLoadCtx* raw() const noexcept { return ctx_; }

    HostContextView host_ctx() const noexcept {
        const BamHostCtx* h = (ctx_ != nullptr && api_.load_host_ctx != nullptr)
            ? api_.load_host_ctx(ctx_)
            : nullptr;
        return HostContextView(api_, h);
    }

    BamResult register_component(const BamComponentDesc& desc, BamComponent& out) const noexcept {
        if (ctx_ == nullptr || api_.register_component == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_.register_component(ctx_, &desc, &out);
    }

    BamResult register_system(const BamSystemDesc& desc) const noexcept {
        if (ctx_ == nullptr || api_.register_system == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_.register_system(ctx_, &desc);
    }

    BamResult register_observer(const BamObserverDesc& desc) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_REGISTER_OBSERVER, sizeof(BamRegisterObserverFn)) ||
            api_.register_observer == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.register_observer(ctx_, &desc);
    }

    BamResult register_relation(const BamRelationDesc& desc, BamComponent& out) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_REGISTER_RELATION, sizeof(BamRegisterRelationFn)) ||
            api_.register_relation == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.register_relation(ctx_, &desc, &out);
    }

    BamResult register_system_handle(const BamSystemDesc& desc, BamSystem& out) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_REGISTER_SYSTEM_HANDLE, sizeof(BamRegisterSystemHandleFn)) ||
            api_.register_system_handle == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.register_system_handle(ctx_, &desc, &out);
    }

    BamResult configure_schedule_set(const BamScheduleSetDesc& desc, BamSystemSet& out) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_CONFIGURE_SCHEDULE_SET, sizeof(BamConfigureScheduleSetFn)) ||
            api_.configure_schedule_set == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.configure_schedule_set(ctx_, &desc, &out);
    }

    BamResult mount_system_in_schedule(
        BamSystem system,
        BamScheduleId schedule,
        BamSystemSet set = BAM_SYSTEM_SET_INVALID
    ) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_MOUNT_SYSTEM_IN_SCHEDULE, sizeof(BamMountSystemInScheduleFn)) ||
            api_.mount_system_in_schedule == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.mount_system_in_schedule(ctx_, system, schedule, set);
    }

    BamResult migrate_component_column(
        BamComponent component,
        uint64_t old_fingerprint,
        const BamComponentDesc& new_desc,
        BamRowMigrateFn migrate_fn,
        void* user_data = nullptr
    ) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_MIGRATE_COMPONENT_COLUMN, sizeof(BamMigrateComponentColumnFn)) ||
            api_.migrate_component_column == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.migrate_component_column(ctx_, component, old_fingerprint, &new_desc, migrate_fn, user_data);
    }

    BamResult register_resource(const BamComponentDesc& desc, BamComponent& out) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_REGISTER_RESOURCE, sizeof(BamRegisterComponentFn)) ||
            api_.register_resource == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.register_resource(ctx_, &desc, &out);
    }

    BamResult register_message(const BamComponentDesc& desc, BamComponent& out) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_REGISTER_MESSAGE, sizeof(BamRegisterComponentFn)) ||
            api_.register_message == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.register_message(ctx_, &desc, &out);
    }

private:
    const BamHostApi& api_;
    BamLoadCtx* ctx_;
};

class ChunkView {
public:
    ChunkView() noexcept = default;
    ChunkView(const BamHostApi& api, BamIter* iter, size_t count = 0u) noexcept
        : api_(&api), iter_(iter), count_(count) {}

    bool valid() const noexcept { return api_ != nullptr && iter_ != nullptr; }
    BamIter* raw() const noexcept { return iter_; }
    size_t len() const noexcept { return count_; }

    bool next_chunk(size_t& out_len) noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->iter_next_chunk == nullptr) {
            count_ = 0u;
            out_len = 0u;
            return false;
        }
        BamBool ok = api_->iter_next_chunk(iter_, &out_len);
        count_ = (ok != BAM_FALSE) ? out_len : 0u;
        return ok != BAM_FALSE;
    }

    bool supports_next_chunk_batch() const noexcept {
        return api_ != nullptr && iter_ != nullptr &&
            bam_host_api_covers_slot(
                api_,
                BAM_HOST_API_OFFSET_ITER_NEXT_CHUNK_BATCH,
                sizeof(BamIterNextChunkBatchFn)
            ) != BAM_FALSE &&
            api_->iter_next_chunk_batch != nullptr;
    }

    bool next_chunk_batch_unchecked(
        size_t& out_len,
        const BamEntity** out_entities,
        uint8_t** out_col_ptrs,
        size_t col_count,
        uint32_t mark_changed_mask
    ) noexcept {
        BamBool ok = api_->iter_next_chunk_batch(
            iter_,
            &out_len,
            out_entities,
            out_col_ptrs,
            col_count,
            mark_changed_mask
        );
        count_ = (ok != BAM_FALSE) ? out_len : 0u;
        return ok != BAM_FALSE;
    }

    bool next_chunk_batch(
        size_t& out_len,
        const BamEntity** out_entities,
        uint8_t** out_col_ptrs,
        size_t col_count,
        uint32_t mark_changed_mask
    ) noexcept {
        if (!supports_next_chunk_batch()) {
            count_ = 0u;
            out_len = 0u;
            return false;
        }
        return next_chunk_batch_unchecked(
            out_len,
            out_entities,
            out_col_ptrs,
            col_count,
            mark_changed_mask
        );
    }

    BamResult reset() noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->iter_reset == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        count_ = 0u;
        return api_->iter_reset(iter_);
    }

    BamResult total_count(size_t& out_count) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_ITER_COUNT, sizeof(BamIterCountFn)) ||
            api_->iter_count == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->iter_count(iter_, &out_count);
    }

    const BamEntity* entities() const noexcept {
        return (api_ != nullptr && iter_ != nullptr && api_->iter_entities != nullptr)
            ? api_->iter_entities(iter_)
            : nullptr;
    }

    template <typename T>
    const T* column_readonly(size_t term_index) const noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->iter_column_readonly == nullptr) {
            return nullptr;
        }
        return reinterpret_cast<const T*>(api_->iter_column_readonly(iter_, term_index));
    }

    template <typename T>
    T* column_mutable(size_t term_index) const noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->iter_column_mutable == nullptr) {
            return nullptr;
        }
        return reinterpret_cast<T*>(api_->iter_column_mutable(iter_, term_index));
    }

    template <typename T>
    T* column_mutable_untracked(size_t term_index) const noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->iter_column_mutable_untracked == nullptr) {
            return nullptr;
        }
        return reinterpret_cast<T*>(api_->iter_column_mutable_untracked(iter_, term_index));
    }

    BamResult mark_changed_range(
        size_t term_index,
        size_t start_row,
        size_t row_count
    ) const noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->iter_mark_changed_range == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_->iter_mark_changed_range(iter_, term_index, start_row, row_count);
    }

    const uint8_t* const* column_rows_readonly(size_t term_index) const noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->iter_column_rows_readonly == nullptr) {
            return nullptr;
        }
        return api_->iter_column_rows_readonly(iter_, term_index);
    }

    uint8_t* const* column_rows_mutable(size_t term_index) const noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->iter_column_rows_mutable == nullptr) {
            return nullptr;
        }
        return api_->iter_column_rows_mutable(iter_, term_index);
    }

    uint8_t* const* column_rows_mutable_untracked(size_t term_index) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_ITER_COLUMN_ROWS_MUTABLE_UNTRACKED, sizeof(BamIterColumnRowsMutableUntrackedFn)) ||
            api_->iter_column_rows_mutable_untracked == nullptr) {
            return nullptr;
        }
        return api_->iter_column_rows_mutable_untracked(iter_, term_index);
    }

    BamResult mark_changed_ranges(
        size_t term_index,
        const BamRowRange* ranges_ptr,
        size_t range_count
    ) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_ITER_MARK_CHANGED_RANGES, sizeof(BamIterMarkChangedRangesFn)) ||
            api_->iter_mark_changed_ranges == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->iter_mark_changed_ranges(iter_, term_index, ranges_ptr, range_count);
    }

    BamResult column_info(size_t term_index, BamColumnInfo& out_info) const noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->iter_column_info == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_->iter_column_info(iter_, term_index, &out_info);
    }

    template <typename T>
    const T* res_ptr(size_t term_index) const noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->res_ptr == nullptr) {
            return nullptr;
        }
        return reinterpret_cast<const T*>(api_->res_ptr(iter_, term_index));
    }

    template <typename T>
    T* res_mut_ptr(size_t term_index) const noexcept {
        if (api_ == nullptr || iter_ == nullptr || api_->res_mut_ptr == nullptr) {
            return nullptr;
        }
        return reinterpret_cast<T*>(api_->res_mut_ptr(iter_, term_index));
    }

    BamResult res_ticks(size_t term_index, BamChangeTicks& out_ticks) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_RES_TICKS, sizeof(BamResTicksFn)) ||
            api_->res_ticks == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->res_ticks(iter_, term_index, &out_ticks);
    }

    BamResult par_for_each_chunk(BamChunkTaskFn chunk_fn, void* user_data = nullptr) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_PAR_FOR_EACH_CHUNK, sizeof(BamParForEachChunkFn)) ||
            api_->par_for_each_chunk == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->par_for_each_chunk(iter_, chunk_fn, user_data);
    }

    BamResult par_for_each_chunk_readonly(BamChunkTaskFn chunk_fn, void* user_data = nullptr) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_PAR_FOR_EACH_CHUNK_READONLY, sizeof(BamParForEachChunkFn)) ||
            api_->par_for_each_chunk_readonly == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->par_for_each_chunk_readonly(iter_, chunk_fn, user_data);
    }

    BamResult par_for_each_chunk_ex(
        BamChunkTaskExFn chunk_fn,
        void* user_data = nullptr,
        const BamParOptions* options = nullptr
    ) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_PAR_FOR_EACH_CHUNK_EX, sizeof(BamParForEachChunkExFn)) ||
            api_->par_for_each_chunk_ex == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->par_for_each_chunk_ex(iter_, chunk_fn, user_data, options);
    }

    template <typename T>
    BamResult msg_read_slice(
        size_t term_index,
        const T*& out_ptr,
        size_t& out_count
    ) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_MSG_READ_SLICE, sizeof(BamMsgReadSliceFn)) ||
            api_->msg_read_slice == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        const uint8_t* raw_ptr = nullptr;
        BamResult res = api_->msg_read_slice(iter_, term_index, &raw_ptr, &out_count);
        out_ptr = reinterpret_cast<const T*>(raw_ptr);
        return res;
    }

    template <typename T>
    BamResult msg_read_batch(
        size_t term_index,
        T* out_buf,
        size_t max_count,
        size_t& out_read
    ) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_MSG_READ_BATCH, sizeof(BamMsgReadBatchFn)) ||
            api_->msg_read_batch == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->msg_read_batch(
            iter_,
            term_index,
            reinterpret_cast<uint8_t*>(out_buf),
            max_count,
            &out_read
        );
    }

    BamResult msg_send_raw(
        size_t term_index,
        const uint8_t* data_ptr,
        size_t data_len
    ) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_MSG_SEND, sizeof(BamMsgSendFn)) ||
            api_->msg_send == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->msg_send(iter_, term_index, data_ptr, data_len);
    }

    template <typename T>
    BamResult msg_send(size_t term_index, const T& msg) const noexcept {
        return msg_send_raw(term_index, reinterpret_cast<const uint8_t*>(&msg), sizeof(T));
    }

    template <typename T>
    BamResult msg_write_batch(
        size_t term_index,
        const T* in_buf,
        size_t count
    ) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_MSG_WRITE_BATCH, sizeof(BamMsgWriteBatchFn)) ||
            api_->msg_write_batch == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->msg_write_batch(
            iter_,
            term_index,
            reinterpret_cast<const uint8_t*>(in_buf),
            count
        );
    }

    BamResult read_removed(
        size_t term_index,
        BamEntity* out_entities,
        size_t max_count,
        size_t& out_read
    ) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_ITER_READ_REMOVED, sizeof(BamIterReadRemovedFn)) ||
            api_->iter_read_removed == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->iter_read_removed(iter_, term_index, out_entities, max_count, &out_read);
    }

    BamResult up_cascade(
        BamComponent relation,
        BamEntity entity,
        size_t term_index,
        const void*& out_ptr,
        BamEntity& out_owner
    ) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_ITER_UP_CASCADE, sizeof(BamIterUpCascadeFn)) ||
            api_->iter_up_cascade == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->iter_up_cascade(iter_, relation, entity, term_index, &out_ptr, &out_owner);
    }

    BamResult column_ticks(
        size_t term_index,
        const uint32_t*& out_added,
        const uint32_t*& out_changed,
        uint32_t& out_last_run,
        uint32_t& out_this_run
    ) const noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_ITER_COLUMN_TICKS, sizeof(BamIterColumnTicksFn)) ||
            api_->iter_column_ticks == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->iter_column_ticks(
            iter_,
            term_index,
            &out_added,
            &out_changed,
            &out_last_run,
            &out_this_run
        );
    }

private:
    const BamHostApi* api_ = nullptr;
    BamIter* iter_ = nullptr;
    size_t count_ = 0u;
};

class CommandsView {
public:
    CommandsView() noexcept = default;
    CommandsView(const BamHostApi& api, BamCommands* cmds) noexcept
        : api_(&api), cmds_(cmds) {}

    bool valid() const noexcept { return api_ != nullptr && cmds_ != nullptr; }
    BamCommands* raw() const noexcept { return cmds_; }

    BamResult spawn_empty(BamEntity& out_entity) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr || api_->cmd_spawn_empty == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_->cmd_spawn_empty(cmds_, &out_entity);
    }

    BamResult spawn(
        const BamComponent* components,
        const uint8_t* const* data_ptrs,
        size_t count,
        BamEntity& out_entity
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr || api_->cmd_spawn == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_->cmd_spawn(cmds_, components, data_ptrs, count, &out_entity);
    }

    BamResult spawn_persistent(
        const BamComponent* components,
        const uint8_t* const* data_ptrs,
        size_t count,
        BamEntity& out_entity
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_SPAWN_PERSISTENT, sizeof(BamCmdSpawnFn)) ||
            api_->cmd_spawn_persistent == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_spawn_persistent(cmds_, components, data_ptrs, count, &out_entity);
    }

    BamResult spawn_batch(
        const BamComponent* components,
        const uint8_t* const* column_ptrs,
        size_t component_count,
        size_t entity_count,
        BamEntity* out_entities
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_SPAWN_BATCH, sizeof(BamCmdSpawnBatchFn)) ||
            api_->cmd_spawn_batch == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_spawn_batch(
            cmds_,
            components,
            column_ptrs,
            component_count,
            entity_count,
            out_entities
        );
    }

    BamResult despawn(BamEntity entity) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr || api_->cmd_despawn == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_->cmd_despawn(cmds_, entity);
    }

    BamResult despawn_all(const BamTermDesc* terms, size_t term_count) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_DESPAWN_ALL, sizeof(BamCmdDespawnAllFn)) ||
            api_->cmd_despawn_all == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_despawn_all(cmds_, terms, term_count);
    }

    BamResult insert(
        BamEntity entity,
        BamComponent component,
        const uint8_t* data_ptr,
        size_t data_len
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr || api_->cmd_insert == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_->cmd_insert(cmds_, entity, component, data_ptr, data_len);
    }

    template <typename T>
    BamResult insert_pod(BamEntity entity, BamComponent component, const T& value) const noexcept {
        return insert(entity, component, reinterpret_cast<const uint8_t*>(&value), sizeof(T));
    }

    BamResult insert_default(BamEntity entity, BamComponent component) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr || api_->cmd_insert_default == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_->cmd_insert_default(cmds_, entity, component);
    }

    BamResult insert_if_new(
        BamEntity entity,
        BamComponent component,
        const uint8_t* data_ptr,
        size_t data_len
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_INSERT_IF_NEW, sizeof(BamCmdInsertFn)) ||
            api_->cmd_insert_if_new == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_insert_if_new(cmds_, entity, component, data_ptr, data_len);
    }

    BamResult remove(BamEntity entity, BamComponent component) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr || api_->cmd_remove == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_->cmd_remove(cmds_, entity, component);
    }

    BamResult trigger(
        BamComponent event,
        BamEntity target,
        const uint8_t* data_ptr,
        size_t data_len
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_TRIGGER, sizeof(BamCmdTriggerFn)) ||
            api_->cmd_trigger == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_trigger(cmds_, event, target, data_ptr, data_len);
    }

    BamResult trigger_ex(const BamTriggerDesc& desc) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_TRIGGER_EX, sizeof(BamCmdTriggerExFn)) ||
            api_->cmd_trigger_ex == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_trigger_ex(cmds_, &desc);
    }

    BamResult observe(const BamObserverDesc& desc, BamEntity& out_entity) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_OBSERVE, sizeof(BamCmdObserveFn)) ||
            api_->cmd_observe == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_observe(cmds_, &desc, &out_entity);
    }

    BamResult spawn_scene(
        const BamAssetHandle& scene_handle,
        BamEntity parent,
        BamEntity& out_entity
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_SPAWN_SCENE, sizeof(BamCmdSpawnSceneFn)) ||
            api_->cmd_spawn_scene == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_spawn_scene(cmds_, &scene_handle, parent, &out_entity);
    }

    BamResult checkpoint(size_t& out_token) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_CHECKPOINT, sizeof(BamCmdCheckpointFn)) ||
            api_->cmd_checkpoint == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_checkpoint(cmds_, &out_token);
    }

    BamResult rollback(size_t token) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_ROLLBACK, sizeof(BamCmdRollbackFn)) ||
            api_->cmd_rollback == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_rollback(cmds_, token);
    }

    BamResult insert_relation(
        BamEntity entity,
        BamComponent relation,
        BamEntity target,
        const void* data = nullptr,
        size_t data_size = 0u
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_INSERT_RELATION, sizeof(BamCmdInsertRelationFn)) ||
            api_->cmd_insert_relation == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_insert_relation(cmds_, entity, relation, target, data, data_size);
    }

    BamResult remove_relation(
        BamEntity entity,
        BamComponent relation,
        BamEntity target
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_REMOVE_RELATION, sizeof(BamCmdRemoveRelationFn)) ||
            api_->cmd_remove_relation == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_remove_relation(cmds_, entity, relation, target);
    }

    BamResult clone_entity(BamEntity src_entity, BamEntity dst_entity) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_CLONE_ENTITY, sizeof(BamCmdCloneEntityFn)) ||
            api_->cmd_clone_entity == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_clone_entity(cmds_, src_entity, dst_entity);
    }

    BamResult insert_resource(
        BamComponent resource,
        const uint8_t* data_ptr,
        size_t data_len
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_INSERT_RESOURCE, sizeof(BamCmdInsertResourceFn)) ||
            api_->cmd_insert_resource == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_insert_resource(cmds_, resource, data_ptr, data_len);
    }

    template <typename T>
    BamResult insert_resource_pod(BamComponent resource, const T& value) const noexcept {
        return insert_resource(resource, reinterpret_cast<const uint8_t*>(&value), sizeof(T));
    }

    BamResult remove_resource(BamComponent resource) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_REMOVE_RESOURCE, sizeof(BamCmdRemoveResourceFn)) ||
            api_->cmd_remove_resource == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_remove_resource(cmds_, resource);
    }

    BamResult despawn_batch(const BamEntity* entities, size_t count) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_DESPAWN_BATCH, sizeof(BamCmdDespawnBatchFn)) ||
            api_->cmd_despawn_batch == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_despawn_batch(cmds_, entities, count);
    }

    BamResult insert_batch(
        const BamEntity* entities,
        size_t entity_count,
        BamComponent component,
        const uint8_t* data_ptr,
        size_t elem_size,
        uint32_t flags = 0u
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_INSERT_BATCH, sizeof(BamCmdInsertBatchFn)) ||
            api_->cmd_insert_batch == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_insert_batch(cmds_, entities, entity_count, component, data_ptr, elem_size, flags);
    }

    BamResult remove_batch(
        const BamEntity* entities,
        size_t entity_count,
        BamComponent component
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_REMOVE_BATCH, sizeof(BamCmdRemoveBatchFn)) ||
            api_->cmd_remove_batch == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_remove_batch(cmds_, entities, entity_count, component);
    }

    BamResult spawn_batch_ex(
        const BamSpawnColumnDesc* columns,
        size_t column_count,
        size_t entity_count,
        BamEntity* out_entities
    ) const noexcept {
        if (api_ == nullptr || cmds_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_CMD_SPAWN_BATCH_EX, sizeof(BamCmdSpawnBatchExFn)) ||
            api_->cmd_spawn_batch_ex == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_->cmd_spawn_batch_ex(cmds_, columns, column_count, entity_count, out_entities);
    }

private:
    const BamHostApi* api_ = nullptr;
    BamCommands* cmds_ = nullptr;
};

class ScopedTransaction {
public:
    explicit ScopedTransaction(CommandsView cmds) noexcept
        : cmds_(cmds), token_(0u), status_(cmds.checkpoint(token_)), active_(is_ok(status_)) {}

    ScopedTransaction(const ScopedTransaction&) = delete;
    ScopedTransaction& operator=(const ScopedTransaction&) = delete;

    ScopedTransaction(ScopedTransaction&& other) noexcept
        : cmds_(other.cmds_), token_(other.token_), status_(other.status_), active_(other.active_) {
        other.active_ = false;
    }

    ~ScopedTransaction() noexcept {
        if (active_) {
            (void)cmds_.rollback(token_);
        }
    }

    bool active() const noexcept { return active_; }
    BamResult status() const noexcept { return status_; }
    size_t token() const noexcept { return token_; }
    CommandsView commands() const noexcept { return cmds_; }

    void commit() noexcept { active_ = false; }

    BamResult rollback_now() noexcept {
        if (!active_) {
            return BAM_RESULT_OK;
        }
        active_ = false;
        return cmds_.rollback(token_);
    }

private:
    CommandsView cmds_;
    size_t token_;
    BamResult status_;
    bool active_;
};

class SystemContextView {
public:
    SystemContextView(const BamHostApi& api, BamSystemCtx* ctx) noexcept
        : api_(api), ctx_(ctx) {}

    bool valid() const noexcept { return ctx_ != nullptr; }
    BamSystemCtx* raw() const noexcept { return ctx_; }

    HostContextView host_ctx() const noexcept {
        const BamHostCtx* h = (ctx_ != nullptr && api_.sys_host_ctx != nullptr)
            ? api_.sys_host_ctx(ctx_)
            : nullptr;
        return HostContextView(api_, h);
    }

    BamResult query(size_t index, BamIter*& out_iter) const noexcept {
        if (ctx_ == nullptr || api_.sys_query == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_.sys_query(ctx_, index, &out_iter);
    }

    BamResult query_view(size_t index, ChunkView& out_view) const noexcept {
        BamIter* iter = nullptr;
        BamResult res = query(index, iter);
        if (!is_ok(res)) {
            return res;
        }
        out_view = ChunkView(api_, iter, 0u);
        return BAM_RESULT_OK;
    }

    template <typename T>
    const T* entity_get(
        size_t query_index,
        BamEntity entity,
        size_t term_index
    ) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_SYS_ENTITY_GET, sizeof(BamSysEntityGetFn)) ||
            api_.sys_entity_get == nullptr) {
            return nullptr;
        }
        return reinterpret_cast<const T*>(api_.sys_entity_get(ctx_, query_index, entity, term_index));
    }

    template <typename T>
    T* entity_get_mut(
        size_t query_index,
        BamEntity entity,
        size_t term_index
    ) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_SYS_ENTITY_GET_MUT, sizeof(BamSysEntityGetMutFn)) ||
            api_.sys_entity_get_mut == nullptr) {
            return nullptr;
        }
        return reinterpret_cast<T*>(api_.sys_entity_get_mut(ctx_, query_index, entity, term_index));
    }

    template <typename T>
    const T* res_ptr(BamComponent comp, BamChangeTicks* out_ticks = nullptr) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_SYS_RES_PTR, sizeof(BamSysResPtrFn)) ||
            api_.sys_res_ptr == nullptr) {
            return nullptr;
        }
        return reinterpret_cast<const T*>(api_.sys_res_ptr(ctx_, comp, out_ticks));
    }

    template <typename T>
    T* res_mut_ptr(BamComponent comp, BamChangeTicks* out_ticks = nullptr) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_SYS_RES_MUT_PTR, sizeof(BamSysResMutPtrFn)) ||
            api_.sys_res_mut_ptr == nullptr) {
            return nullptr;
        }
        return reinterpret_cast<T*>(api_.sys_res_mut_ptr(ctx_, comp, out_ticks));
    }

    BamResult entity_children(
        BamEntity entity,
        BamComponent relationship_target,
        const BamEntity*& out_entities,
        size_t& out_len
    ) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_SYS_ENTITY_CHILDREN, sizeof(BamSysEntityChildrenFn)) ||
            api_.sys_entity_children == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.sys_entity_children(ctx_, entity, relationship_target, &out_entities, &out_len);
    }

    BamResult commands(BamCommands*& out_cmds) const noexcept {
        if (ctx_ == nullptr || api_.sys_commands == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        return api_.sys_commands(ctx_, &out_cmds);
    }

    BamResult commands_view(CommandsView& out_view) const noexcept {
        BamCommands* cmds = nullptr;
        BamResult res = commands(cmds);
        if (!is_ok(res)) {
            return res;
        }
        out_view = CommandsView(api_, cmds);
        return BAM_RESULT_OK;
    }

    BamResult current_ticks(uint32_t& out_last_run, uint32_t& out_this_run) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_SYS_CURRENT_TICKS, sizeof(BamSysCurrentTicksFn)) ||
            api_.sys_current_ticks == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.sys_current_ticks(ctx_, &out_last_run, &out_this_run);
    }

    BamResult time_info(BamTimeInfo& out_info) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_SYS_TIME_INFO, sizeof(BamSysTimeInfoFn)) ||
            api_.sys_time_info == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        out_info = bam_time_info();
        return api_.sys_time_info(ctx_, &out_info);
    }

    BamResult relation_targets(
        BamEntity entity,
        BamComponent relation,
        const BamEntity*& out_targets,
        size_t& out_len
    ) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_SYS_RELATION_TARGETS, sizeof(BamSysRelationTargetsFn)) ||
            api_.sys_relation_targets == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.sys_relation_targets(ctx_, entity, relation, &out_targets, &out_len);
    }

    BamResult relation_sources(
        BamEntity target,
        BamComponent relation,
        const BamEntity*& out_sources,
        size_t& out_len
    ) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_SYS_RELATION_SOURCES, sizeof(BamSysRelationSourcesFn)) ||
            api_.sys_relation_sources == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.sys_relation_sources(ctx_, target, relation, &out_sources, &out_len);
    }

    BamResult relation_edge_data(
        BamEntity entity,
        BamComponent relation,
        BamEntity target,
        const void*& out_ptr,
        size_t& out_size
    ) const noexcept {
        if (ctx_ == nullptr ||
            !bam_host_api_covers_slot(&api_, BAM_HOST_API_OFFSET_SYS_RELATION_EDGE_DATA, sizeof(BamSysRelationEdgeDataFn)) ||
            api_.sys_relation_edge_data == nullptr) {
            return BAM_RESULT_UNSUPPORTED;
        }
        return api_.sys_relation_edge_data(ctx_, entity, relation, target, &out_ptr, &out_size);
    }

private:
    const BamHostApi& api_;
    BamSystemCtx* ctx_;
};

} // namespace bam

#endif // BAMABI_HPP
"#;

const ZIG_SDK_BODY: &str = r#"
pub const BamAccessMode = c.BamAccessMode;
pub const BamAssetGetLoadStateFn = c.BamAssetGetLoadStateFn;
pub const BamAssetHandle = c.BamAssetHandle;
pub const BamAssetLoadFn = c.BamAssetLoadFn;
pub const BamAssetLoadState = c.BamAssetLoadState;
pub const BamAssetReleaseFn = c.BamAssetReleaseFn;
pub const BamBindNamedObjectFn = c.BamBindNamedObjectFn;
pub const BamBool = c.BamBool;
pub const BamByteSlice = c.BamByteSlice;
pub const BamChangeTicks = c.BamChangeTicks;
pub const BamChildOf = c.BamChildOf;
pub const BamChunkTaskExFn = c.BamChunkTaskExFn;
pub const BamChunkTaskFn = c.BamChunkTaskFn;
pub const BamCmdCheckpointFn = c.BamCmdCheckpointFn;
pub const BamCmdCloneEntityFn = c.BamCmdCloneEntityFn;
pub const BamCmdDespawnAllFn = c.BamCmdDespawnAllFn;
pub const BamCmdDespawnBatchFn = c.BamCmdDespawnBatchFn;
pub const BamCmdDespawnFn = c.BamCmdDespawnFn;
pub const BamCmdInsertBatchFn = c.BamCmdInsertBatchFn;
pub const BamCmdInsertDefaultFn = c.BamCmdInsertDefaultFn;
pub const BamCmdInsertFn = c.BamCmdInsertFn;
pub const BamCmdInsertRelationFn = c.BamCmdInsertRelationFn;
pub const BamCmdInsertResourceFn = c.BamCmdInsertResourceFn;
pub const BamCmdObserveFn = c.BamCmdObserveFn;
pub const BamCmdRemoveBatchFn = c.BamCmdRemoveBatchFn;
pub const BamCmdRemoveFn = c.BamCmdRemoveFn;
pub const BamCmdRemoveRelationFn = c.BamCmdRemoveRelationFn;
pub const BamCmdRemoveResourceFn = c.BamCmdRemoveResourceFn;
pub const BamCmdRollbackFn = c.BamCmdRollbackFn;
pub const BamCmdSpawnBatchExFn = c.BamCmdSpawnBatchExFn;
pub const BamCmdSpawnBatchFn = c.BamCmdSpawnBatchFn;
pub const BamCmdSpawnEmptyFn = c.BamCmdSpawnEmptyFn;
pub const BamCmdSpawnFn = c.BamCmdSpawnFn;
pub const BamCmdSpawnSceneFn = c.BamCmdSpawnSceneFn;
pub const BamCmdTriggerExFn = c.BamCmdTriggerExFn;
pub const BamCmdTriggerFn = c.BamCmdTriggerFn;
pub const BamColumnInfo = c.BamColumnInfo;
pub const BamCommands = c.BamCommands;
pub const BamComponent = c.BamComponent;
pub const BamComponentCloneFn = c.BamComponentCloneFn;
pub const BamComponentDesc = c.BamComponentDesc;
pub const BamComponentDropFn = c.BamComponentDropFn;
pub const BamComponentHookFn = c.BamComponentHookFn;
pub const BamComponentLayoutInfoFn = c.BamComponentLayoutInfoFn;
pub const BamComponentUuid = c.BamComponentUuid;
pub const BamConfigureScheduleSetFn = c.BamConfigureScheduleSetFn;
pub const BamEntity = c.BamEntity;
pub const BamEventKind = c.BamEventKind;
pub const BamFieldDesc = c.BamFieldDesc;
pub const BamFieldKind = c.BamFieldKind;
pub const BamGetProcAddressFn = c.BamGetProcAddressFn;
pub const BamHostApi = c.BamHostApi;
pub const BamHostCtx = c.BamHostCtx;
pub const BamInitFn = c.BamInitFn;
pub const BamInstance = c.BamInstance;
pub const BamInternSymbolFn = c.BamInternSymbolFn;
pub const BamIter = c.BamIter;
pub const BamIterColumnInfoFn = c.BamIterColumnInfoFn;
pub const BamIterColumnMutableFn = c.BamIterColumnMutableFn;
pub const BamIterColumnMutableUntrackedFn = c.BamIterColumnMutableUntrackedFn;
pub const BamIterColumnReadonlyFn = c.BamIterColumnReadonlyFn;
pub const BamIterColumnRowsMutableFn = c.BamIterColumnRowsMutableFn;
pub const BamIterColumnRowsMutableUntrackedFn = c.BamIterColumnRowsMutableUntrackedFn;
pub const BamIterColumnRowsReadonlyFn = c.BamIterColumnRowsReadonlyFn;
pub const BamIterColumnTicksFn = c.BamIterColumnTicksFn;
pub const BamIterCountFn = c.BamIterCountFn;
pub const BamIterEntitiesFn = c.BamIterEntitiesFn;
pub const BamIterMarkChangedRangeFn = c.BamIterMarkChangedRangeFn;
pub const BamIterMarkChangedRangesFn = c.BamIterMarkChangedRangesFn;
pub const BamIterNextChunkBatchFn = c.BamIterNextChunkBatchFn;
pub const BamIterNextChunkFn = c.BamIterNextChunkFn;
pub const BamIterReadRemovedFn = c.BamIterReadRemovedFn;
pub const BamIterResetFn = c.BamIterResetFn;
pub const BamIterUpCascadeFn = c.BamIterUpCascadeFn;
pub const BamLastErrorFn = c.BamLastErrorFn;
pub const BamLayoutInfo = c.BamLayoutInfo;
pub const BamLoadCtx = c.BamLoadCtx;
pub const BamLoadHostCtxFn = c.BamLoadHostCtxFn;
pub const BamLogDiagnosticFn = c.BamLogDiagnosticFn;
pub const BamManifest = c.BamManifest;
pub const BamManifestFn = c.BamManifestFn;
pub const BamMappedStringGetFn = c.BamMappedStringGetFn;
pub const BamMappedStringSetFn = c.BamMappedStringSetFn;
pub const BamMappedStringVTable = c.BamMappedStringVTable;
pub const BamMigrateComponentColumnFn = c.BamMigrateComponentColumnFn;
pub const BamMountSystemInScheduleFn = c.BamMountSystemInScheduleFn;
pub const BamMsgReadBatchFn = c.BamMsgReadBatchFn;
pub const BamMsgReadSliceFn = c.BamMsgReadSliceFn;
pub const BamMsgSendFn = c.BamMsgSendFn;
pub const BamMsgWriteBatchFn = c.BamMsgWriteBatchFn;
pub const BamObserverCtxFn = c.BamObserverCtxFn;
pub const BamObserverCtxMutFn = c.BamObserverCtxMutFn;
pub const BamObserverDesc = c.BamObserverDesc;
pub const BamObserverFn = c.BamObserverFn;
pub const BamObserverMutFn = c.BamObserverMutFn;
pub const BamParForEachChunkExFn = c.BamParForEachChunkExFn;
pub const BamParForEachChunkFn = c.BamParForEachChunkFn;
pub const BamParOptions = c.BamParOptions;
pub const BamQueryDesc = c.BamQueryDesc;
pub const BamRegisterComponentFn = c.BamRegisterComponentFn;
pub const BamRegisterObserverFn = c.BamRegisterObserverFn;
pub const BamRegisterRelationFn = c.BamRegisterRelationFn;
pub const BamRegisterSystemFn = c.BamRegisterSystemFn;
pub const BamRegisterSystemHandleFn = c.BamRegisterSystemHandleFn;
pub const BamRelationDesc = c.BamRelationDesc;
pub const BamReportGuestFaultFn = c.BamReportGuestFaultFn;
pub const BamRequiredComponent = c.BamRequiredComponent;
pub const BamRequiredInitFn = c.BamRequiredInitFn;
pub const BamResMutPtrFn = c.BamResMutPtrFn;
pub const BamResPtrFn = c.BamResPtrFn;
pub const BamResTicksFn = c.BamResTicksFn;
pub const BamResolveComponentByUuidFn = c.BamResolveComponentByUuidFn;
pub const BamResolveComponentFn = c.BamResolveComponentFn;
pub const BamResolveNamedObjectFn = c.BamResolveNamedObjectFn;
pub const BamResolveSymbolStrFn = c.BamResolveSymbolStrFn;
pub const BamRestoreStateFn = c.BamRestoreStateFn;
pub const BamResult = c.BamResult;
pub const BamRowMigrateFn = c.BamRowMigrateFn;
pub const BamRowRange = c.BamRowRange;
pub const BamRunIfFn = c.BamRunIfFn;
pub const BamSaveStateFn = c.BamSaveStateFn;
pub const BamScheduleId = c.BamScheduleId;
pub const BamScheduleSetDesc = c.BamScheduleSetDesc;
pub const BamShutdownFn = c.BamShutdownFn;
pub const BamSipHasher128 = c.BamSipHasher128;
pub const BamSpawnColumnDesc = c.BamSpawnColumnDesc;
pub const BamStateWriterFn = c.BamStateWriterFn;
pub const BamStrView = c.BamStrView;
pub const BamSymbol = c.BamSymbol;
pub const BamSysCommandsFn = c.BamSysCommandsFn;
pub const BamSysCurrentTicksFn = c.BamSysCurrentTicksFn;
pub const BamSysEntityChildrenFn = c.BamSysEntityChildrenFn;
pub const BamSysEntityGetFn = c.BamSysEntityGetFn;
pub const BamSysEntityGetMutFn = c.BamSysEntityGetMutFn;
pub const BamSysHostCtxFn = c.BamSysHostCtxFn;
pub const BamSysQueryFn = c.BamSysQueryFn;
pub const BamSysRelationEdgeDataFn = c.BamSysRelationEdgeDataFn;
pub const BamSysRelationSourcesFn = c.BamSysRelationSourcesFn;
pub const BamSysRelationTargetsFn = c.BamSysRelationTargetsFn;
pub const BamSysResMutPtrFn = c.BamSysResMutPtrFn;
pub const BamSysResPtrFn = c.BamSysResPtrFn;
pub const BamSysTimeInfoFn = c.BamSysTimeInfoFn;
pub const BamSystem = c.BamSystem;
pub const BamSystemCtx = c.BamSystemCtx;
pub const BamSystemDesc = c.BamSystemDesc;
pub const BamSystemFn = c.BamSystemFn;
pub const BamSystemSet = c.BamSystemSet;
pub const BamSystemTelemetryDigest64 = c.BamSystemTelemetryDigest64;
pub const BamTermDesc = c.BamTermDesc;
pub const BamTimeInfo = c.BamTimeInfo;
pub const BamTriggerDesc = c.BamTriggerDesc;
pub const BamVTableHeader = c.BamVTableHeader;

pub const BAM_ABI_VERSION = c.BAM_ABI_VERSION;
pub const BAM_ABI_VERSION_MAJOR = c.BAM_ABI_VERSION_MAJOR;
pub const BAM_ABI_VERSION_MINOR = c.BAM_ABI_VERSION_MINOR;
pub const BAM_ABI_VERSION_PATCH = c.BAM_ABI_VERSION_PATCH;
pub const BAM_ACCESS_ADDED = c.BAM_ACCESS_ADDED;
pub const BAM_ACCESS_ALLOW = c.BAM_ACCESS_ALLOW;
pub const BAM_ACCESS_CHANGED = c.BAM_ACCESS_CHANGED;
pub const BAM_ACCESS_FILTER_ONLY = c.BAM_ACCESS_FILTER_ONLY;
pub const BAM_ACCESS_HAS = c.BAM_ACCESS_HAS;
pub const BAM_ACCESS_MESSAGE_READ = c.BAM_ACCESS_MESSAGE_READ;
pub const BAM_ACCESS_MESSAGE_WRITE = c.BAM_ACCESS_MESSAGE_WRITE;
pub const BAM_ACCESS_OPTIONAL_READ = c.BAM_ACCESS_OPTIONAL_READ;
pub const BAM_ACCESS_OPTIONAL_WRITE = c.BAM_ACCESS_OPTIONAL_WRITE;
pub const BAM_ACCESS_READ = c.BAM_ACCESS_READ;
pub const BAM_ACCESS_REMOVED_READ = c.BAM_ACCESS_REMOVED_READ;
pub const BAM_ACCESS_RESOURCE_READ = c.BAM_ACCESS_RESOURCE_READ;
pub const BAM_ACCESS_RESOURCE_WRITE = c.BAM_ACCESS_RESOURCE_WRITE;
pub const BAM_ACCESS_WITHOUT = c.BAM_ACCESS_WITHOUT;
pub const BAM_ACCESS_WRITE = c.BAM_ACCESS_WRITE;
pub const BAM_ASSET_HANDLE_KIND_INDEX = c.BAM_ASSET_HANDLE_KIND_INDEX;
pub const BAM_ASSET_HANDLE_KIND_NONE = c.BAM_ASSET_HANDLE_KIND_NONE;
pub const BAM_ASSET_HANDLE_KIND_UUID = c.BAM_ASSET_HANDLE_KIND_UUID;
pub const BAM_ASSET_LOAD_STATE_FAILED = c.BAM_ASSET_LOAD_STATE_FAILED;
pub const BAM_ASSET_LOAD_STATE_LOADED = c.BAM_ASSET_LOAD_STATE_LOADED;
pub const BAM_ASSET_LOAD_STATE_LOADING = c.BAM_ASSET_LOAD_STATE_LOADING;
pub const BAM_ASSET_LOAD_STATE_NOT_LOADED = c.BAM_ASSET_LOAD_STATE_NOT_LOADED;
pub const BAM_CHILDREN_NAME = c.BAM_CHILDREN_NAME;
pub const BAM_CHILD_OF_NAME = c.BAM_CHILD_OF_NAME;
pub const BAM_COMPONENT_DESC_MIN_V0_2_SIZE = c.BAM_COMPONENT_DESC_MIN_V0_2_SIZE;
pub const BAM_COMPONENT_DESC_MIN_V0_3_SIZE = c.BAM_COMPONENT_DESC_MIN_V0_3_SIZE;
pub const BAM_COMPONENT_FLAG_IMMUTABLE = c.BAM_COMPONENT_FLAG_IMMUTABLE;
pub const BAM_COMPONENT_FLAG_SPARSE_SET = c.BAM_COMPONENT_FLAG_SPARSE_SET;
pub const BAM_COMPONENT_FLAG_SUMMARY_TICK = c.BAM_COMPONENT_FLAG_SUMMARY_TICK;
pub const BAM_COMPONENT_INVALID = c.BAM_COMPONENT_INVALID;
pub const BAM_COMPONENT_KNOWN_FLAGS = c.BAM_COMPONENT_KNOWN_FLAGS;
pub const BAM_DISABLED_NAME = c.BAM_DISABLED_NAME;
pub const BAM_ENTITY_NULL = c.BAM_ENTITY_NULL;
pub const BAM_EVENT_KIND_ADD = c.BAM_EVENT_KIND_ADD;
pub const BAM_EVENT_KIND_CUSTOM = c.BAM_EVENT_KIND_CUSTOM;
pub const BAM_EVENT_KIND_DESPAWN = c.BAM_EVENT_KIND_DESPAWN;
pub const BAM_EVENT_KIND_DISCARD = c.BAM_EVENT_KIND_DISCARD;
pub const BAM_EVENT_KIND_INSERT = c.BAM_EVENT_KIND_INSERT;
pub const BAM_EVENT_KIND_READY = c.BAM_EVENT_KIND_READY;
pub const BAM_EVENT_KIND_REMOVE = c.BAM_EVENT_KIND_REMOVE;
pub const BAM_FALSE = c.BAM_FALSE;
pub const BAM_FIELD_DESC_MIN_V0_3_SIZE = c.BAM_FIELD_DESC_MIN_V0_3_SIZE;
pub const BAM_FIELD_KIND_BOOL = c.BAM_FIELD_KIND_BOOL;
pub const BAM_FIELD_KIND_F32 = c.BAM_FIELD_KIND_F32;
pub const BAM_FIELD_KIND_F64 = c.BAM_FIELD_KIND_F64;
pub const BAM_FIELD_KIND_FFI_OPTION = c.BAM_FIELD_KIND_FFI_OPTION;
pub const BAM_FIELD_KIND_I16 = c.BAM_FIELD_KIND_I16;
pub const BAM_FIELD_KIND_I32 = c.BAM_FIELD_KIND_I32;
pub const BAM_FIELD_KIND_I64 = c.BAM_FIELD_KIND_I64;
pub const BAM_FIELD_KIND_I8 = c.BAM_FIELD_KIND_I8;
pub const BAM_FIELD_KIND_ISIZE = c.BAM_FIELD_KIND_ISIZE;
pub const BAM_FIELD_KIND_NESTED = c.BAM_FIELD_KIND_NESTED;
pub const BAM_FIELD_KIND_OPAQUE = c.BAM_FIELD_KIND_OPAQUE;
pub const BAM_FIELD_KIND_STRING = c.BAM_FIELD_KIND_STRING;
pub const BAM_FIELD_KIND_U16 = c.BAM_FIELD_KIND_U16;
pub const BAM_FIELD_KIND_U32 = c.BAM_FIELD_KIND_U32;
pub const BAM_FIELD_KIND_U64 = c.BAM_FIELD_KIND_U64;
pub const BAM_FIELD_KIND_U8 = c.BAM_FIELD_KIND_U8;
pub const BAM_FIELD_KIND_UNKNOWN = c.BAM_FIELD_KIND_UNKNOWN;
pub const BAM_FIELD_KIND_USIZE = c.BAM_FIELD_KIND_USIZE;
pub const BAM_FIELD_KIND_VEC = c.BAM_FIELD_KIND_VEC;
pub const BAM_FNV1A_64_OFFSET_BASIS = c.BAM_FNV1A_64_OFFSET_BASIS;
pub const BAM_FNV1A_64_PRIME = c.BAM_FNV1A_64_PRIME;
pub const BAM_HOST_API_MIN_V0_2_SIZE = c.BAM_HOST_API_MIN_V0_2_SIZE;
pub const BAM_HOST_API_MIN_V0_3_SIZE = c.BAM_HOST_API_MIN_V0_3_SIZE;
pub const BAM_HOST_API_MIN_V0_4_SIZE = c.BAM_HOST_API_MIN_V0_4_SIZE;
pub const BAM_HOST_API_SLOT_COUNT = c.BAM_HOST_API_SLOT_COUNT;
pub const BAM_LAYOUT_INFO_MIN_V0_3_SIZE = c.BAM_LAYOUT_INFO_MIN_V0_3_SIZE;
pub const BAM_LOG_DEBUG = c.BAM_LOG_DEBUG;
pub const BAM_LOG_ERROR = c.BAM_LOG_ERROR;
pub const BAM_LOG_INFO = c.BAM_LOG_INFO;
pub const BAM_LOG_TRACE = c.BAM_LOG_TRACE;
pub const BAM_LOG_WARN = c.BAM_LOG_WARN;
pub const BAM_MANIFEST_KNOWN_FLAGS = c.BAM_MANIFEST_KNOWN_FLAGS;
pub const BAM_MANIFEST_MIN_V0_2_SIZE = c.BAM_MANIFEST_MIN_V0_2_SIZE;
pub const BAM_MAX_CHANGE_AGE = c.BAM_MAX_CHANGE_AGE;
pub const BAM_MAX_QUERIES_PER_SYSTEM = c.BAM_MAX_QUERIES_PER_SYSTEM;
pub const BAM_MAX_TERMS_PER_QUERY = c.BAM_MAX_TERMS_PER_QUERY;
pub const BAM_OBSERVER_DESC_MIN_V0_2_SIZE = c.BAM_OBSERVER_DESC_MIN_V0_2_SIZE;
pub const BAM_OBSERVER_DESC_MIN_V0_4_SIZE = c.BAM_OBSERVER_DESC_MIN_V0_4_SIZE;
pub const BAM_OBSERVER_FLAG_PROPAGATE = c.BAM_OBSERVER_FLAG_PROPAGATE;
pub const BAM_OBSERVER_KNOWN_FLAGS = c.BAM_OBSERVER_KNOWN_FLAGS;
pub const BAM_PAR_OPTIONS_MIN_V0_4_SIZE = c.BAM_PAR_OPTIONS_MIN_V0_4_SIZE;
pub const BAM_QUERY_DESC_MIN_V0_2_SIZE = c.BAM_QUERY_DESC_MIN_V0_2_SIZE;
pub const BAM_QUERY_FLAG_PARAM_SET = c.BAM_QUERY_FLAG_PARAM_SET;
pub const BAM_QUERY_KNOWN_FLAGS = c.BAM_QUERY_KNOWN_FLAGS;
pub const BAM_RELATION_DESC_MIN_V0_3_SIZE = c.BAM_RELATION_DESC_MIN_V0_3_SIZE;
pub const BAM_RELATION_FLAG_ACYCLIC = c.BAM_RELATION_FLAG_ACYCLIC;
pub const BAM_RELATION_FLAG_CASCADE_DELETE = c.BAM_RELATION_FLAG_CASCADE_DELETE;
pub const BAM_RELATION_FLAG_CASCADE_DESPAWN = c.BAM_RELATION_FLAG_CASCADE_DESPAWN;
pub const BAM_RELATION_FLAG_CLEANUP_REMOVE = c.BAM_RELATION_FLAG_CLEANUP_REMOVE;
pub const BAM_RELATION_FLAG_EXCLUSIVE = c.BAM_RELATION_FLAG_EXCLUSIVE;
pub const BAM_RELATION_FLAG_SYMMETRIC = c.BAM_RELATION_FLAG_SYMMETRIC;
pub const BAM_RELATION_KNOWN_FLAGS = c.BAM_RELATION_KNOWN_FLAGS;
pub const BAM_REQUIRED_COMPONENT_MIN_V0_3_SIZE = c.BAM_REQUIRED_COMPONENT_MIN_V0_3_SIZE;
pub const BAM_REQUIRED_FLAG_CUSTOM_BYTES = c.BAM_REQUIRED_FLAG_CUSTOM_BYTES;
pub const BAM_REQUIRED_FLAG_INIT_FN = c.BAM_REQUIRED_FLAG_INIT_FN;
pub const BAM_REQUIRED_FLAG_RESOLVE_BY_UUID = c.BAM_REQUIRED_FLAG_RESOLVE_BY_UUID;
pub const BAM_REQUIRED_KNOWN_FLAGS = c.BAM_REQUIRED_KNOWN_FLAGS;
pub const BAM_RESULT_ACCESS_CONFLICT = c.BAM_RESULT_ACCESS_CONFLICT;
pub const BAM_RESULT_CAPACITY_EXCEEDED = c.BAM_RESULT_CAPACITY_EXCEEDED;
pub const BAM_RESULT_CYCLE_DETECTED = c.BAM_RESULT_CYCLE_DETECTED;
pub const BAM_RESULT_INTERNAL_ERROR = c.BAM_RESULT_INTERNAL_ERROR;
pub const BAM_RESULT_INVALID_ARGUMENT = c.BAM_RESULT_INVALID_ARGUMENT;
pub const BAM_RESULT_INVALID_COMPONENT = c.BAM_RESULT_INVALID_COMPONENT;
pub const BAM_RESULT_INVALID_ENTITY = c.BAM_RESULT_INVALID_ENTITY;
pub const BAM_RESULT_INVALID_STATE = c.BAM_RESULT_INVALID_STATE;
pub const BAM_RESULT_INVALID_UTF8 = c.BAM_RESULT_INVALID_UTF8;
pub const BAM_RESULT_INVALID_VERSION = c.BAM_RESULT_INVALID_VERSION;
pub const BAM_RESULT_NOT_FOUND = c.BAM_RESULT_NOT_FOUND;
pub const BAM_RESULT_NULL_POINTER = c.BAM_RESULT_NULL_POINTER;
pub const BAM_RESULT_OK = c.BAM_RESULT_OK;
pub const BAM_RESULT_OUT_OF_RANGE = c.BAM_RESULT_OUT_OF_RANGE;
pub const BAM_RESULT_PANIC_CAUGHT = c.BAM_RESULT_PANIC_CAUGHT;
pub const BAM_RESULT_PERMISSION_DENIED = c.BAM_RESULT_PERMISSION_DENIED;
pub const BAM_RESULT_TYPE_MISMATCH = c.BAM_RESULT_TYPE_MISMATCH;
pub const BAM_RESULT_UNSUPPORTED = c.BAM_RESULT_UNSUPPORTED;
pub const BAM_SCHEDULE_CUSTOM_BASE = c.BAM_SCHEDULE_CUSTOM_BASE;
pub const BAM_SCHEDULE_FIRST = c.BAM_SCHEDULE_FIRST;
pub const BAM_SCHEDULE_FIXED_FIRST = c.BAM_SCHEDULE_FIXED_FIRST;
pub const BAM_SCHEDULE_FIXED_LAST = c.BAM_SCHEDULE_FIXED_LAST;
pub const BAM_SCHEDULE_FIXED_POST_UPDATE = c.BAM_SCHEDULE_FIXED_POST_UPDATE;
pub const BAM_SCHEDULE_FIXED_PRE_UPDATE = c.BAM_SCHEDULE_FIXED_PRE_UPDATE;
pub const BAM_SCHEDULE_FIXED_UPDATE = c.BAM_SCHEDULE_FIXED_UPDATE;
pub const BAM_SCHEDULE_LAST = c.BAM_SCHEDULE_LAST;
pub const BAM_SCHEDULE_POST_STARTUP = c.BAM_SCHEDULE_POST_STARTUP;
pub const BAM_SCHEDULE_POST_UPDATE = c.BAM_SCHEDULE_POST_UPDATE;
pub const BAM_SCHEDULE_PRE_STARTUP = c.BAM_SCHEDULE_PRE_STARTUP;
pub const BAM_SCHEDULE_PRE_UPDATE = c.BAM_SCHEDULE_PRE_UPDATE;
pub const BAM_SCHEDULE_RUN_FIXED_MAIN_LOOP = c.BAM_SCHEDULE_RUN_FIXED_MAIN_LOOP;
pub const BAM_SCHEDULE_SET_DESC_MIN_V0_3_SIZE = c.BAM_SCHEDULE_SET_DESC_MIN_V0_3_SIZE;
pub const BAM_SCHEDULE_SET_DESC_MIN_V0_4_SIZE = c.BAM_SCHEDULE_SET_DESC_MIN_V0_4_SIZE;
pub const BAM_SCHEDULE_SET_FLAG_AFTER_WEAK = c.BAM_SCHEDULE_SET_FLAG_AFTER_WEAK;
pub const BAM_SCHEDULE_SET_FLAG_BEFORE_WEAK = c.BAM_SCHEDULE_SET_FLAG_BEFORE_WEAK;
pub const BAM_SCHEDULE_SET_FLAG_PARALLEL = c.BAM_SCHEDULE_SET_FLAG_PARALLEL;
pub const BAM_SCHEDULE_SET_FLAG_SEQUENCE = c.BAM_SCHEDULE_SET_FLAG_SEQUENCE;
pub const BAM_SCHEDULE_SET_FLAG_SEQUENCE_WEAK = c.BAM_SCHEDULE_SET_FLAG_SEQUENCE_WEAK;
pub const BAM_SCHEDULE_SET_KNOWN_FLAGS = c.BAM_SCHEDULE_SET_KNOWN_FLAGS;
pub const BAM_SCHEDULE_SPAWN_SCENE = c.BAM_SCHEDULE_SPAWN_SCENE;
pub const BAM_SCHEDULE_STARTUP = c.BAM_SCHEDULE_STARTUP;
pub const BAM_SCHEDULE_STATE_TRANSITION = c.BAM_SCHEDULE_STATE_TRANSITION;
pub const BAM_SCHEDULE_UPDATE = c.BAM_SCHEDULE_UPDATE;
pub const BAM_SIPHASH_KEY_0 = c.BAM_SIPHASH_KEY_0;
pub const BAM_SIPHASH_KEY_1 = c.BAM_SIPHASH_KEY_1;
pub const BAM_SPAWN_COL_FLAG_SPLAT = c.BAM_SPAWN_COL_FLAG_SPLAT;
pub const BAM_SPAWN_COL_KNOWN_FLAGS = c.BAM_SPAWN_COL_KNOWN_FLAGS;
pub const BAM_STORAGE_FLAG_ENTITY_EVENT = c.BAM_STORAGE_FLAG_ENTITY_EVENT;
pub const BAM_STORAGE_FLAG_EVENT = c.BAM_STORAGE_FLAG_EVENT;
pub const BAM_STORAGE_FLAG_IMMUTABLE = c.BAM_STORAGE_FLAG_IMMUTABLE;
pub const BAM_STORAGE_FLAG_MESSAGE = c.BAM_STORAGE_FLAG_MESSAGE;
pub const BAM_STORAGE_FLAG_RESOURCE = c.BAM_STORAGE_FLAG_RESOURCE;
pub const BAM_STORAGE_FLAG_SPARSE_SET = c.BAM_STORAGE_FLAG_SPARSE_SET;
pub const BAM_STORAGE_FLAG_SUMMARY_TICK = c.BAM_STORAGE_FLAG_SUMMARY_TICK;
pub const BAM_SYMBOL_INVALID = c.BAM_SYMBOL_INVALID;
pub const BAM_SYSTEM_DESC_MIN_V0_2_SIZE = c.BAM_SYSTEM_DESC_MIN_V0_2_SIZE;
pub const BAM_SYSTEM_FLAG_AMBIGUOUS_WITH_ALL = c.BAM_SYSTEM_FLAG_AMBIGUOUS_WITH_ALL;
pub const BAM_SYSTEM_FLAG_MAIN_THREAD = c.BAM_SYSTEM_FLAG_MAIN_THREAD;
pub const BAM_SYSTEM_INVALID = c.BAM_SYSTEM_INVALID;
pub const BAM_SYSTEM_KNOWN_FLAGS = c.BAM_SYSTEM_KNOWN_FLAGS;
pub const BAM_SYSTEM_SET_INVALID = c.BAM_SYSTEM_SET_INVALID;
pub const BAM_SYSTEM_TELEMETRY_DIGEST_SIZE = c.BAM_SYSTEM_TELEMETRY_DIGEST_SIZE;
pub const BAM_SYSTEM_TELEMETRY_DIGEST_VERSION = c.BAM_SYSTEM_TELEMETRY_DIGEST_VERSION;
pub const BAM_SYSTEM_TELEMETRY_MAGIC_0 = c.BAM_SYSTEM_TELEMETRY_MAGIC_0;
pub const BAM_SYSTEM_TELEMETRY_MAGIC_1 = c.BAM_SYSTEM_TELEMETRY_MAGIC_1;
pub const BAM_SYSTEM_TELEMETRY_MAGIC_2 = c.BAM_SYSTEM_TELEMETRY_MAGIC_2;
pub const BAM_SYSTEM_TELEMETRY_MAGIC_3 = c.BAM_SYSTEM_TELEMETRY_MAGIC_3;
pub const BAM_TELEMETRY_FLAG_BUDGET_BREACHED = c.BAM_TELEMETRY_FLAG_BUDGET_BREACHED;
pub const BAM_TELEMETRY_FLAG_FAULTED = c.BAM_TELEMETRY_FLAG_FAULTED;
pub const BAM_TELEMETRY_FLAG_QUARANTINED = c.BAM_TELEMETRY_FLAG_QUARANTINED;
pub const BAM_TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE = c.BAM_TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE;
pub const BAM_TERM_FLAG_ADDED = c.BAM_TERM_FLAG_ADDED;
pub const BAM_TERM_FLAG_CHANGED = c.BAM_TERM_FLAG_CHANGED;
pub const BAM_TERM_KNOWN_FLAGS = c.BAM_TERM_KNOWN_FLAGS;
pub const BAM_TIME_INFO_MIN_V0_4_SIZE = c.BAM_TIME_INFO_MIN_V0_4_SIZE;
pub const BAM_TRIGGER_DESC_MIN_V0_2_SIZE = c.BAM_TRIGGER_DESC_MIN_V0_2_SIZE;
pub const BAM_TRIGGER_FLAG_PROPAGATE = c.BAM_TRIGGER_FLAG_PROPAGATE;
pub const BAM_TRIGGER_KNOWN_FLAGS = c.BAM_TRIGGER_KNOWN_FLAGS;
pub const BAM_TRUE = c.BAM_TRUE;
pub const BAM_VTABLE_HEADER_MIN_V0_2_SIZE = c.BAM_VTABLE_HEADER_MIN_V0_2_SIZE;

pub const bam_version_is_compatible = c.bam_version_is_compatible;
pub const SystemTelemetryDigest64 = c.BamSystemTelemetryDigest64;

pub inline fn strView(slice: []const u8) c.BamStrView {
    return .{ .ptr = slice.ptr, .len = slice.len };
}

pub inline fn fnv1a64Init() u64 {
    return c.bam_fnv1a_64_init();
}

pub inline fn fnv1a64UpdateBytes(hash: u64, bytes: []const u8) u64 {
    return c.bam_fnv1a_64_update_bytes(hash, bytes.ptr, bytes.len);
}

pub inline fn fnv1a64UpdateU64(hash: u64, value: u64) u64 {
    return c.bam_fnv1a_64_update_u64(hash, value);
}

pub inline fn fnv1a64Str(s: []const u8) u64 {
    return fnv1a64UpdateBytes(fnv1a64Init(), s);
}

pub inline fn fnv1a64HashSystem(mod_name: []const u8, system_name: []const u8) u64 {
    var h = fnv1a64UpdateBytes(fnv1a64Init(), mod_name);
    h = fnv1a64UpdateBytes(h, "::");
    return fnv1a64UpdateBytes(h, system_name);
}

pub inline fn hashFieldSchema(
    prev_hash: u64,
    field_name: [*:0]const u8,
    field_type_tag: [*:0]const u8,
    field_offset: u64,
) u64 {
    return c.bam_hash_field_schema(prev_hash, field_name, field_type_tag, field_offset);
}

pub const HostContextView = struct {
    api: *const c.BamHostApi,
    ctx: *const c.BamHostCtx,

    pub fn getProcAddress(self: HostContextView, name: [*:0]const u8) ?*const anyopaque {
        const f = self.api.get_proc_address orelse return null;
        return f(self.ctx, name);
    }

    pub fn lastError(self: HostContextView, buf: []u8) usize {
        const f = self.api.last_error orelse return 0;
        return f(self.ctx, @ptrCast(buf.ptr), buf.len);
    }

    pub fn resolveComponent(
        self: HostContextView,
        name: ?[*:0]const u8,
        uuid: ?*const c.BamComponentUuid,
    ) BamError!c.BamComponent {
        const f = self.api.resolve_component orelse return error.NullPointer;
        var out: c.BamComponent = c.BAM_COMPONENT_INVALID;
        try checkResult(f(self.ctx, name, uuid, &out));
        return out;
    }

    pub fn assetLoad(
        self: HostContextView,
        asset_type: c.BamComponent,
        path: []const u8,
    ) BamError!c.BamAssetHandle {
        if (!coversHostApiSlot(self.api, offsets.ASSET_LOAD, @sizeOf(c.BamAssetLoadFn))) {
            return error.Unsupported;
        }
        const f = self.api.asset_load orelse return error.Unsupported;
        const sv = strView(path);
        var out: c.BamAssetHandle = undefined;
        try checkResult(f(self.ctx, asset_type, &sv, &out));
        return out;
    }

    pub fn assetGetLoadState(
        self: HostContextView,
        handle: *const c.BamAssetHandle,
    ) BamError!c.BamAssetLoadState {
        if (!coversHostApiSlot(self.api, offsets.ASSET_GET_LOAD_STATE, @sizeOf(c.BamAssetGetLoadStateFn))) {
            return error.Unsupported;
        }
        const f = self.api.asset_get_load_state orelse return error.Unsupported;
        var out: c.BamAssetLoadState = c.BAM_ASSET_LOAD_STATE_NOT_LOADED;
        try checkResult(f(self.ctx, handle, &out));
        return out;
    }

    pub fn assetRelease(self: HostContextView, handle: *const c.BamAssetHandle) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.ASSET_RELEASE, @sizeOf(c.BamAssetReleaseFn))) {
            return error.Unsupported;
        }
        const f = self.api.asset_release orelse return error.Unsupported;
        try checkResult(f(self.ctx, handle));
    }

    pub fn logDiagnostic(self: HostContextView, level: u32, msg: []const u8) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.LOG_DIAGNOSTIC, @sizeOf(c.BamLogDiagnosticFn))) {
            return error.Unsupported;
        }
        const f = self.api.log_diagnostic orelse return error.Unsupported;
        const sv = strView(msg);
        try checkResult(f(self.ctx, level, &sv));
    }

    pub fn reportGuestFault(self: HostContextView, msg: []const u8) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.REPORT_GUEST_FAULT, @sizeOf(c.BamReportGuestFaultFn))) {
            return error.Unsupported;
        }
        const f = self.api.report_guest_fault orelse return error.Unsupported;
        const sv = strView(msg);
        try checkResult(f(self.ctx, &sv));
    }

    pub fn resolveComponentByUuid(
        self: HostContextView,
        uuid: *const c.BamComponentUuid,
    ) BamError!c.BamComponent {
        if (!coversHostApiSlot(self.api, offsets.RESOLVE_COMPONENT_BY_UUID, @sizeOf(c.BamResolveComponentByUuidFn))) {
            return error.Unsupported;
        }
        const f = self.api.resolve_component_by_uuid orelse return error.Unsupported;
        var out: c.BamComponent = c.BAM_COMPONENT_INVALID;
        try checkResult(f(self.ctx, uuid, &out));
        return out;
    }

    pub fn componentLayoutInfo(
        self: HostContextView,
        component: c.BamComponent,
    ) BamError!c.BamLayoutInfo {
        if (!coversHostApiSlot(self.api, offsets.COMPONENT_LAYOUT_INFO, @sizeOf(c.BamComponentLayoutInfoFn))) {
            return error.Unsupported;
        }
        const f = self.api.component_layout_info orelse return error.Unsupported;
        var out: c.BamLayoutInfo = undefined;
        try checkResult(f(self.ctx, component, &out));
        return out;
    }

    pub fn internSymbol(self: HostContextView, symbol: []const u8) BamError!c.BamSymbol {
        if (!coversHostApiSlot(self.api, offsets.INTERN_SYMBOL, @sizeOf(c.BamInternSymbolFn))) {
            return error.Unsupported;
        }
        const f = self.api.intern_symbol orelse return error.Unsupported;
        const sv = strView(symbol);
        var out: c.BamSymbol = c.BAM_SYMBOL_INVALID;
        try checkResult(f(self.ctx, &sv, &out));
        return out;
    }

    pub fn bindNamedObject(
        self: HostContextView,
        name: []const u8,
        entity: c.BamEntity,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.BIND_NAMED_OBJECT, @sizeOf(c.BamBindNamedObjectFn))) {
            return error.Unsupported;
        }
        const f = self.api.bind_named_object orelse return error.Unsupported;
        const sv = strView(name);
        try checkResult(f(self.ctx, &sv, entity));
    }

    pub fn resolveNamedObject(self: HostContextView, name: []const u8) BamError!c.BamEntity {
        if (!coversHostApiSlot(self.api, offsets.RESOLVE_NAMED_OBJECT, @sizeOf(c.BamResolveNamedObjectFn))) {
            return error.Unsupported;
        }
        const f = self.api.resolve_named_object orelse return error.Unsupported;
        const sv = strView(name);
        var out: c.BamEntity = c.BAM_ENTITY_NULL;
        try checkResult(f(self.ctx, &sv, &out));
        return out;
    }

    pub fn resolveSymbolStr(self: HostContextView, symbol: c.BamSymbol) BamError!c.BamStrView {
        if (!coversHostApiSlot(self.api, offsets.RESOLVE_SYMBOL_STR, @sizeOf(c.BamResolveSymbolStrFn))) {
            return error.Unsupported;
        }
        const f = self.api.resolve_symbol_str orelse return error.Unsupported;
        var out: c.BamStrView = .{ .ptr = null, .len = 0 };
        try checkResult(f(self.ctx, symbol, &out));
        return out;
    }
};

pub const LoadContextView = struct {
    api: *const c.BamHostApi,
    ctx: *c.BamLoadCtx,

    pub fn hostCtx(self: LoadContextView) BamError!HostContextView {
        const f = self.api.load_host_ctx orelse return error.NullPointer;
        const h = f(self.ctx) orelse return error.NullPointer;
        return .{ .api = self.api, .ctx = h };
    }

    pub fn registerComponent(
        self: LoadContextView,
        desc: *const c.BamComponentDesc,
    ) BamError!c.BamComponent {
        const f = self.api.register_component orelse return error.NullPointer;
        var out: c.BamComponent = c.BAM_COMPONENT_INVALID;
        try checkResult(f(self.ctx, desc, &out));
        return out;
    }

    pub fn registerSystem(self: LoadContextView, desc: *const c.BamSystemDesc) BamError!void {
        const f = self.api.register_system orelse return error.NullPointer;
        try checkResult(f(self.ctx, desc));
    }

    pub fn registerObserver(self: LoadContextView, desc: *const c.BamObserverDesc) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.REGISTER_OBSERVER, @sizeOf(c.BamRegisterObserverFn))) {
            return error.Unsupported;
        }
        const f = self.api.register_observer orelse return error.Unsupported;
        try checkResult(f(self.ctx, desc));
    }

    pub fn registerRelation(
        self: LoadContextView,
        desc: *const c.BamRelationDesc,
    ) BamError!c.BamComponent {
        if (!coversHostApiSlot(self.api, offsets.REGISTER_RELATION, @sizeOf(c.BamRegisterRelationFn))) {
            return error.Unsupported;
        }
        const f = self.api.register_relation orelse return error.Unsupported;
        var out: c.BamComponent = c.BAM_COMPONENT_INVALID;
        try checkResult(f(self.ctx, desc, &out));
        return out;
    }

    pub fn registerSystemHandle(
        self: LoadContextView,
        desc: *const c.BamSystemDesc,
    ) BamError!c.BamSystem {
        if (!coversHostApiSlot(self.api, offsets.REGISTER_SYSTEM_HANDLE, @sizeOf(c.BamRegisterSystemHandleFn))) {
            return error.Unsupported;
        }
        const f = self.api.register_system_handle orelse return error.Unsupported;
        var out: c.BamSystem = c.BAM_SYSTEM_INVALID;
        try checkResult(f(self.ctx, desc, &out));
        return out;
    }

    pub fn configureScheduleSet(
        self: LoadContextView,
        desc: *const c.BamScheduleSetDesc,
    ) BamError!c.BamSystemSet {
        if (!coversHostApiSlot(self.api, offsets.CONFIGURE_SCHEDULE_SET, @sizeOf(c.BamConfigureScheduleSetFn))) {
            return error.Unsupported;
        }
        const f = self.api.configure_schedule_set orelse return error.Unsupported;
        var out: c.BamSystemSet = c.BAM_SYSTEM_SET_INVALID;
        try checkResult(f(self.ctx, desc, &out));
        return out;
    }

    pub fn mountSystemInSchedule(
        self: LoadContextView,
        system: c.BamSystem,
        schedule: c.BamScheduleId,
        set: c.BamSystemSet,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.MOUNT_SYSTEM_IN_SCHEDULE, @sizeOf(c.BamMountSystemInScheduleFn))) {
            return error.Unsupported;
        }
        const f = self.api.mount_system_in_schedule orelse return error.Unsupported;
        try checkResult(f(self.ctx, system, schedule, set));
    }

    pub fn migrateComponentColumn(
        self: LoadContextView,
        component: c.BamComponent,
        old_fingerprint: u64,
        new_desc: *const c.BamComponentDesc,
        migrate_fn: c.BamRowMigrateFn,
        user_data: ?*anyopaque,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.MIGRATE_COMPONENT_COLUMN, @sizeOf(c.BamMigrateComponentColumnFn))) {
            return error.Unsupported;
        }
        const f = self.api.migrate_component_column orelse return error.Unsupported;
        try checkResult(f(self.ctx, component, old_fingerprint, new_desc, migrate_fn, user_data));
    }

    pub fn registerResource(
        self: LoadContextView,
        desc: *const c.BamComponentDesc,
    ) BamError!c.BamComponent {
        if (!coversHostApiSlot(self.api, offsets.REGISTER_RESOURCE, @sizeOf(c.BamRegisterComponentFn))) {
            return error.Unsupported;
        }
        const f = self.api.register_resource orelse return error.Unsupported;
        var out: c.BamComponent = c.BAM_COMPONENT_INVALID;
        try checkResult(f(self.ctx, desc, &out));
        return out;
    }

    pub fn registerMessage(
        self: LoadContextView,
        desc: *const c.BamComponentDesc,
    ) BamError!c.BamComponent {
        if (!coversHostApiSlot(self.api, offsets.REGISTER_MESSAGE, @sizeOf(c.BamRegisterComponentFn))) {
            return error.Unsupported;
        }
        const f = self.api.register_message orelse return error.Unsupported;
        var out: c.BamComponent = c.BAM_COMPONENT_INVALID;
        try checkResult(f(self.ctx, desc, &out));
        return out;
    }
};

pub const ColumnTicks = struct {
    added: ?[*]const u32,
    changed: ?[*]const u32,
    last_run: u32,
    this_run: u32,
};

pub const ChunkView = struct {
    api: *const c.BamHostApi,
    iter: *c.BamIter,
    len: usize = 0,

    pub fn nextChunk(self: *ChunkView) ?usize {
        const f = self.api.iter_next_chunk orelse return null;
        var count: usize = 0;
        if (f(self.iter, &count) == c.BAM_FALSE) {
            self.len = 0;
            return null;
        }
        self.len = count;
        return count;
    }

    pub inline fn supportsNextChunkBatch(self: ChunkView) bool {
        return coversHostApiSlot(
            self.api,
            offsets.ITER_NEXT_CHUNK_BATCH,
            @sizeOf(c.BamIterNextChunkBatchFn),
        ) and self.api.iter_next_chunk_batch != null;
    }

    pub inline fn nextChunkBatchUnchecked(
        self: *ChunkView,
        out_entities: ?*?[*]const c.BamEntity,
        out_col_ptrs: []?[*]u8,
        mark_changed_mask: u32,
    ) ?usize {
        const f = self.api.iter_next_chunk_batch.?;
        var count: usize = 0;
        const ok = f(
            self.iter,
            &count,
            if (out_entities) |p| @ptrCast(p) else null,
            if (out_col_ptrs.len > 0) @ptrCast(out_col_ptrs.ptr) else null,
            out_col_ptrs.len,
            mark_changed_mask,
        );
        if (ok == c.BAM_FALSE) {
            self.len = 0;
            return null;
        }
        self.len = count;
        return count;
    }

    pub fn nextChunkBatch(
        self: *ChunkView,
        out_entities: ?*?[*]const c.BamEntity,
        out_col_ptrs: []?[*]u8,
        mark_changed_mask: u32,
    ) BamError!?usize {
        if (!self.supportsNextChunkBatch()) {
            return error.Unsupported;
        }
        return self.nextChunkBatchUnchecked(out_entities, out_col_ptrs, mark_changed_mask);
    }

    pub fn reset(self: *ChunkView) BamError!void {
        const f = self.api.iter_reset orelse return error.NullPointer;
        self.len = 0;
        try checkResult(f(self.iter));
    }

    pub fn entities(self: ChunkView) ?[*]const c.BamEntity {
        const f = self.api.iter_entities orelse return null;
        return f(self.iter);
    }

    pub fn columnReadonly(self: ChunkView, comptime T: type, term_index: usize) ?[*]const T {
        const f = self.api.iter_column_readonly orelse return null;
        const p = f(self.iter, term_index) orelse return null;
        return @ptrCast(@alignCast(p));
    }

    pub fn columnMutable(self: ChunkView, comptime T: type, term_index: usize) ?[*]T {
        const f = self.api.iter_column_mutable orelse return null;
        const p = f(self.iter, term_index) orelse return null;
        return @ptrCast(@alignCast(p));
    }

    pub fn columnMutableUntracked(self: ChunkView, comptime T: type, term_index: usize) ?[*]T {
        const f = self.api.iter_column_mutable_untracked orelse return null;
        const p = f(self.iter, term_index) orelse return null;
        return @ptrCast(@alignCast(p));
    }

    pub fn markChangedRange(
        self: ChunkView,
        term_index: usize,
        start_row: usize,
        row_count: usize,
    ) BamError!void {
        const f = self.api.iter_mark_changed_range orelse return error.NullPointer;
        try checkResult(f(self.iter, term_index, start_row, row_count));
    }

    pub fn columnRowsReadonly(self: ChunkView, term_index: usize) ?[*]const ?[*]const u8 {
        const f = self.api.iter_column_rows_readonly orelse return null;
        return @ptrCast(f(self.iter, term_index));
    }

    pub fn columnRowsMutable(self: ChunkView, term_index: usize) ?[*]const ?[*]u8 {
        const f = self.api.iter_column_rows_mutable orelse return null;
        return @ptrCast(f(self.iter, term_index));
    }

    pub fn columnRowsMutableUntracked(self: ChunkView, term_index: usize) ?[*]const ?[*]u8 {
        if (!coversHostApiSlot(self.api, offsets.ITER_COLUMN_ROWS_MUTABLE_UNTRACKED, @sizeOf(c.BamIterColumnRowsMutableUntrackedFn))) {
            return null;
        }
        const f = self.api.iter_column_rows_mutable_untracked orelse return null;
        return @ptrCast(f(self.iter, term_index));
    }

    pub fn markChangedRanges(
        self: ChunkView,
        term_index: usize,
        ranges: []const c.BamRowRange,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.ITER_MARK_CHANGED_RANGES, @sizeOf(c.BamIterMarkChangedRangesFn))) {
            return error.Unsupported;
        }
        const f = self.api.iter_mark_changed_ranges orelse return error.Unsupported;
        try checkResult(f(self.iter, term_index, ranges.ptr, ranges.len));
    }

    pub fn columnInfo(self: ChunkView, term_index: usize) BamError!c.BamColumnInfo {
        const f = self.api.iter_column_info orelse return error.NullPointer;
        var out: c.BamColumnInfo = undefined;
        try checkResult(f(self.iter, term_index, &out));
        return out;
    }

    pub fn resPtr(self: ChunkView, comptime T: type, term_index: usize) ?*const T {
        const f = self.api.res_ptr orelse return null;
        const p = f(self.iter, term_index) orelse return null;
        return @ptrCast(@alignCast(p));
    }

    pub fn resMutPtr(self: ChunkView, comptime T: type, term_index: usize) ?*T {
        const f = self.api.res_mut_ptr orelse return null;
        const p = f(self.iter, term_index) orelse return null;
        return @ptrCast(@alignCast(p));
    }

    pub fn resTicks(self: ChunkView, term_index: usize) BamError!c.BamChangeTicks {
        if (!coversHostApiSlot(self.api, offsets.RES_TICKS, @sizeOf(c.BamResTicksFn))) {
            return error.Unsupported;
        }
        const f = self.api.res_ticks orelse return error.Unsupported;
        var out: c.BamChangeTicks = undefined;
        try checkResult(f(self.iter, term_index, &out));
        return out;
    }

    pub fn parForEachChunk(
        self: ChunkView,
        chunk_fn: c.BamChunkTaskFn,
        user_data: ?*anyopaque,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.PAR_FOR_EACH_CHUNK, @sizeOf(c.BamParForEachChunkFn))) {
            return error.Unsupported;
        }
        const f = self.api.par_for_each_chunk orelse return error.Unsupported;
        try checkResult(f(self.iter, chunk_fn, user_data));
    }

    pub fn parForEachChunkReadonly(
        self: ChunkView,
        chunk_fn: c.BamChunkTaskFn,
        user_data: ?*anyopaque,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.PAR_FOR_EACH_CHUNK_READONLY, @sizeOf(c.BamParForEachChunkFn))) {
            return error.Unsupported;
        }
        const f = self.api.par_for_each_chunk_readonly orelse return error.Unsupported;
        try checkResult(f(self.iter, chunk_fn, user_data));
    }

    pub fn parForEachChunkEx(
        self: ChunkView,
        chunk_fn: c.BamChunkTaskExFn,
        user_data: ?*anyopaque,
        options: ?*const c.BamParOptions,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.PAR_FOR_EACH_CHUNK_EX, @sizeOf(c.BamParForEachChunkExFn))) {
            return error.Unsupported;
        }
        const f = self.api.par_for_each_chunk_ex orelse return error.Unsupported;
        try checkResult(f(self.iter, chunk_fn, user_data, options));
    }

    pub fn msgReadSlice(
        self: ChunkView,
        comptime T: type,
        term_index: usize,
    ) BamError![]const T {
        if (!coversHostApiSlot(self.api, offsets.MSG_READ_SLICE, @sizeOf(c.BamMsgReadSliceFn))) {
            return error.Unsupported;
        }
        const f = self.api.msg_read_slice orelse return error.Unsupported;
        var ptr: ?[*]const u8 = null;
        var count: usize = 0;
        try checkResult(f(self.iter, term_index, @ptrCast(&ptr), &count));
        if (count == 0) return &.{};
        const typed: [*]const T = @ptrCast(@alignCast(ptr orelse return error.NullPointer));
        return typed[0..count];
    }

    pub fn msgReadBatch(
        self: ChunkView,
        comptime T: type,
        term_index: usize,
        out_buf: []T,
    ) BamError!usize {
        if (!coversHostApiSlot(self.api, offsets.MSG_READ_BATCH, @sizeOf(c.BamMsgReadBatchFn))) {
            return error.Unsupported;
        }
        const f = self.api.msg_read_batch orelse return error.Unsupported;
        var read_count: usize = 0;
        try checkResult(f(self.iter, term_index, @ptrCast(out_buf.ptr), out_buf.len, &read_count));
        return read_count;
    }

    pub fn msgSend(self: ChunkView, term_index: usize, data: []const u8) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.MSG_SEND, @sizeOf(c.BamMsgSendFn))) {
            return error.Unsupported;
        }
        const f = self.api.msg_send orelse return error.Unsupported;
        try checkResult(f(self.iter, term_index, data.ptr, data.len));
    }

    pub fn msgWriteBatch(
        self: ChunkView,
        comptime T: type,
        term_index: usize,
        in_buf: []const T,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.MSG_WRITE_BATCH, @sizeOf(c.BamMsgWriteBatchFn))) {
            return error.Unsupported;
        }
        const f = self.api.msg_write_batch orelse return error.Unsupported;
        try checkResult(f(self.iter, term_index, @ptrCast(in_buf.ptr), in_buf.len));
    }

    pub fn readRemoved(
        self: ChunkView,
        term_index: usize,
        out_entities: []c.BamEntity,
    ) BamError!usize {
        if (!coversHostApiSlot(self.api, offsets.ITER_READ_REMOVED, @sizeOf(c.BamIterReadRemovedFn))) {
            return error.Unsupported;
        }
        const f = self.api.iter_read_removed orelse return error.Unsupported;
        var read_count: usize = 0;
        try checkResult(f(self.iter, term_index, out_entities.ptr, out_entities.len, &read_count));
        return read_count;
    }

    pub fn totalCount(self: ChunkView) BamError!usize {
        if (!coversHostApiSlot(self.api, offsets.ITER_COUNT, @sizeOf(c.BamIterCountFn))) {
            return error.Unsupported;
        }
        const f = self.api.iter_count orelse return error.Unsupported;
        var out: usize = 0;
        try checkResult(f(self.iter, &out));
        return out;
    }

    pub fn upCascade(
        self: ChunkView,
        relation: c.BamComponent,
        entity: c.BamEntity,
        term_index: usize,
        out_ptr: *?*const anyopaque,
        out_owner: *c.BamEntity,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.ITER_UP_CASCADE, @sizeOf(c.BamIterUpCascadeFn))) {
            return error.Unsupported;
        }
        const f = self.api.iter_up_cascade orelse return error.Unsupported;
        try checkResult(f(self.iter, relation, entity, term_index, out_ptr, out_owner));
    }

    pub fn columnTicks(self: ChunkView, term_index: usize) BamError!ColumnTicks {
        if (!coversHostApiSlot(self.api, offsets.ITER_COLUMN_TICKS, @sizeOf(c.BamIterColumnTicksFn))) {
            return error.Unsupported;
        }
        const f = self.api.iter_column_ticks orelse return error.Unsupported;
        var added: ?[*]const u32 = null;
        var changed: ?[*]const u32 = null;
        var last_run: u32 = 0;
        var this_run: u32 = 0;
        try checkResult(f(self.iter, term_index, @ptrCast(&added), @ptrCast(&changed), &last_run, &this_run));
        return .{
            .added = added,
            .changed = changed,
            .last_run = last_run,
            .this_run = this_run,
        };
    }
};

pub const CommandsView = struct {
    api: *const c.BamHostApi,
    cmds: *c.BamCommands,

    pub fn spawnEmpty(self: CommandsView) BamError!c.BamEntity {
        const f = self.api.cmd_spawn_empty orelse return error.NullPointer;
        var out: c.BamEntity = c.BAM_ENTITY_NULL;
        try checkResult(f(self.cmds, &out));
        return out;
    }

    pub fn spawn(
        self: CommandsView,
        components: [*]const c.BamComponent,
        data_ptrs: [*]const ?[*]const u8,
        count: usize,
    ) BamError!c.BamEntity {
        const f = self.api.cmd_spawn orelse return error.NullPointer;
        var out: c.BamEntity = c.BAM_ENTITY_NULL;
        try checkResult(f(self.cmds, components, @ptrCast(data_ptrs), count, &out));
        return out;
    }

    pub fn spawnPersistent(
        self: CommandsView,
        components: [*]const c.BamComponent,
        data_ptrs: [*]const ?[*]const u8,
        count: usize,
    ) BamError!c.BamEntity {
        if (!coversHostApiSlot(self.api, offsets.CMD_SPAWN_PERSISTENT, @sizeOf(c.BamCmdSpawnFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_spawn_persistent orelse return error.Unsupported;
        var out: c.BamEntity = c.BAM_ENTITY_NULL;
        try checkResult(f(self.cmds, components, @ptrCast(data_ptrs), count, &out));
        return out;
    }

    pub fn spawnBatch(
        self: CommandsView,
        components: [*]const c.BamComponent,
        column_ptrs: [*]const ?[*]const u8,
        component_count: usize,
        out_entities: []c.BamEntity,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_SPAWN_BATCH, @sizeOf(c.BamCmdSpawnBatchFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_spawn_batch orelse return error.Unsupported;
        try checkResult(f(self.cmds, components, @ptrCast(column_ptrs), component_count, out_entities.len, out_entities.ptr));
    }

    pub fn despawn(self: CommandsView, entity: c.BamEntity) BamError!void {
        const f = self.api.cmd_despawn orelse return error.NullPointer;
        try checkResult(f(self.cmds, entity));
    }

    pub fn despawnAll(self: CommandsView, terms: []const c.BamTermDesc) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_DESPAWN_ALL, @sizeOf(c.BamCmdDespawnAllFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_despawn_all orelse return error.Unsupported;
        try checkResult(f(self.cmds, terms.ptr, terms.len));
    }

    pub fn insert(
        self: CommandsView,
        entity: c.BamEntity,
        component: c.BamComponent,
        data: []const u8,
    ) BamError!void {
        const f = self.api.cmd_insert orelse return error.NullPointer;
        try checkResult(f(self.cmds, entity, component, data.ptr, data.len));
    }

    pub fn insertDefault(
        self: CommandsView,
        entity: c.BamEntity,
        component: c.BamComponent,
    ) BamError!void {
        const f = self.api.cmd_insert_default orelse return error.NullPointer;
        try checkResult(f(self.cmds, entity, component));
    }

    pub fn insertIfNew(
        self: CommandsView,
        entity: c.BamEntity,
        component: c.BamComponent,
        data: []const u8,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_INSERT_IF_NEW, @sizeOf(c.BamCmdInsertFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_insert_if_new orelse return error.Unsupported;
        try checkResult(f(self.cmds, entity, component, data.ptr, data.len));
    }

    pub fn remove(
        self: CommandsView,
        entity: c.BamEntity,
        component: c.BamComponent,
    ) BamError!void {
        const f = self.api.cmd_remove orelse return error.NullPointer;
        try checkResult(f(self.cmds, entity, component));
    }

    pub fn trigger(
        self: CommandsView,
        event: c.BamComponent,
        target: c.BamEntity,
        data: []const u8,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_TRIGGER, @sizeOf(c.BamCmdTriggerFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_trigger orelse return error.Unsupported;
        try checkResult(f(self.cmds, event, target, data.ptr, data.len));
    }

    pub fn triggerEx(self: CommandsView, desc: *const c.BamTriggerDesc) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_TRIGGER_EX, @sizeOf(c.BamCmdTriggerExFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_trigger_ex orelse return error.Unsupported;
        try checkResult(f(self.cmds, desc));
    }

    pub fn observe(self: CommandsView, desc: *const c.BamObserverDesc) BamError!c.BamEntity {
        if (!coversHostApiSlot(self.api, offsets.CMD_OBSERVE, @sizeOf(c.BamCmdObserveFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_observe orelse return error.Unsupported;
        var out: c.BamEntity = c.BAM_ENTITY_NULL;
        try checkResult(f(self.cmds, desc, &out));
        return out;
    }

    pub fn spawnScene(
        self: CommandsView,
        scene_handle: *const c.BamAssetHandle,
        parent: c.BamEntity,
    ) BamError!c.BamEntity {
        if (!coversHostApiSlot(self.api, offsets.CMD_SPAWN_SCENE, @sizeOf(c.BamCmdSpawnSceneFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_spawn_scene orelse return error.Unsupported;
        var out: c.BamEntity = c.BAM_ENTITY_NULL;
        try checkResult(f(self.cmds, scene_handle, parent, &out));
        return out;
    }

    pub fn checkpoint(self: CommandsView) BamError!usize {
        if (!coversHostApiSlot(self.api, offsets.CMD_CHECKPOINT, @sizeOf(c.BamCmdCheckpointFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_checkpoint orelse return error.Unsupported;
        var token: usize = 0;
        try checkResult(f(self.cmds, &token));
        return token;
    }

    pub fn rollback(self: CommandsView, token: usize) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_ROLLBACK, @sizeOf(c.BamCmdRollbackFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_rollback orelse return error.Unsupported;
        try checkResult(f(self.cmds, token));
    }

    pub fn insertRelation(
        self: CommandsView,
        entity: c.BamEntity,
        relation: c.BamComponent,
        target: c.BamEntity,
        data: ?*const anyopaque,
        data_size: usize,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_INSERT_RELATION, @sizeOf(c.BamCmdInsertRelationFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_insert_relation orelse return error.Unsupported;
        try checkResult(f(self.cmds, entity, relation, target, data, data_size));
    }

    pub fn removeRelation(
        self: CommandsView,
        entity: c.BamEntity,
        relation: c.BamComponent,
        target: c.BamEntity,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_REMOVE_RELATION, @sizeOf(c.BamCmdRemoveRelationFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_remove_relation orelse return error.Unsupported;
        try checkResult(f(self.cmds, entity, relation, target));
    }

    pub fn cloneEntity(
        self: CommandsView,
        src_entity: c.BamEntity,
        dst_entity: c.BamEntity,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_CLONE_ENTITY, @sizeOf(c.BamCmdCloneEntityFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_clone_entity orelse return error.Unsupported;
        try checkResult(f(self.cmds, src_entity, dst_entity));
    }

    pub fn insertResource(
        self: CommandsView,
        resource: c.BamComponent,
        data: []const u8,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_INSERT_RESOURCE, @sizeOf(c.BamCmdInsertResourceFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_insert_resource orelse return error.Unsupported;
        try checkResult(f(self.cmds, resource, data.ptr, data.len));
    }

    pub fn removeResource(self: CommandsView, resource: c.BamComponent) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_REMOVE_RESOURCE, @sizeOf(c.BamCmdRemoveResourceFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_remove_resource orelse return error.Unsupported;
        try checkResult(f(self.cmds, resource));
    }

    pub fn despawnBatch(self: CommandsView, entities: []const c.BamEntity) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_DESPAWN_BATCH, @sizeOf(c.BamCmdDespawnBatchFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_despawn_batch orelse return error.Unsupported;
        try checkResult(f(self.cmds, entities.ptr, entities.len));
    }

    pub fn insertBatch(
        self: CommandsView,
        entities: []const c.BamEntity,
        component: c.BamComponent,
        data_ptr: ?[*]const u8,
        elem_size: usize,
        flags: u32,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_INSERT_BATCH, @sizeOf(c.BamCmdInsertBatchFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_insert_batch orelse return error.Unsupported;
        try checkResult(f(self.cmds, entities.ptr, entities.len, component, data_ptr, elem_size, flags));
    }

    pub fn removeBatch(
        self: CommandsView,
        entities: []const c.BamEntity,
        component: c.BamComponent,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_REMOVE_BATCH, @sizeOf(c.BamCmdRemoveBatchFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_remove_batch orelse return error.Unsupported;
        try checkResult(f(self.cmds, entities.ptr, entities.len, component));
    }

    pub fn spawnBatchEx(
        self: CommandsView,
        columns: []const c.BamSpawnColumnDesc,
        out_entities: []c.BamEntity,
    ) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.CMD_SPAWN_BATCH_EX, @sizeOf(c.BamCmdSpawnBatchExFn))) {
            return error.Unsupported;
        }
        const f = self.api.cmd_spawn_batch_ex orelse return error.Unsupported;
        try checkResult(f(self.cmds, columns.ptr, columns.len, out_entities.len, out_entities.ptr));
    }
};

/// RAII-style command-buffer transaction guard for Zig `defer tx.deinit()`.
///
/// Calls `cmds.checkpoint()` on `init` and automatically rolls back on `deinit()`
/// unless `commit()` or `rollbackNow()` was called first.
pub const ScopedTransaction = struct {
    cmds: CommandsView,
    token: usize,
    active: bool,

    pub fn init(cmds: CommandsView) BamError!ScopedTransaction {
        const tok = try cmds.checkpoint();
        return .{
            .cmds = cmds,
            .token = tok,
            .active = true,
        };
    }

    pub fn commit(self: *ScopedTransaction) void {
        self.active = false;
    }

    pub fn rollbackNow(self: *ScopedTransaction) BamError!void {
        if (!self.active) return;
        self.active = false;
        try self.cmds.rollback(self.token);
    }

    pub fn deinit(self: *ScopedTransaction) void {
        if (self.active) {
            self.active = false;
            self.cmds.rollback(self.token) catch {};
        }
    }
};

pub const SystemContextView = struct {
    api: *const c.BamHostApi,
    ctx: *c.BamSystemCtx,

    pub fn hostCtx(self: SystemContextView) BamError!HostContextView {
        const f = self.api.sys_host_ctx orelse return error.NullPointer;
        const h = f(self.ctx) orelse return error.NullPointer;
        return .{ .api = self.api, .ctx = h };
    }

    pub fn query(self: SystemContextView, index: usize) BamError!ChunkView {
        const f = self.api.sys_query orelse return error.NullPointer;
        var out_iter: ?*c.BamIter = null;
        try checkResult(f(self.ctx, index, &out_iter));
        return .{ .api = self.api, .iter = out_iter orelse return error.NullPointer };
    }

    pub fn commands(self: SystemContextView) BamError!CommandsView {
        const f = self.api.sys_commands orelse return error.NullPointer;
        var out_cmds: ?*c.BamCommands = null;
        try checkResult(f(self.ctx, &out_cmds));
        return .{ .api = self.api, .cmds = out_cmds orelse return error.NullPointer };
    }

    pub fn entityGet(
        self: SystemContextView,
        comptime T: type,
        query_index: usize,
        entity: c.BamEntity,
        term_index: usize,
    ) ?*const T {
        if (!coversHostApiSlot(self.api, offsets.SYS_ENTITY_GET, @sizeOf(c.BamSysEntityGetFn))) {
            return null;
        }
        const f = self.api.sys_entity_get orelse return null;
        const p = f(self.ctx, query_index, entity, term_index) orelse return null;
        return @ptrCast(@alignCast(p));
    }

    pub fn entityGetMut(
        self: SystemContextView,
        comptime T: type,
        query_index: usize,
        entity: c.BamEntity,
        term_index: usize,
    ) ?*T {
        if (!coversHostApiSlot(self.api, offsets.SYS_ENTITY_GET_MUT, @sizeOf(c.BamSysEntityGetMutFn))) {
            return null;
        }
        const f = self.api.sys_entity_get_mut orelse return null;
        const p = f(self.ctx, query_index, entity, term_index) orelse return null;
        return @ptrCast(@alignCast(p));
    }

    pub fn resPtr(
        self: SystemContextView,
        comptime T: type,
        comp: c.BamComponent,
        out_ticks: ?*c.BamChangeTicks,
    ) ?*const T {
        if (!coversHostApiSlot(self.api, offsets.SYS_RES_PTR, @sizeOf(c.BamSysResPtrFn))) {
            return null;
        }
        const f = self.api.sys_res_ptr orelse return null;
        const p = f(self.ctx, comp, out_ticks) orelse return null;
        return @ptrCast(@alignCast(p));
    }

    pub fn resMutPtr(
        self: SystemContextView,
        comptime T: type,
        comp: c.BamComponent,
        out_ticks: ?*c.BamChangeTicks,
    ) ?*T {
        if (!coversHostApiSlot(self.api, offsets.SYS_RES_MUT_PTR, @sizeOf(c.BamSysResMutPtrFn))) {
            return null;
        }
        const f = self.api.sys_res_mut_ptr orelse return null;
        const p = f(self.ctx, comp, out_ticks) orelse return null;
        return @ptrCast(@alignCast(p));
    }

    pub fn entityChildren(
        self: SystemContextView,
        entity: c.BamEntity,
        relationship_target: c.BamComponent,
    ) BamError![]const c.BamEntity {
        if (!coversHostApiSlot(self.api, offsets.SYS_ENTITY_CHILDREN, @sizeOf(c.BamSysEntityChildrenFn))) {
            return error.Unsupported;
        }
        const f = self.api.sys_entity_children orelse return error.Unsupported;
        var ptr: ?[*]const c.BamEntity = null;
        var len: usize = 0;
        try checkResult(f(self.ctx, entity, relationship_target, &ptr, &len));
        if (len == 0) return &.{};
        return (ptr orelse return error.NullPointer)[0..len];
    }

    pub fn currentTicks(self: SystemContextView) BamError!struct { last_run: u32, this_run: u32 } {
        if (!coversHostApiSlot(self.api, offsets.SYS_CURRENT_TICKS, @sizeOf(c.BamSysCurrentTicksFn))) {
            return error.Unsupported;
        }
        const f = self.api.sys_current_ticks orelse return error.Unsupported;
        var last_run: u32 = 0;
        var this_run: u32 = 0;
        try checkResult(f(self.ctx, &last_run, &this_run));
        return .{ .last_run = last_run, .this_run = this_run };
    }

    pub fn timeInfo(self: SystemContextView) BamError!c.BamTimeInfo {
        if (!coversHostApiSlot(self.api, offsets.SYS_TIME_INFO, @sizeOf(c.BamSysTimeInfoFn))) {
            return error.Unsupported;
        }
        const f = self.api.sys_time_info orelse return error.Unsupported;
        var info = c.bam_time_info();
        try checkResult(f(self.ctx, &info));
        return info;
    }

    pub fn relationTargets(
        self: SystemContextView,
        entity: c.BamEntity,
        relation: c.BamComponent,
    ) BamError![]const c.BamEntity {
        if (!coversHostApiSlot(self.api, offsets.SYS_RELATION_TARGETS, @sizeOf(c.BamSysRelationTargetsFn))) {
            return error.Unsupported;
        }
        const f = self.api.sys_relation_targets orelse return error.Unsupported;
        var ptr: ?[*]const c.BamEntity = null;
        var len: usize = 0;
        try checkResult(f(self.ctx, entity, relation, &ptr, &len));
        if (len == 0) return &.{};
        return (ptr orelse return error.NullPointer)[0..len];
    }

    pub fn relationSources(
        self: SystemContextView,
        target: c.BamEntity,
        relation: c.BamComponent,
    ) BamError![]const c.BamEntity {
        if (!coversHostApiSlot(self.api, offsets.SYS_RELATION_SOURCES, @sizeOf(c.BamSysRelationSourcesFn))) {
            return error.Unsupported;
        }
        const f = self.api.sys_relation_sources orelse return error.Unsupported;
        var ptr: ?[*]const c.BamEntity = null;
        var len: usize = 0;
        try checkResult(f(self.ctx, target, relation, &ptr, &len));
        if (len == 0) return &.{};
        return (ptr orelse return error.NullPointer)[0..len];
    }

    pub fn relationEdgeData(
        self: SystemContextView,
        entity: c.BamEntity,
        relation: c.BamComponent,
        target: c.BamEntity,
    ) BamError![]const u8 {
        if (!coversHostApiSlot(self.api, offsets.SYS_RELATION_EDGE_DATA, @sizeOf(c.BamSysRelationEdgeDataFn))) {
            return error.Unsupported;
        }
        const f = self.api.sys_relation_edge_data orelse return error.Unsupported;
        var ptr: ?*const anyopaque = null;
        var len: usize = 0;
        try checkResult(f(self.ctx, entity, relation, target, &ptr, &len));
        if (len == 0) return &.{};
        const bytes: [*]const u8 = @ptrCast(ptr orelse return error.NullPointer);
        return bytes[0..len];
    }
};
"#;

/// Exports `bamabi.h`, `bamabi.hpp`, and `bamabi.zig` into `out_dir`.
///
/// Intended purpose: equips polyglot C99, C++17, and Zig 0.16.0 guest projects (via `cargo-bam
/// export-sdk`) with a synchronized, version-matched ABI `0.4.0` binding bundle in one step.
///
/// What it does: creates `out_dir` recursively, renders `bamabi.h` via [`crate::render_c99_header`],
/// writes `bamabi.h`, [`CPP17_HEADER`] (`bamabi.hpp`), and [`ZIG_SDK`] (`bamabi.zig`), and
/// returns the list of written paths.
///
/// # Errors
/// Returns an `io::Error` if writing any SDK artifact fails or C header layout checks fail.
pub fn export_sdk(out_dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    fs::create_dir_all(out_dir)?;
    let c_hdr = crate::render_c99_header().map_err(std::io::Error::other)?;
    let cpp_hdr = cpp17_header();
    let zig_hdr = zig_sdk();
    let files: [(&str, &str); 3] = [
        ("bamabi.h", &c_hdr),
        ("bamabi.hpp", &cpp_hdr),
        ("bamabi.zig", &zig_hdr),
    ];
    let mut written = Vec::with_capacity(files.len());
    for (filename, content) in files {
        let dest = out_dir.join(filename);
        fs::write(&dest, content.as_bytes())?;
        written.push(dest);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use bamabi_sys::BAM_HOST_API_SLOT_COUNT;

    use super::*;

    /// Test scenario: invokes [`export_sdk`] into a temporary directory and verifies that all
    /// three ABI `0.4.0` SDK files (`bamabi.h`, `bamabi.hpp`, `bamabi.zig`) are written and
    /// contain required `0.4.0` symbols (`BAM_ABI_VERSION`, `BAM_RESTRICT`, `CYCLE_DETECTED`,
    /// relation/schedule-set/splat flags, batch/telemetry macros, and Zig `addTranslateC` `"bam_c"`
    /// import without legacy `@cImport`/`@cInclude`).
    #[test]
    fn export_sdk_writes_all_three_abi_0_4_headers() {
        let tmp = std::env::temp_dir().join(format!("bam_sdk_gen_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let written = export_sdk(&tmp).expect("export_sdk must succeed");
        assert_eq!(written.len(), 3);
        let h_content = fs::read_to_string(tmp.join("bamabi.h")).unwrap();
        let hpp_content = fs::read_to_string(tmp.join("bamabi.hpp")).unwrap();
        let zig_content = fs::read_to_string(tmp.join("bamabi.zig")).unwrap();
        assert!(h_content.contains("#define BAM_ABI_VERSION 0x00040000u"));
        assert!(h_content.contains("#  define BAM_RESTRICT restrict"));
        assert!(h_content.contains("#define BAM_RESULT_CYCLE_DETECTED    ((BamResult)16u)"));
        assert!(h_content.contains("#define BAM_RELATION_FLAG_CASCADE_DELETE    0x00000002u"));
        assert!(h_content.contains("#define BAM_RELATION_FLAG_ACYCLIC           0x00000008u"));
        assert!(h_content.contains("#define BAM_RELATION_FLAG_CLEANUP_REMOVE    0x00000010u"));
        assert!(h_content.contains("#define BAM_SCHEDULE_SET_FLAG_SEQUENCE      0x00000001u"));
        assert!(h_content.contains("#define BAM_SCHEDULE_SET_FLAG_SEQUENCE_WEAK 0x00000002u"));
        assert!(h_content.contains("#define BAM_SCHEDULE_SET_FLAG_PARALLEL      0x00000004u"));
        assert!(h_content.contains("#define BAM_SCHEDULE_SET_FLAG_BEFORE_WEAK   0x00000008u"));
        assert!(h_content.contains("#define BAM_SCHEDULE_SET_FLAG_AFTER_WEAK    0x00000010u"));
        assert!(h_content.contains("#define BAM_SPAWN_COL_FLAG_SPLAT            0x00000001u"));
        assert!(h_content.contains("bam_hash_field_schema("));
        assert!(h_content.contains("bam_fnv1a_64_hash_system("));
        assert!(hpp_content.contains("BAM_HOST_API_OFFSET_CMD_CHECKPOINT"));
        assert!(hpp_content.contains("BAM_HOST_API_OFFSET_CMD_SPAWN_BATCH_EX"));
        assert!(hpp_content.contains("hash_field_schema("));
        assert!(hpp_content.contains("BAM_RESTRICT"));
        assert!(zig_content.contains("ABI 0.4.0"));
        assert!(zig_content.contains("addTranslateC"));
        assert!(zig_content.contains("@import(\"bam_c\")"));
        assert!(!zig_content.contains(concat!("@", "cImport")));
        assert!(!zig_content.contains(concat!("@", "cInclude")));
        assert!(zig_content.contains("BAM_RESTRICT_QUALIFIER"));
        assert!(zig_content.contains("BAM_RESULT_CYCLE_DETECTED"));
        assert!(zig_content.contains("SystemTelemetryDigest64"));
        assert!(zig_content.contains("fnv1a64HashSystem"));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Test scenario: verifies cross-language offset and aliasing-qualifier parity across C99,
    /// C++17, and Zig by checking that `BAM_RESTRICT` / `noalias` and every single one of the 90
    /// `BamHostApi` vtable slot offsets (plus header offsets) are mirrored identically in
    /// `c99_header()`, [`CPP17_HEADER`], and [`ZIG_SDK`].
    #[test]
    fn all_90_host_api_offsets_and_restrict_mirrored_across_c_cpp_and_zig() {
        let h_content = c99_header();
        let slots = crate::header_gen::host_api_slot_names();
        assert_eq!(slots.len(), BAM_HOST_API_SLOT_COUNT);

        assert!(h_content.contains("#if defined(_MSC_VER)\n#  define BAM_RESTRICT __restrict\n"));
        assert!(h_content.contains(
            "#elif !defined(__cplusplus) && defined(__STDC_VERSION__) && (__STDC_VERSION__ >= 199901L)\n#  define BAM_RESTRICT restrict\n"
        ));
        assert!(h_content.contains(
            "#elif defined(__GNUC__) || defined(__clang__)\n#  define BAM_RESTRICT __restrict__\n"
        ));
        assert!(CPP17_HEADER.contains("#ifndef BAM_RESTRICT"));
        assert!(CPP17_HEADER.contains(
            "#  elif defined(__GNUC__) || defined(__clang__)\n#    define BAM_RESTRICT __restrict__\n"
        ));
        assert!(CPP17_HEADER.contains("T* BAM_RESTRICT dst"));
        assert!(CPP17_HEADER.contains(
            "inline constexpr std::size_t STRUCT_SIZE = offsetof(BamHostApi, struct_size);"
        ));
        assert!(CPP17_HEADER.contains(
            "inline constexpr std::size_t ABI_VERSION = offsetof(BamHostApi, abi_version);"
        ));
        assert!(
            ZIG_SDK.contains(
                "pub const STRUCT_SIZE: usize = @offsetOf(c.BamHostApi, \"struct_size\");"
            )
        );
        assert!(
            ZIG_SDK.contains(
                "pub const ABI_VERSION: usize = @offsetOf(c.BamHostApi, \"abi_version\");"
            )
        );
        assert!(ZIG_SDK.contains("noalias dst: [*]T"));

        for slot in slots {
            let upper = slot.to_ascii_uppercase();
            let c_line =
                format!("#define BAM_HOST_API_OFFSET_{upper} offsetof(BamHostApi, {slot})");
            let cpp_line =
                format!("inline constexpr std::size_t {upper} = BAM_HOST_API_OFFSET_{upper};");
            let zig_offset_line =
                format!("pub const {upper}: usize = @offsetOf(c.BamHostApi, \"{slot}\");");
            let zig_alias_line =
                format!("pub const BAM_HOST_API_OFFSET_{upper}: usize = offsets.{upper};");

            assert!(h_content.contains(&c_line), "C99 header missing: {c_line}");
            assert!(
                CPP17_HEADER.contains(&cpp_line),
                "CPP17_HEADER missing: {cpp_line}"
            );
            assert!(
                ZIG_SDK.contains(&zig_offset_line),
                "ZIG_SDK missing: {zig_offset_line}"
            );
            assert!(
                ZIG_SDK.contains(&zig_alias_line),
                "ZIG_SDK missing: {zig_alias_line}"
            );
        }
    }

    /// Test scenario: verifies that all 90 `BamHostApi` function-pointer slots are wrapped by
    /// ergonomic view methods in both [`CPP17_HEADER`] (`HostContextView`, `LoadContextView`,
    /// `ChunkView`, `CommandsView`, `ScopedTransaction`, `SystemContextView`) and [`ZIG_SDK`],
    /// and that [`ZIG_SDK`] maps all 18 `BamResult` status codes losslessly in `checkResult` and
    /// `mapError`.
    #[test]
    fn cpp17_and_zig_views_wrap_all_90_slots_and_all_18_result_codes() {
        let slots = crate::header_gen::host_api_slot_names();
        assert_eq!(slots.len(), BAM_HOST_API_SLOT_COUNT);

        assert!(CPP17_HEADER.contains("class ScopedTransaction"));
        assert!(ZIG_SDK.contains("pub const ScopedTransaction = struct"));
        assert!(ZIG_SDK.contains("pub const SLOT_COUNT: usize = 90;"));
        for slot in &slots {
            let cpp_dot = format!("api_.{slot}(");
            let cpp_arrow = format!("api_->{slot}(");
            assert!(
                CPP17_HEADER.contains(&cpp_dot) || CPP17_HEADER.contains(&cpp_arrow),
                "CPP17_HEADER view methods do not call BamHostApi slot `{slot}`"
            );

            let zig_field = format!("self.api.{slot}");
            assert!(
                ZIG_SDK.contains(&zig_field),
                "ZIG_SDK view methods do not access BamHostApi slot `{slot}`"
            );
        }

        let zig_results = [
            "BAM_RESULT_OK",
            "BAM_RESULT_INVALID_COMPONENT",
            "BAM_RESULT_ACCESS_CONFLICT",
            "BAM_RESULT_PANIC_CAUGHT",
            "BAM_RESULT_NULL_POINTER",
            "BAM_RESULT_INVALID_UTF8",
            "BAM_RESULT_PERMISSION_DENIED",
            "BAM_RESULT_INVALID_VERSION",
            "BAM_RESULT_INVALID_ARGUMENT",
            "BAM_RESULT_OUT_OF_RANGE",
            "BAM_RESULT_NOT_FOUND",
            "BAM_RESULT_INVALID_STATE",
            "BAM_RESULT_INVALID_ENTITY",
            "BAM_RESULT_CAPACITY_EXCEEDED",
            "BAM_RESULT_UNSUPPORTED",
            "BAM_RESULT_TYPE_MISMATCH",
            "BAM_RESULT_CYCLE_DETECTED",
            "BAM_RESULT_INTERNAL_ERROR",
        ];
        for code in zig_results {
            assert!(
                ZIG_SDK.contains(code),
                "ZIG_SDK missing BamResult mapping for `{code}`"
            );
        }
    }
}
