/*
 * ISO C++17 Benchmark Guest Mod (`ABI 0.4.0`)
 * Compiled with: `-std=c++17 -Wall -Wextra -Werror -Wpedantic` (`-O3` and `-O0 -g`).
 */

#include "bamabi.hpp"

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <new>

namespace {

struct BenchPosition {
    float x;
    float y;
    float z;
    float w;
};

struct BenchVelocity {
    float vx;
    float vy;
    float vz;
    float vw;
};

struct BenchSimParams {
    float    dt;
    float    damping;
    uint32_t tick;
    uint32_t _pad;
};

static_assert(sizeof(BenchPosition) == 16u, "BenchPosition must be 16 bytes");
static_assert(alignof(BenchPosition) == 4u, "BenchPosition must be 4-byte aligned");
static_assert(sizeof(BenchVelocity) == 16u, "BenchVelocity must be 16 bytes");
static_assert(alignof(BenchVelocity) == 4u, "BenchVelocity must be 4-byte aligned");
static_assert(sizeof(BenchSimParams) == 16u, "BenchSimParams must be 16 bytes");
static_assert(alignof(BenchSimParams) == 4u, "BenchSimParams must be 4-byte aligned");

const BamManifest g_manifest = {
    static_cast<uint32_t>(sizeof(BamManifest)),
    BAM_ABI_VERSION,
    "bench_guest_cpp17",
    "0.4.0",
    0u,
    0u,
    nullptr,
    nullptr
};

inline void integrate_chunk(
    BenchPosition* BAM_RESTRICT pos,
    const BenchVelocity* BAM_RESTRICT vel,
    std::size_t count
) noexcept {
    constexpr float dt = 0.016f;
    for (std::size_t i = 0u; i < count; ++i) {
        pos[i].x += vel[i].vx * dt;
        pos[i].y += vel[i].vy * dt;
        pos[i].z += vel[i].vz * dt;
        pos[i].w += vel[i].vw * dt;
    }
}

} // namespace

struct BamInstance {
    BamHostApi        api{};
    const BamHostCtx* host_ctx = nullptr;
    BamComponent      pos_comp = BAM_COMPONENT_INVALID;
    BamComponent      vel_comp = BAM_COMPONENT_INVALID;
    BamComponent      sim_comp = BAM_COMPONENT_INVALID;
    BamTermDesc       dense_terms[2]{};
    BamQueryDesc      dense_query{};
    BamTermDesc       roundtrip_terms[1]{};
    BamQueryDesc      roundtrip_query{};
    bool              has_iter_next_chunk_batch = false;
    bool              has_checkpoint_rollback = false;
    bool              has_roundtrip_slots = false;
    volatile uint64_t sink = 0u;
};

