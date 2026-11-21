//! Multi-language SDK exporter (`bamabi.h`, `bamabi.hpp`, `bamabi.zig`).

use std::fs;
use std::path::{Path, PathBuf};

/// Generates the canonical strict ISO C99 header directly from `bamabi_sys`.
///
/// Intended purpose: provides an infallible-for-valid-builds convenience wrapper for CLI and test
/// callers that need the rendered `bamabi.h` text without handling layout-assertion errors.
///
/// What it does: delegates to [`crate::render_c99_header`] and unwraps the result.
///
/// # Panics
/// Panics only if internal `bamabi_sys` wire layout invariants are violated.
#[must_use]
pub fn c99_header() -> String {
    crate::render_c99_header().expect("valid bamabi_sys wire layouts")
}

/// Ergonomic ISO C++17 RAII wrapper header over `bamabi.h` (`ABI 0.3.0`).
pub const CPP17_HEADER: &str = r#"#ifndef BAMABI_HPP
#define BAMABI_HPP

#include "bamabi.h"

#include <cstddef>
#include <cstdint>
#include <exception>

#ifndef BAM_RESTRICT
#  if defined(_MSC_VER)
#    define BAM_RESTRICT __restrict
#  else
#    define BAM_RESTRICT
#  endif
#endif

namespace bam {

namespace offsets {

inline constexpr std::size_t SLOT_COUNT = BAM_HOST_API_SLOT_COUNT;
inline constexpr std::size_t STRUCT_SIZE = offsetof(BamHostApi, struct_size);
inline constexpr std::size_t ABI_VERSION = offsetof(BamHostApi, abi_version);

inline constexpr std::size_t GET_PROC_ADDRESS = BAM_HOST_API_OFFSET_GET_PROC_ADDRESS;
inline constexpr std::size_t LAST_ERROR = BAM_HOST_API_OFFSET_LAST_ERROR;
inline constexpr std::size_t LOAD_HOST_CTX = BAM_HOST_API_OFFSET_LOAD_HOST_CTX;
inline constexpr std::size_t REGISTER_COMPONENT = BAM_HOST_API_OFFSET_REGISTER_COMPONENT;
inline constexpr std::size_t RESOLVE_COMPONENT = BAM_HOST_API_OFFSET_RESOLVE_COMPONENT;
inline constexpr std::size_t REGISTER_SYSTEM = BAM_HOST_API_OFFSET_REGISTER_SYSTEM;
inline constexpr std::size_t SYS_HOST_CTX = BAM_HOST_API_OFFSET_SYS_HOST_CTX;
inline constexpr std::size_t SYS_QUERY = BAM_HOST_API_OFFSET_SYS_QUERY;
inline constexpr std::size_t SYS_COMMANDS = BAM_HOST_API_OFFSET_SYS_COMMANDS;
inline constexpr std::size_t ITER_NEXT_CHUNK = BAM_HOST_API_OFFSET_ITER_NEXT_CHUNK;
inline constexpr std::size_t ITER_RESET = BAM_HOST_API_OFFSET_ITER_RESET;
inline constexpr std::size_t ITER_ENTITIES = BAM_HOST_API_OFFSET_ITER_ENTITIES;
inline constexpr std::size_t ITER_COLUMN_READONLY = BAM_HOST_API_OFFSET_ITER_COLUMN_READONLY;
inline constexpr std::size_t ITER_COLUMN_MUTABLE = BAM_HOST_API_OFFSET_ITER_COLUMN_MUTABLE;
inline constexpr std::size_t ITER_COLUMN_MUTABLE_UNTRACKED = BAM_HOST_API_OFFSET_ITER_COLUMN_MUTABLE_UNTRACKED;
inline constexpr std::size_t ITER_MARK_CHANGED_RANGE = BAM_HOST_API_OFFSET_ITER_MARK_CHANGED_RANGE;
inline constexpr std::size_t ITER_COLUMN_ROWS_READONLY = BAM_HOST_API_OFFSET_ITER_COLUMN_ROWS_READONLY;
inline constexpr std::size_t ITER_COLUMN_ROWS_MUTABLE = BAM_HOST_API_OFFSET_ITER_COLUMN_ROWS_MUTABLE;
inline constexpr std::size_t ITER_COLUMN_INFO = BAM_HOST_API_OFFSET_ITER_COLUMN_INFO;
inline constexpr std::size_t RES_PTR = BAM_HOST_API_OFFSET_RES_PTR;
inline constexpr std::size_t RES_MUT_PTR = BAM_HOST_API_OFFSET_RES_MUT_PTR;
inline constexpr std::size_t CMD_SPAWN_EMPTY = BAM_HOST_API_OFFSET_CMD_SPAWN_EMPTY;
inline constexpr std::size_t CMD_SPAWN = BAM_HOST_API_OFFSET_CMD_SPAWN;
inline constexpr std::size_t CMD_DESPAWN = BAM_HOST_API_OFFSET_CMD_DESPAWN;
inline constexpr std::size_t CMD_INSERT = BAM_HOST_API_OFFSET_CMD_INSERT;
inline constexpr std::size_t CMD_INSERT_DEFAULT = BAM_HOST_API_OFFSET_CMD_INSERT_DEFAULT;
inline constexpr std::size_t CMD_REMOVE = BAM_HOST_API_OFFSET_CMD_REMOVE;
inline constexpr std::size_t PAR_FOR_EACH_CHUNK = BAM_HOST_API_OFFSET_PAR_FOR_EACH_CHUNK;
inline constexpr std::size_t SYS_ENTITY_GET = BAM_HOST_API_OFFSET_SYS_ENTITY_GET;
inline constexpr std::size_t SYS_ENTITY_GET_MUT = BAM_HOST_API_OFFSET_SYS_ENTITY_GET_MUT;
inline constexpr std::size_t MSG_READ_BATCH = BAM_HOST_API_OFFSET_MSG_READ_BATCH;
inline constexpr std::size_t MSG_SEND = BAM_HOST_API_OFFSET_MSG_SEND;
inline constexpr std::size_t MSG_WRITE_BATCH = BAM_HOST_API_OFFSET_MSG_WRITE_BATCH;
inline constexpr std::size_t RES_TICKS = BAM_HOST_API_OFFSET_RES_TICKS;
inline constexpr std::size_t SYS_RES_PTR = BAM_HOST_API_OFFSET_SYS_RES_PTR;
inline constexpr std::size_t SYS_RES_MUT_PTR = BAM_HOST_API_OFFSET_SYS_RES_MUT_PTR;
inline constexpr std::size_t REGISTER_OBSERVER = BAM_HOST_API_OFFSET_REGISTER_OBSERVER;
inline constexpr std::size_t CMD_TRIGGER = BAM_HOST_API_OFFSET_CMD_TRIGGER;
inline constexpr std::size_t CMD_SPAWN_PERSISTENT = BAM_HOST_API_OFFSET_CMD_SPAWN_PERSISTENT;
inline constexpr std::size_t CMD_SPAWN_BATCH = BAM_HOST_API_OFFSET_CMD_SPAWN_BATCH;
inline constexpr std::size_t CMD_DESPAWN_ALL = BAM_HOST_API_OFFSET_CMD_DESPAWN_ALL;
inline constexpr std::size_t CMD_INSERT_IF_NEW = BAM_HOST_API_OFFSET_CMD_INSERT_IF_NEW;
inline constexpr std::size_t CMD_TRIGGER_EX = BAM_HOST_API_OFFSET_CMD_TRIGGER_EX;
inline constexpr std::size_t CMD_OBSERVE = BAM_HOST_API_OFFSET_CMD_OBSERVE;
inline constexpr std::size_t ITER_READ_REMOVED = BAM_HOST_API_OFFSET_ITER_READ_REMOVED;
inline constexpr std::size_t SYS_ENTITY_CHILDREN = BAM_HOST_API_OFFSET_SYS_ENTITY_CHILDREN;
inline constexpr std::size_t ASSET_LOAD = BAM_HOST_API_OFFSET_ASSET_LOAD;
inline constexpr std::size_t ASSET_GET_LOAD_STATE = BAM_HOST_API_OFFSET_ASSET_GET_LOAD_STATE;
inline constexpr std::size_t ASSET_RELEASE = BAM_HOST_API_OFFSET_ASSET_RELEASE;
inline constexpr std::size_t CMD_SPAWN_SCENE = BAM_HOST_API_OFFSET_CMD_SPAWN_SCENE;
inline constexpr std::size_t LOG_DIAGNOSTIC = BAM_HOST_API_OFFSET_LOG_DIAGNOSTIC;
inline constexpr std::size_t REPORT_GUEST_FAULT = BAM_HOST_API_OFFSET_REPORT_GUEST_FAULT;
inline constexpr std::size_t RESOLVE_COMPONENT_BY_UUID = BAM_HOST_API_OFFSET_RESOLVE_COMPONENT_BY_UUID;
inline constexpr std::size_t COMPONENT_LAYOUT_INFO = BAM_HOST_API_OFFSET_COMPONENT_LAYOUT_INFO;
inline constexpr std::size_t REGISTER_RELATION = BAM_HOST_API_OFFSET_REGISTER_RELATION;
inline constexpr std::size_t INTERN_SYMBOL = BAM_HOST_API_OFFSET_INTERN_SYMBOL;
inline constexpr std::size_t BIND_NAMED_OBJECT = BAM_HOST_API_OFFSET_BIND_NAMED_OBJECT;
inline constexpr std::size_t ITER_COUNT = BAM_HOST_API_OFFSET_ITER_COUNT;
inline constexpr std::size_t ITER_UP_CASCADE = BAM_HOST_API_OFFSET_ITER_UP_CASCADE;
inline constexpr std::size_t PAR_FOR_EACH_CHUNK_READONLY = BAM_HOST_API_OFFSET_PAR_FOR_EACH_CHUNK_READONLY;
inline constexpr std::size_t REGISTER_SYSTEM_HANDLE = BAM_HOST_API_OFFSET_REGISTER_SYSTEM_HANDLE;
inline constexpr std::size_t CONFIGURE_SCHEDULE_SET = BAM_HOST_API_OFFSET_CONFIGURE_SCHEDULE_SET;
inline constexpr std::size_t MOUNT_SYSTEM_IN_SCHEDULE = BAM_HOST_API_OFFSET_MOUNT_SYSTEM_IN_SCHEDULE;
inline constexpr std::size_t CMD_CHECKPOINT = BAM_HOST_API_OFFSET_CMD_CHECKPOINT;
inline constexpr std::size_t CMD_ROLLBACK = BAM_HOST_API_OFFSET_CMD_ROLLBACK;
inline constexpr std::size_t CMD_INSERT_RELATION = BAM_HOST_API_OFFSET_CMD_INSERT_RELATION;
inline constexpr std::size_t CMD_REMOVE_RELATION = BAM_HOST_API_OFFSET_CMD_REMOVE_RELATION;
inline constexpr std::size_t MIGRATE_COMPONENT_COLUMN = BAM_HOST_API_OFFSET_MIGRATE_COMPONENT_COLUMN;
inline constexpr std::size_t ITER_NEXT_CHUNK_BATCH = BAM_HOST_API_OFFSET_ITER_NEXT_CHUNK_BATCH;

static_assert(STRUCT_SIZE == 0u, "BamHostApi::struct_size must be at offset 0");
static_assert(ABI_VERSION == 4u, "BamHostApi::abi_version must be at offset 4");
static_assert(GET_PROC_ADDRESS == 8u, "BamHostApi slot 0 must be at offset 8");
static_assert(
    ITER_NEXT_CHUNK_BATCH + sizeof(BamIterNextChunkBatchFn) == sizeof(BamHostApi),
    "BamHostApi tail slot must end at sizeof(BamHostApi)"
);

} // namespace offsets

inline bool is_ok(BamResult res) noexcept {
    return res == BAM_RESULT_OK;
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
        return api_.asset_load(ctx_, asset_type, path, &out_handle);
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
        return api_.log_diagnostic(ctx_, level, msg);
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
        return api_.report_guest_fault(ctx_, msg);
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
        return api_.intern_symbol(ctx_, symbol, &out);
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
        return api_.bind_named_object(ctx_, name, entity);
    }

    BamResult bind_named_object(const char* name, BamEntity entity) const noexcept {
        return bind_named_object(bam_str_view(name), entity);
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

    bool next_chunk_batch(
        size_t& out_len,
        const BamEntity** out_entities,
        uint8_t** out_col_ptrs,
        size_t col_count,
        uint32_t mark_changed_mask
    ) noexcept {
        if (api_ == nullptr || iter_ == nullptr ||
            !bam_host_api_covers_slot(api_, BAM_HOST_API_OFFSET_ITER_NEXT_CHUNK_BATCH, sizeof(BamIterNextChunkBatchFn)) ||
            api_->iter_next_chunk_batch == nullptr) {
            count_ = 0u;
            out_len = 0u;
            return false;
        }
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

private:
    const BamHostApi& api_;
    BamSystemCtx* ctx_;
};

} // namespace bam

#endif // BAMABI_HPP
"#;

/// Zig SDK wrapper over `bamabi.h` (`ABI 0.3.0` `b.addTranslateC` `"bam_c"` module).
pub const ZIG_SDK: &str = r#"//! Zig SDK bindings for `bamabi` (`ABI 0.3.0`).
//!
//! Expects `build.zig` to translate `bamabi.h` via `b.addTranslateC(...)`
//! and attach the resulting module as `"bam_c"` (`mod.addImport("bam_c", bam_c_mod)`).

pub const c = @import("bam_c");

/// Zig expresses C99/MSVC `BAM_RESTRICT` non-aliasing guarantees on SoA column pointers via the
/// built-in `noalias` parameter qualifier (for example:
/// `fn integrate(noalias pos: [*]Pos, noalias vel: [*]const Vel, len: usize) void`).
pub const BAM_RESTRICT_QUALIFIER = "noalias";

/// Compile-time byte offsets for `c.BamHostApi` header fields and all 68 function-pointer slots,
/// mirroring `bamabi_sys::offsets` and `BAM_HOST_API_OFFSET_*` via `@offsetOf`.
pub const offsets = struct {
    pub const SLOT_COUNT: usize = 68;
    pub const STRUCT_SIZE: usize = @offsetOf(c.BamHostApi, "struct_size");
    pub const ABI_VERSION: usize = @offsetOf(c.BamHostApi, "abi_version");

    pub const GET_PROC_ADDRESS: usize = @offsetOf(c.BamHostApi, "get_proc_address");
    pub const LAST_ERROR: usize = @offsetOf(c.BamHostApi, "last_error");
    pub const LOAD_HOST_CTX: usize = @offsetOf(c.BamHostApi, "load_host_ctx");
    pub const REGISTER_COMPONENT: usize = @offsetOf(c.BamHostApi, "register_component");
    pub const RESOLVE_COMPONENT: usize = @offsetOf(c.BamHostApi, "resolve_component");
    pub const REGISTER_SYSTEM: usize = @offsetOf(c.BamHostApi, "register_system");
    pub const SYS_HOST_CTX: usize = @offsetOf(c.BamHostApi, "sys_host_ctx");
    pub const SYS_QUERY: usize = @offsetOf(c.BamHostApi, "sys_query");
    pub const SYS_COMMANDS: usize = @offsetOf(c.BamHostApi, "sys_commands");
    pub const ITER_NEXT_CHUNK: usize = @offsetOf(c.BamHostApi, "iter_next_chunk");
    pub const ITER_RESET: usize = @offsetOf(c.BamHostApi, "iter_reset");
    pub const ITER_ENTITIES: usize = @offsetOf(c.BamHostApi, "iter_entities");
    pub const ITER_COLUMN_READONLY: usize = @offsetOf(c.BamHostApi, "iter_column_readonly");
    pub const ITER_COLUMN_MUTABLE: usize = @offsetOf(c.BamHostApi, "iter_column_mutable");
    pub const ITER_COLUMN_MUTABLE_UNTRACKED: usize = @offsetOf(c.BamHostApi, "iter_column_mutable_untracked");
    pub const ITER_MARK_CHANGED_RANGE: usize = @offsetOf(c.BamHostApi, "iter_mark_changed_range");
    pub const ITER_COLUMN_ROWS_READONLY: usize = @offsetOf(c.BamHostApi, "iter_column_rows_readonly");
    pub const ITER_COLUMN_ROWS_MUTABLE: usize = @offsetOf(c.BamHostApi, "iter_column_rows_mutable");
    pub const ITER_COLUMN_INFO: usize = @offsetOf(c.BamHostApi, "iter_column_info");
    pub const RES_PTR: usize = @offsetOf(c.BamHostApi, "res_ptr");
    pub const RES_MUT_PTR: usize = @offsetOf(c.BamHostApi, "res_mut_ptr");
    pub const CMD_SPAWN_EMPTY: usize = @offsetOf(c.BamHostApi, "cmd_spawn_empty");
    pub const CMD_SPAWN: usize = @offsetOf(c.BamHostApi, "cmd_spawn");
    pub const CMD_DESPAWN: usize = @offsetOf(c.BamHostApi, "cmd_despawn");
    pub const CMD_INSERT: usize = @offsetOf(c.BamHostApi, "cmd_insert");
    pub const CMD_INSERT_DEFAULT: usize = @offsetOf(c.BamHostApi, "cmd_insert_default");
    pub const CMD_REMOVE: usize = @offsetOf(c.BamHostApi, "cmd_remove");
    pub const PAR_FOR_EACH_CHUNK: usize = @offsetOf(c.BamHostApi, "par_for_each_chunk");
    pub const SYS_ENTITY_GET: usize = @offsetOf(c.BamHostApi, "sys_entity_get");
    pub const SYS_ENTITY_GET_MUT: usize = @offsetOf(c.BamHostApi, "sys_entity_get_mut");
    pub const MSG_READ_BATCH: usize = @offsetOf(c.BamHostApi, "msg_read_batch");
    pub const MSG_SEND: usize = @offsetOf(c.BamHostApi, "msg_send");
    pub const MSG_WRITE_BATCH: usize = @offsetOf(c.BamHostApi, "msg_write_batch");
    pub const RES_TICKS: usize = @offsetOf(c.BamHostApi, "res_ticks");
    pub const SYS_RES_PTR: usize = @offsetOf(c.BamHostApi, "sys_res_ptr");
    pub const SYS_RES_MUT_PTR: usize = @offsetOf(c.BamHostApi, "sys_res_mut_ptr");
    pub const REGISTER_OBSERVER: usize = @offsetOf(c.BamHostApi, "register_observer");
    pub const CMD_TRIGGER: usize = @offsetOf(c.BamHostApi, "cmd_trigger");
    pub const CMD_SPAWN_PERSISTENT: usize = @offsetOf(c.BamHostApi, "cmd_spawn_persistent");
    pub const CMD_SPAWN_BATCH: usize = @offsetOf(c.BamHostApi, "cmd_spawn_batch");
    pub const CMD_DESPAWN_ALL: usize = @offsetOf(c.BamHostApi, "cmd_despawn_all");
    pub const CMD_INSERT_IF_NEW: usize = @offsetOf(c.BamHostApi, "cmd_insert_if_new");
    pub const CMD_TRIGGER_EX: usize = @offsetOf(c.BamHostApi, "cmd_trigger_ex");
    pub const CMD_OBSERVE: usize = @offsetOf(c.BamHostApi, "cmd_observe");
    pub const ITER_READ_REMOVED: usize = @offsetOf(c.BamHostApi, "iter_read_removed");
    pub const SYS_ENTITY_CHILDREN: usize = @offsetOf(c.BamHostApi, "sys_entity_children");
    pub const ASSET_LOAD: usize = @offsetOf(c.BamHostApi, "asset_load");
    pub const ASSET_GET_LOAD_STATE: usize = @offsetOf(c.BamHostApi, "asset_get_load_state");
    pub const ASSET_RELEASE: usize = @offsetOf(c.BamHostApi, "asset_release");
    pub const CMD_SPAWN_SCENE: usize = @offsetOf(c.BamHostApi, "cmd_spawn_scene");
    pub const LOG_DIAGNOSTIC: usize = @offsetOf(c.BamHostApi, "log_diagnostic");
    pub const REPORT_GUEST_FAULT: usize = @offsetOf(c.BamHostApi, "report_guest_fault");
    pub const RESOLVE_COMPONENT_BY_UUID: usize = @offsetOf(c.BamHostApi, "resolve_component_by_uuid");
    pub const COMPONENT_LAYOUT_INFO: usize = @offsetOf(c.BamHostApi, "component_layout_info");
    pub const REGISTER_RELATION: usize = @offsetOf(c.BamHostApi, "register_relation");
    pub const INTERN_SYMBOL: usize = @offsetOf(c.BamHostApi, "intern_symbol");
    pub const BIND_NAMED_OBJECT: usize = @offsetOf(c.BamHostApi, "bind_named_object");
    pub const ITER_COUNT: usize = @offsetOf(c.BamHostApi, "iter_count");
    pub const ITER_UP_CASCADE: usize = @offsetOf(c.BamHostApi, "iter_up_cascade");
    pub const PAR_FOR_EACH_CHUNK_READONLY: usize = @offsetOf(c.BamHostApi, "par_for_each_chunk_readonly");
    pub const REGISTER_SYSTEM_HANDLE: usize = @offsetOf(c.BamHostApi, "register_system_handle");
    pub const CONFIGURE_SCHEDULE_SET: usize = @offsetOf(c.BamHostApi, "configure_schedule_set");
    pub const MOUNT_SYSTEM_IN_SCHEDULE: usize = @offsetOf(c.BamHostApi, "mount_system_in_schedule");
    pub const CMD_CHECKPOINT: usize = @offsetOf(c.BamHostApi, "cmd_checkpoint");
    pub const CMD_ROLLBACK: usize = @offsetOf(c.BamHostApi, "cmd_rollback");
    pub const CMD_INSERT_RELATION: usize = @offsetOf(c.BamHostApi, "cmd_insert_relation");
    pub const CMD_REMOVE_RELATION: usize = @offsetOf(c.BamHostApi, "cmd_remove_relation");
    pub const MIGRATE_COMPONENT_COLUMN: usize = @offsetOf(c.BamHostApi, "migrate_component_column");
    pub const ITER_NEXT_CHUNK_BATCH: usize = @offsetOf(c.BamHostApi, "iter_next_chunk_batch");
};

pub const BAM_HOST_API_OFFSET_GET_PROC_ADDRESS: usize = offsets.GET_PROC_ADDRESS;
pub const BAM_HOST_API_OFFSET_LAST_ERROR: usize = offsets.LAST_ERROR;
pub const BAM_HOST_API_OFFSET_LOAD_HOST_CTX: usize = offsets.LOAD_HOST_CTX;
pub const BAM_HOST_API_OFFSET_REGISTER_COMPONENT: usize = offsets.REGISTER_COMPONENT;
pub const BAM_HOST_API_OFFSET_RESOLVE_COMPONENT: usize = offsets.RESOLVE_COMPONENT;
pub const BAM_HOST_API_OFFSET_REGISTER_SYSTEM: usize = offsets.REGISTER_SYSTEM;
pub const BAM_HOST_API_OFFSET_SYS_HOST_CTX: usize = offsets.SYS_HOST_CTX;
pub const BAM_HOST_API_OFFSET_SYS_QUERY: usize = offsets.SYS_QUERY;
pub const BAM_HOST_API_OFFSET_SYS_COMMANDS: usize = offsets.SYS_COMMANDS;
pub const BAM_HOST_API_OFFSET_ITER_NEXT_CHUNK: usize = offsets.ITER_NEXT_CHUNK;
pub const BAM_HOST_API_OFFSET_ITER_RESET: usize = offsets.ITER_RESET;
pub const BAM_HOST_API_OFFSET_ITER_ENTITIES: usize = offsets.ITER_ENTITIES;
pub const BAM_HOST_API_OFFSET_ITER_COLUMN_READONLY: usize = offsets.ITER_COLUMN_READONLY;
pub const BAM_HOST_API_OFFSET_ITER_COLUMN_MUTABLE: usize = offsets.ITER_COLUMN_MUTABLE;
pub const BAM_HOST_API_OFFSET_ITER_COLUMN_MUTABLE_UNTRACKED: usize = offsets.ITER_COLUMN_MUTABLE_UNTRACKED;
pub const BAM_HOST_API_OFFSET_ITER_MARK_CHANGED_RANGE: usize = offsets.ITER_MARK_CHANGED_RANGE;
pub const BAM_HOST_API_OFFSET_ITER_COLUMN_ROWS_READONLY: usize = offsets.ITER_COLUMN_ROWS_READONLY;
pub const BAM_HOST_API_OFFSET_ITER_COLUMN_ROWS_MUTABLE: usize = offsets.ITER_COLUMN_ROWS_MUTABLE;
pub const BAM_HOST_API_OFFSET_ITER_COLUMN_INFO: usize = offsets.ITER_COLUMN_INFO;
pub const BAM_HOST_API_OFFSET_RES_PTR: usize = offsets.RES_PTR;
pub const BAM_HOST_API_OFFSET_RES_MUT_PTR: usize = offsets.RES_MUT_PTR;
pub const BAM_HOST_API_OFFSET_CMD_SPAWN_EMPTY: usize = offsets.CMD_SPAWN_EMPTY;
pub const BAM_HOST_API_OFFSET_CMD_SPAWN: usize = offsets.CMD_SPAWN;
pub const BAM_HOST_API_OFFSET_CMD_DESPAWN: usize = offsets.CMD_DESPAWN;
pub const BAM_HOST_API_OFFSET_CMD_INSERT: usize = offsets.CMD_INSERT;
pub const BAM_HOST_API_OFFSET_CMD_INSERT_DEFAULT: usize = offsets.CMD_INSERT_DEFAULT;
pub const BAM_HOST_API_OFFSET_CMD_REMOVE: usize = offsets.CMD_REMOVE;
pub const BAM_HOST_API_OFFSET_PAR_FOR_EACH_CHUNK: usize = offsets.PAR_FOR_EACH_CHUNK;
pub const BAM_HOST_API_OFFSET_SYS_ENTITY_GET: usize = offsets.SYS_ENTITY_GET;
pub const BAM_HOST_API_OFFSET_SYS_ENTITY_GET_MUT: usize = offsets.SYS_ENTITY_GET_MUT;
pub const BAM_HOST_API_OFFSET_MSG_READ_BATCH: usize = offsets.MSG_READ_BATCH;
pub const BAM_HOST_API_OFFSET_MSG_SEND: usize = offsets.MSG_SEND;
pub const BAM_HOST_API_OFFSET_MSG_WRITE_BATCH: usize = offsets.MSG_WRITE_BATCH;
pub const BAM_HOST_API_OFFSET_RES_TICKS: usize = offsets.RES_TICKS;
pub const BAM_HOST_API_OFFSET_SYS_RES_PTR: usize = offsets.SYS_RES_PTR;
pub const BAM_HOST_API_OFFSET_SYS_RES_MUT_PTR: usize = offsets.SYS_RES_MUT_PTR;
pub const BAM_HOST_API_OFFSET_REGISTER_OBSERVER: usize = offsets.REGISTER_OBSERVER;
pub const BAM_HOST_API_OFFSET_CMD_TRIGGER: usize = offsets.CMD_TRIGGER;
pub const BAM_HOST_API_OFFSET_CMD_SPAWN_PERSISTENT: usize = offsets.CMD_SPAWN_PERSISTENT;
pub const BAM_HOST_API_OFFSET_CMD_SPAWN_BATCH: usize = offsets.CMD_SPAWN_BATCH;
pub const BAM_HOST_API_OFFSET_CMD_DESPAWN_ALL: usize = offsets.CMD_DESPAWN_ALL;
pub const BAM_HOST_API_OFFSET_CMD_INSERT_IF_NEW: usize = offsets.CMD_INSERT_IF_NEW;
pub const BAM_HOST_API_OFFSET_CMD_TRIGGER_EX: usize = offsets.CMD_TRIGGER_EX;
pub const BAM_HOST_API_OFFSET_CMD_OBSERVE: usize = offsets.CMD_OBSERVE;
pub const BAM_HOST_API_OFFSET_ITER_READ_REMOVED: usize = offsets.ITER_READ_REMOVED;
pub const BAM_HOST_API_OFFSET_SYS_ENTITY_CHILDREN: usize = offsets.SYS_ENTITY_CHILDREN;
pub const BAM_HOST_API_OFFSET_ASSET_LOAD: usize = offsets.ASSET_LOAD;
pub const BAM_HOST_API_OFFSET_ASSET_GET_LOAD_STATE: usize = offsets.ASSET_GET_LOAD_STATE;
pub const BAM_HOST_API_OFFSET_ASSET_RELEASE: usize = offsets.ASSET_RELEASE;
pub const BAM_HOST_API_OFFSET_CMD_SPAWN_SCENE: usize = offsets.CMD_SPAWN_SCENE;
pub const BAM_HOST_API_OFFSET_LOG_DIAGNOSTIC: usize = offsets.LOG_DIAGNOSTIC;
pub const BAM_HOST_API_OFFSET_REPORT_GUEST_FAULT: usize = offsets.REPORT_GUEST_FAULT;
pub const BAM_HOST_API_OFFSET_RESOLVE_COMPONENT_BY_UUID: usize = offsets.RESOLVE_COMPONENT_BY_UUID;
pub const BAM_HOST_API_OFFSET_COMPONENT_LAYOUT_INFO: usize = offsets.COMPONENT_LAYOUT_INFO;
pub const BAM_HOST_API_OFFSET_REGISTER_RELATION: usize = offsets.REGISTER_RELATION;
pub const BAM_HOST_API_OFFSET_INTERN_SYMBOL: usize = offsets.INTERN_SYMBOL;
pub const BAM_HOST_API_OFFSET_BIND_NAMED_OBJECT: usize = offsets.BIND_NAMED_OBJECT;
pub const BAM_HOST_API_OFFSET_ITER_COUNT: usize = offsets.ITER_COUNT;
pub const BAM_HOST_API_OFFSET_ITER_UP_CASCADE: usize = offsets.ITER_UP_CASCADE;
pub const BAM_HOST_API_OFFSET_PAR_FOR_EACH_CHUNK_READONLY: usize = offsets.PAR_FOR_EACH_CHUNK_READONLY;
pub const BAM_HOST_API_OFFSET_REGISTER_SYSTEM_HANDLE: usize = offsets.REGISTER_SYSTEM_HANDLE;
pub const BAM_HOST_API_OFFSET_CONFIGURE_SCHEDULE_SET: usize = offsets.CONFIGURE_SCHEDULE_SET;
pub const BAM_HOST_API_OFFSET_MOUNT_SYSTEM_IN_SCHEDULE: usize = offsets.MOUNT_SYSTEM_IN_SCHEDULE;
pub const BAM_HOST_API_OFFSET_CMD_CHECKPOINT: usize = offsets.CMD_CHECKPOINT;
pub const BAM_HOST_API_OFFSET_CMD_ROLLBACK: usize = offsets.CMD_ROLLBACK;
pub const BAM_HOST_API_OFFSET_CMD_INSERT_RELATION: usize = offsets.CMD_INSERT_RELATION;
pub const BAM_HOST_API_OFFSET_CMD_REMOVE_RELATION: usize = offsets.CMD_REMOVE_RELATION;
pub const BAM_HOST_API_OFFSET_MIGRATE_COMPONENT_COLUMN: usize = offsets.MIGRATE_COMPONENT_COLUMN;
pub const BAM_HOST_API_OFFSET_ITER_NEXT_CHUNK_BATCH: usize = offsets.ITER_NEXT_CHUNK_BATCH;

comptime {
    if (offsets.STRUCT_SIZE != 0 or offsets.ABI_VERSION != 4 or offsets.GET_PROC_ADDRESS != 8) {
        @compileError("BamHostApi header offsets mismatch");
    }
    if (offsets.ITER_NEXT_CHUNK_BATCH + @sizeOf(usize) != @sizeOf(c.BamHostApi)) {
        @compileError("BamHostApi tail slot offset mismatch");
    }
}

/// Returns `true` when `api.struct_size` covers a slot of `slot_size` bytes at `slot_offset`.
pub inline fn coversHostApiSlot(api: *const c.BamHostApi, slot_offset: usize, slot_size: usize) bool {
    const sz: usize = @as(usize, api.struct_size);
    return slot_offset <= sz and slot_size <= (sz - slot_offset);
}

/// Copies `len` elements between non-overlapping SoA columns using Zig's `noalias`
/// parameter qualifier (the Zig equivalent of C99/MSVC `BAM_RESTRICT`).
pub fn copyColumnNoAlias(
    comptime T: type,
    noalias dst: [*]T,
    noalias src: [*]const T,
    len: usize,
) void {
    @memcpy(dst[0..len], src[0..len]);
}

pub const BamError = error{
    InvalidComponent,
    AccessConflict,
    PanicCaught,
    NullPointer,
    InvalidUtf8,
    PermissionDenied,
    InvalidVersion,
    InvalidArgument,
    OutOfRange,
    NotFound,
    InvalidState,
    InvalidEntity,
    CapacityExceeded,
    Unsupported,
    TypeMismatch,
    CycleDetected,
    OutOfMemory,
    InternalError,
};

pub fn checkResult(res: c.BamResult) BamError!void {
    return switch (res) {
        c.BAM_RESULT_OK => {},
        c.BAM_RESULT_INVALID_COMPONENT => error.InvalidComponent,
        c.BAM_RESULT_ACCESS_CONFLICT => error.AccessConflict,
        c.BAM_RESULT_PANIC_CAUGHT => error.PanicCaught,
        c.BAM_RESULT_NULL_POINTER => error.NullPointer,
        c.BAM_RESULT_INVALID_UTF8 => error.InvalidUtf8,
        c.BAM_RESULT_PERMISSION_DENIED => error.PermissionDenied,
        c.BAM_RESULT_INVALID_VERSION => error.InvalidVersion,
        c.BAM_RESULT_INVALID_ARGUMENT => error.InvalidArgument,
        c.BAM_RESULT_OUT_OF_RANGE => error.OutOfRange,
        c.BAM_RESULT_NOT_FOUND => error.NotFound,
        c.BAM_RESULT_INVALID_STATE => error.InvalidState,
        c.BAM_RESULT_INVALID_ENTITY => error.InvalidEntity,
        c.BAM_RESULT_CAPACITY_EXCEEDED => error.CapacityExceeded,
        c.BAM_RESULT_UNSUPPORTED => error.Unsupported,
        c.BAM_RESULT_TYPE_MISMATCH => error.TypeMismatch,
        c.BAM_RESULT_CYCLE_DETECTED => error.CycleDetected,
        c.BAM_RESULT_INTERNAL_ERROR => error.InternalError,
        else => error.InternalError,
    };
}

pub fn mapError(err: BamError) c.BamResult {
    return switch (err) {
        error.InvalidComponent => c.BAM_RESULT_INVALID_COMPONENT,
        error.AccessConflict => c.BAM_RESULT_ACCESS_CONFLICT,
        error.PanicCaught => c.BAM_RESULT_PANIC_CAUGHT,
        error.NullPointer => c.BAM_RESULT_NULL_POINTER,
        error.InvalidUtf8 => c.BAM_RESULT_INVALID_UTF8,
        error.PermissionDenied => c.BAM_RESULT_PERMISSION_DENIED,
        error.InvalidVersion => c.BAM_RESULT_INVALID_VERSION,
        error.InvalidArgument => c.BAM_RESULT_INVALID_ARGUMENT,
        error.OutOfRange => c.BAM_RESULT_OUT_OF_RANGE,
        error.NotFound => c.BAM_RESULT_NOT_FOUND,
        error.InvalidState => c.BAM_RESULT_INVALID_STATE,
        error.InvalidEntity => c.BAM_RESULT_INVALID_ENTITY,
        error.CapacityExceeded => c.BAM_RESULT_CAPACITY_EXCEEDED,
        error.Unsupported => c.BAM_RESULT_UNSUPPORTED,
        error.TypeMismatch => c.BAM_RESULT_TYPE_MISMATCH,
        error.CycleDetected => c.BAM_RESULT_CYCLE_DETECTED,
        error.OutOfMemory, error.InternalError => c.BAM_RESULT_INTERNAL_ERROR,
    };
}

pub inline fn strView(slice: []const u8) c.BamStrView {
    return .{ .ptr = slice.ptr, .len = slice.len };
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
        var out: c.BamAssetHandle = undefined;
        try checkResult(f(self.ctx, asset_type, strView(path), &out));
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
        try checkResult(f(self.ctx, level, strView(msg)));
    }