namespace {

BamResult resolve_or_register_pod(
    const bam::LoadContextView& lview,
    const bam::HostContextView& hview,
    const char* name,
    std::size_t size,
    std::size_t align,
    BamComponent& out_comp
) noexcept {
    const BamComponentUuid uuid = bam_uuid_from_parts(name, size, align, 0ull);
    BamResult rc = hview.resolve_component(name, out_comp, &uuid);
    if (rc == BAM_RESULT_NOT_FOUND) {
        const BamComponentDesc desc = bam_desc_component_with_layout(name, size, align, nullptr);
        rc = lview.register_component(desc, out_comp);
    }
    return rc;
}

extern "C" BamResult sys_noop(BamSystemCtx* sys_ctx, void* user_data) {
    (void)sys_ctx;
    (void)user_data;
    return BAM_RESULT_OK;
}

extern "C" BamResult sys_dense_update(BamSystemCtx* sys_ctx, void* user_data) {
    return bam::guard_boundary([&]() -> BamResult {
        auto* inst = static_cast<BamInstance*>(user_data);
        if (inst == nullptr || sys_ctx == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        bam::SystemContextView sview(inst->api, sys_ctx);
        bam::ChunkView cview;
        BamResult rc = sview.query_view(0u, cview);
        if (!bam::is_ok(rc)) {
            return rc;
        }
        if (!cview.valid()) {
            return BAM_RESULT_NULL_POINTER;
        }
        std::size_t chunk_len = 0u;
        if (inst->has_iter_next_chunk_batch) {
            uint8_t* col_ptrs[2] = { nullptr, nullptr };
            while (cview.next_chunk_batch_unchecked(chunk_len, nullptr, col_ptrs, 2u, 1u)) {
                if (chunk_len == 0u) {
                    continue;
                }
                if (col_ptrs[0] == nullptr || col_ptrs[1] == nullptr) {
                    return BAM_RESULT_NULL_POINTER;
                }
                auto* pos = reinterpret_cast<BenchPosition*>(col_ptrs[0]);
                const auto* vel = reinterpret_cast<const BenchVelocity*>(col_ptrs[1]);
                integrate_chunk(pos, vel, chunk_len);
            }
        } else {
            while (cview.next_chunk(chunk_len)) {
                if (chunk_len == 0u) {
                    continue;
                }
                auto* pos = cview.column_mutable<BenchPosition>(0u);
                const auto* vel = cview.column_readonly<BenchVelocity>(1u);
                if (pos == nullptr || vel == nullptr) {
                    return BAM_RESULT_NULL_POINTER;
                }
                integrate_chunk(pos, vel, chunk_len);
            }
        }
        return BAM_RESULT_OK;
    });
}

extern "C" BamResult sys_command_burst(BamSystemCtx* sys_ctx, void* user_data) {
    return bam::guard_boundary([&]() -> BamResult {
        auto* inst = static_cast<BamInstance*>(user_data);
        if (inst == nullptr || sys_ctx == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        if (!inst->has_checkpoint_rollback) {
            return BAM_RESULT_UNSUPPORTED;
        }
        bam::SystemContextView sview(inst->api, sys_ctx);
        bam::CommandsView cmds;
        BamResult rc = sview.commands_view(cmds);
        if (!bam::is_ok(rc)) {
            return rc;
        }

        constexpr BenchPosition sample_pos{1.0f, 2.0f, 3.0f, 4.0f};
        for (std::size_t i = 0u; i < 64u; ++i) {
            BamEntity ent = BAM_ENTITY_NULL;
            rc = cmds.spawn_empty(ent);
            if (!bam::is_ok(rc)) {
                return rc;
            }
            rc = cmds.insert_pod(ent, inst->pos_comp, sample_pos);
            if (!bam::is_ok(rc)) {
                return rc;
            }
        }

        std::size_t checkpoint = 0u;
        rc = inst->api.cmd_checkpoint(cmds.raw(), &checkpoint);
        if (!bam::is_ok(rc)) {
            return rc;
        }
        BamEntity rolled_entity = BAM_ENTITY_NULL;
        rc = cmds.spawn_empty(rolled_entity);
        if (!bam::is_ok(rc)) {
            (void)inst->api.cmd_rollback(cmds.raw(), checkpoint);
            return rc;
        }
        return inst->api.cmd_rollback(cmds.raw(), checkpoint);
    });
}

extern "C" BamResult sys_host_api_roundtrip(BamSystemCtx* sys_ctx, void* user_data) {
    return bam::guard_boundary([&]() -> BamResult {
        auto* inst = static_cast<BamInstance*>(user_data);
        if (inst == nullptr || sys_ctx == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        if (!inst->has_roundtrip_slots) {
            return BAM_RESULT_UNSUPPORTED;
        }
        bam::SystemContextView sview(inst->api, sys_ctx);
        bam::ChunkView cview;
        BamResult rc = sview.query_view(0u, cview);
        if (!bam::is_ok(rc)) {
            return rc;
        }
        bam::HostContextView sys_hview = sview.host_ctx();
        bam::HostContextView hview(inst->api, sys_hview.valid() ? sys_hview.raw() : inst->host_ctx);
        if (!cview.valid() || !hview.valid() || inst->api.res_ptr == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }

        const BamStrView sym_name = bam_str_view_len(reinterpret_cast<const uint8_t*>("bench.sym"), 9u);
        uint64_t acc = 0u;
        for (std::size_t i = 0u; i < 64u; ++i) {
            const auto* params =
                reinterpret_cast<const BenchSimParams*>(inst->api.res_ptr(cview.raw(), 0u));
            if (params == nullptr) {
                return BAM_RESULT_NOT_FOUND;
            }
            BamChangeTicks ticks{};
            rc = inst->api.res_ticks(cview.raw(), 0u, &ticks);
            if (!bam::is_ok(rc)) {
                return rc;
            }
            BamSymbol sym = BAM_SYMBOL_INVALID;
            rc = inst->api.intern_symbol(hview.raw(), &sym_name, &sym);
            if (!bam::is_ok(rc)) {
                return rc;
            }
            uint32_t dt_bits = 0u;
            std::memcpy(&dt_bits, &params->dt, sizeof(dt_bits));
            acc += static_cast<uint64_t>(ticks.this_run)
                + static_cast<uint64_t>(params->tick)
                + static_cast<uint64_t>(dt_bits)
                + static_cast<uint64_t>(sym);
        }
        inst->sink = acc;
        return BAM_RESULT_OK;
    });
}

BamResult register_bench_system(
    BamInstance* inst,
    const bam::LoadContextView& lview,
    const char* name,
    const BamQueryDesc* queries,
    std::size_t query_count,
    BamSystemFn callback
) noexcept {
    BamSystemDesc sys_desc{};
    sys_desc.struct_size = static_cast<uint32_t>(sizeof(BamSystemDesc));
    sys_desc.flags = 0u;
    sys_desc.name = name;
    sys_desc.queries = queries;
    sys_desc.query_count = query_count;
    sys_desc.callback = callback;
    sys_desc.user_data = inst;
    sys_desc.schedule = BAM_SCHEDULE_UPDATE;
    return lview.register_system(sys_desc);
}

} // namespace

BAM_EXPORT const BamManifest* bamabi_manifest(void);
BAM_EXPORT BamResult bamabi_init(const BamHostApi* api, BamLoadCtx* load_ctx, BamInstance** instance_out);
BAM_EXPORT void bamabi_shutdown(BamInstance* instance);

BAM_EXPORT const BamManifest* bamabi_manifest(void) {
    return &g_manifest;
}

BAM_EXPORT BamResult bamabi_init(
    const BamHostApi* api,
    BamLoadCtx* load_ctx,
    BamInstance** instance_out
) {
    return bam::guard_boundary([&]() -> BamResult {
        if (api == nullptr || load_ctx == nullptr || instance_out == nullptr) {
            return BAM_RESULT_NULL_POINTER;
        }
        *instance_out = nullptr;

        BamInstance* inst = new (std::nothrow) BamInstance();
        if (inst == nullptr) {
            return BAM_RESULT_INTERNAL_ERROR;
        }

        BamResult rc = bam_host_api_copy_bounded(&inst->api, sizeof(inst->api), api);
        if (!bam::is_ok(rc)) {
            delete inst;
            return rc;
        }
        if (inst->api.cmd_spawn_empty == nullptr || inst->api.res_ptr == nullptr) {
            delete inst;
            return BAM_RESULT_NULL_POINTER;
        }

        inst->has_iter_next_chunk_batch =
            bam::host_api_covers_slot(
                inst->api,
                bam::offsets::ITER_NEXT_CHUNK_BATCH,
                sizeof(BamIterNextChunkBatchFn)
            ) &&
            (inst->api.iter_next_chunk_batch != nullptr);
        inst->has_checkpoint_rollback =
            bam::host_api_covers_slot(
                inst->api,
                bam::offsets::CMD_CHECKPOINT,
                sizeof(BamCmdCheckpointFn)
            ) &&
            (inst->api.cmd_checkpoint != nullptr) &&
            bam::host_api_covers_slot(
                inst->api,
                bam::offsets::CMD_ROLLBACK,
                sizeof(BamCmdRollbackFn)
            ) &&
            (inst->api.cmd_rollback != nullptr);
        inst->has_roundtrip_slots =
            bam::host_api_covers_slot(
                inst->api,
                bam::offsets::RES_TICKS,
                sizeof(BamResTicksFn)
            ) &&
            (inst->api.res_ticks != nullptr) &&
            bam::host_api_covers_slot(
                inst->api,
                bam::offsets::INTERN_SYMBOL,
                sizeof(BamInternSymbolFn)
            ) &&
            (inst->api.intern_symbol != nullptr);

        bam::LoadContextView lview(inst->api, load_ctx);
        bam::HostContextView hview = lview.host_ctx();
        if (!hview.valid()) {
            delete inst;
            return BAM_RESULT_NULL_POINTER;
        }
        inst->host_ctx = hview.raw();

        rc = resolve_or_register_pod(
            lview,
            hview,
            "bench::Position",
            sizeof(BenchPosition),
            alignof(BenchPosition),
            inst->pos_comp
        );
        if (!bam::is_ok(rc)) {
            delete inst;
            return rc;
        }
        rc = resolve_or_register_pod(
            lview,
            hview,
            "bench::Velocity",
            sizeof(BenchVelocity),
            alignof(BenchVelocity),
            inst->vel_comp
        );
        if (!bam::is_ok(rc)) {
            delete inst;
            return rc;
        }
        rc = resolve_or_register_pod(
            lview,
            hview,
            "bench::SimParams",
            sizeof(BenchSimParams),
            alignof(BenchSimParams),
            inst->sim_comp
        );
        if (!bam::is_ok(rc)) {
            delete inst;
            return rc;
        }

        inst->dense_terms[0] = bam_term_desc(inst->pos_comp, BAM_ACCESS_WRITE, 0u);
        inst->dense_terms[1] = bam_term_desc(inst->vel_comp, BAM_ACCESS_READ, 0u);
        inst->dense_query.struct_size = static_cast<uint32_t>(sizeof(BamQueryDesc));
        inst->dense_query.flags = 0u;
        inst->dense_query.terms = inst->dense_terms;
        inst->dense_query.term_count = 2u;

        inst->roundtrip_terms[0] = bam_term_desc(inst->sim_comp, BAM_ACCESS_RESOURCE_READ, 0u);
        inst->roundtrip_query.struct_size = static_cast<uint32_t>(sizeof(BamQueryDesc));
        inst->roundtrip_query.flags = 0u;
        inst->roundtrip_query.terms = inst->roundtrip_terms;
        inst->roundtrip_query.term_count = 1u;

        rc = register_bench_system(inst, lview, "bench_noop", nullptr, 0u, sys_noop);
        if (!bam::is_ok(rc)) {
            delete inst;
            return rc;
        }
        rc = register_bench_system(inst, lview, "bench_dense_update", &inst->dense_query, 1u, sys_dense_update);
        if (!bam::is_ok(rc)) {
            delete inst;
            return rc;
        }
        rc = register_bench_system(inst, lview, "bench_command_burst", nullptr, 0u, sys_command_burst);
        if (!bam::is_ok(rc)) {
            delete inst;
            return rc;
        }
        rc = register_bench_system(
            inst,
            lview,
            "bench_host_api_roundtrip",
            &inst->roundtrip_query,
            1u,
            sys_host_api_roundtrip
        );
        if (!bam::is_ok(rc)) {
            delete inst;
            return rc;
        }

        *instance_out = inst;
        return BAM_RESULT_OK;
    });
}

BAM_EXPORT void bamabi_shutdown(BamInstance* instance) {
    try {
        delete instance;
    } catch (...) {
    }
}