    pub fn reportGuestFault(self: HostContextView, msg: []const u8) BamError!void {
        if (!coversHostApiSlot(self.api, offsets.REPORT_GUEST_FAULT, @sizeOf(c.BamReportGuestFaultFn))) {
            return error.Unsupported;
        }
        const f = self.api.report_guest_fault orelse return error.Unsupported;
        try checkResult(f(self.ctx, strView(msg)));
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
        var out: c.BamSymbol = c.BAM_SYMBOL_INVALID;
        try checkResult(f(self.ctx, strView(symbol), &out));
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
        try checkResult(f(self.ctx, strView(name), entity));
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

    pub fn nextChunkBatch(
        self: *ChunkView,
        out_entities: ?*?[*]const c.BamEntity,
        out_col_ptrs: []?[*]u8,
        mark_changed_mask: u32,
    ) BamError!?usize {
        if (!coversHostApiSlot(self.api, offsets.ITER_NEXT_CHUNK_BATCH, @sizeOf(c.BamIterNextChunkBatchFn))) {
            return error.Unsupported;
        }
        const f = self.api.iter_next_chunk_batch orelse return error.Unsupported;
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
};
"#;

/// Exports `bamabi.h`, `bamabi.hpp`, and `bamabi.zig` into `out_dir`.
///
/// Intended purpose: equips polyglot C99, C++17, and Zig 0.16.0 guest projects (via `cargo-bam
/// export-sdk`) with a synchronized, version-matched ABI `0.3.0` binding bundle in one step.
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
    let files: [(&str, &str); 3] = [
        ("bamabi.h", &c_hdr),
        ("bamabi.hpp", CPP17_HEADER),
        ("bamabi.zig", ZIG_SDK),
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
    /// three ABI `0.3.0` SDK files (`bamabi.h`, `bamabi.hpp`, `bamabi.zig`) are written and
    /// contain required `0.3.0` symbols (`BAM_ABI_VERSION`, `BAM_RESTRICT`, `CYCLE_DETECTED`,
    /// relation/schedule-set flags, checkpoint offset macros, and Zig `addTranslateC` `"bam_c"`
    /// import without legacy `@cImport`/`@cInclude`).
    #[test]
    fn export_sdk_writes_all_three_abi_0_3_headers() {
        let tmp = std::env::temp_dir().join(format!("bam_sdk_gen_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let written = export_sdk(&tmp).expect("export_sdk must succeed");
        assert_eq!(written.len(), 3);
        let h_content = fs::read_to_string(tmp.join("bamabi.h")).unwrap();
        let hpp_content = fs::read_to_string(tmp.join("bamabi.hpp")).unwrap();
        let zig_content = fs::read_to_string(tmp.join("bamabi.zig")).unwrap();
        assert!(h_content.contains("#define BAM_ABI_VERSION 0x00030000u"));
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
        assert!(hpp_content.contains("BAM_HOST_API_OFFSET_CMD_CHECKPOINT"));
        assert!(hpp_content.contains("BAM_RESTRICT"));
        assert!(zig_content.contains("ABI 0.3.0"));
        assert!(zig_content.contains("addTranslateC"));
        assert!(zig_content.contains("@import(\"bam_c\")"));
        assert!(!zig_content.contains(concat!("@", "cImport")));
        assert!(!zig_content.contains(concat!("@", "cInclude")));
        assert!(zig_content.contains("BAM_RESTRICT_QUALIFIER"));
        assert!(zig_content.contains("BAM_RESULT_CYCLE_DETECTED"));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Test scenario: verifies cross-language offset and aliasing-qualifier parity across C99,
    /// C++17, and Zig by checking that `BAM_RESTRICT` / `noalias` and every single one of the 68
    /// `BamHostApi` vtable slot offsets (plus header offsets) are mirrored identically in
    /// `c99_header()`, [`CPP17_HEADER`], and [`ZIG_SDK`].
    #[test]
    fn all_68_host_api_offsets_and_restrict_mirrored_across_c_cpp_and_zig() {
        let h_content = c99_header();
        let slots = crate::header_gen::host_api_slot_names();
        assert_eq!(slots.len(), BAM_HOST_API_SLOT_COUNT);

        assert!(h_content.contains("#if defined(_MSC_VER)\n#  define BAM_RESTRICT __restrict\n"));
        assert!(h_content.contains(
            "#elif !defined(__cplusplus) && defined(__STDC_VERSION__) && (__STDC_VERSION__ >= 199901L)\n#  define BAM_RESTRICT restrict\n"
        ));
        assert!(CPP17_HEADER.contains("#ifndef BAM_RESTRICT"));
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

    /// Test scenario: verifies that all 68 `BamHostApi` function-pointer slots are wrapped by
    /// ergonomic view methods in both [`CPP17_HEADER`] (`HostContextView`, `LoadContextView`,
    /// `ChunkView`, `CommandsView`, `ScopedTransaction`, `SystemContextView`) and [`ZIG_SDK`],
    /// and that [`ZIG_SDK`] maps all 18 `BamResult` status codes losslessly in `checkResult` and
    /// `mapError`.
    #[test]
    fn cpp17_and_zig_views_wrap_all_68_slots_and_all_18_result_codes() {
        let slots = crate::header_gen::host_api_slot_names();
        assert_eq!(slots.len(), BAM_HOST_API_SLOT_COUNT);

        assert!(CPP17_HEADER.contains("class ScopedTransaction"));
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
