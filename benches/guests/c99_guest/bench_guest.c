/*
 * ISO C99 Benchmark Guest Mod (`ABI 0.4.0`)
 * Compiled with: `-std=c99 -Wall -Wextra -Werror -pedantic` (`-O3` and `-O0 -g`).
 */

#include "bamabi.h"

#include <stdlib.h>
#include <string.h>

typedef struct BenchPosition {
    float x;
    float y;
    float z;
    float w;
} BenchPosition;

typedef struct BenchVelocity {
    float vx;
    float vy;
    float vz;
    float vw;
} BenchVelocity;

typedef struct BenchSimParams {
    float    dt;
    float    damping;
    uint32_t tick;
    uint32_t _pad;
} BenchSimParams;

BAM_STATIC_ASSERT(sizeof(BenchPosition) == 16u, bench_position_size);
BAM_STATIC_ASSERT(BAM_ALIGNOF(BenchPosition) == 4u, bench_position_align);
BAM_STATIC_ASSERT(sizeof(BenchVelocity) == 16u, bench_velocity_size);
BAM_STATIC_ASSERT(BAM_ALIGNOF(BenchVelocity) == 4u, bench_velocity_align);
BAM_STATIC_ASSERT(sizeof(BenchSimParams) == 16u, bench_sim_params_size);
BAM_STATIC_ASSERT(BAM_ALIGNOF(BenchSimParams) == 4u, bench_sim_params_align);

struct BamInstance {
    BamHostApi        api;
    const BamHostCtx* host_ctx;
    BamComponent      pos_comp;
    BamComponent      vel_comp;
    BamComponent      sim_comp;
    BamTermDesc       dense_terms[2];
    BamQueryDesc      dense_query;
    BamTermDesc       roundtrip_terms[1];
    BamQueryDesc      roundtrip_query;
    int               has_iter_next_chunk_batch;
    int               has_checkpoint_rollback;
    int               has_roundtrip_slots;
    volatile uint64_t sink;
};

static const BamManifest g_manifest = {
    (uint32_t)sizeof(BamManifest),
    BAM_ABI_VERSION,
    "bench_guest_c99",
    "0.4.0",
    0u,
    0u,
    NULL,
    NULL
};

BAM_EXPORT const BamManifest* bamabi_manifest(void);
BAM_EXPORT BamResult bamabi_init(const BamHostApi* api, BamLoadCtx* load_ctx, BamInstance** instance_out);
BAM_EXPORT void bamabi_shutdown(BamInstance* instance);

static void bench_integrate_chunk(
    BenchPosition* BAM_RESTRICT pos,
    const BenchVelocity* BAM_RESTRICT vel,
    size_t count
) {
    const float dt = 0.016f;
    size_t i;
    for (i = 0u; i < count; ++i) {
        pos[i].x += vel[i].vx * dt;
        pos[i].y += vel[i].vy * dt;
        pos[i].z += vel[i].vz * dt;
        pos[i].w += vel[i].vw * dt;
    }
}

static BamResult resolve_or_register_pod(
    BamInstance* inst,
    BamLoadCtx* load_ctx,
    const char* name,
    size_t size,
    size_t align,
    BamComponent* out_comp
) {
    BamComponentUuid uuid = bam_uuid_from_parts(name, size, align, 0ull);
    BamResult rc = inst->api.resolve_component(inst->host_ctx, name, &uuid, out_comp);
    if (rc == BAM_RESULT_NOT_FOUND) {
        BamComponentDesc desc = bam_desc_component_with_layout(name, size, align, NULL);
        rc = inst->api.register_component(load_ctx, &desc, out_comp);
    }
    return rc;
}

static BamResult sys_noop(BamSystemCtx* sys_ctx, void* user_data) {
    (void)sys_ctx;
    (void)user_data;
    return BAM_RESULT_OK;
}

static BamResult sys_dense_update(BamSystemCtx* sys_ctx, void* user_data) {
    BamInstance* inst = (BamInstance*)user_data;
    BamIter* iter = NULL;
    size_t chunk_len = 0u;
    BamResult rc;

    if (inst == NULL || sys_ctx == NULL) {
        return BAM_RESULT_NULL_POINTER;
    }
    rc = inst->api.sys_query(sys_ctx, 0u, &iter);
    if (rc != BAM_RESULT_OK) {
        return rc;
    }
    if (iter == NULL) {
        return BAM_RESULT_NULL_POINTER;
    }

    if (inst->has_iter_next_chunk_batch) {
        uint8_t* col_ptrs[2] = { NULL, NULL };
        while (inst->api.iter_next_chunk_batch(iter, &chunk_len, NULL, col_ptrs, 2u, 1u) != BAM_FALSE) {
            BenchPosition* pos;
            const BenchVelocity* vel;
            if (chunk_len == 0u) {
                continue;
            }
            if (col_ptrs[0] == NULL || col_ptrs[1] == NULL) {
                return BAM_RESULT_NULL_POINTER;
            }
            pos = (BenchPosition*)(void*)col_ptrs[0];
            vel = (const BenchVelocity*)(const void*)col_ptrs[1];
            bench_integrate_chunk(pos, vel, chunk_len);
        }
    } else {
        while (inst->api.iter_next_chunk(iter, &chunk_len) != BAM_FALSE) {
            BenchPosition* pos;
            const BenchVelocity* vel;
            if (chunk_len == 0u) {
                continue;
            }
            pos = (BenchPosition*)(void*)inst->api.iter_column_mutable(iter, 0u);
            vel = (const BenchVelocity*)(const void*)inst->api.iter_column_readonly(iter, 1u);
            if (pos == NULL || vel == NULL) {
                return BAM_RESULT_NULL_POINTER;
            }
            bench_integrate_chunk(pos, vel, chunk_len);
        }
    }
    return BAM_RESULT_OK;
}

static BamResult sys_command_burst(BamSystemCtx* sys_ctx, void* user_data) {
    BamInstance* inst = (BamInstance*)user_data;
    BamCommands* cmds = NULL;
    BamResult rc;
    size_t i;
    size_t checkpoint = 0u;
    BamEntity rolled_entity = BAM_ENTITY_NULL;
    const BenchPosition sample_pos = { 1.0f, 2.0f, 3.0f, 4.0f };

    if (inst == NULL || sys_ctx == NULL) {
        return BAM_RESULT_NULL_POINTER;
    }
    if (!inst->has_checkpoint_rollback) {
        return BAM_RESULT_UNSUPPORTED;
    }

    rc = inst->api.sys_commands(sys_ctx, &cmds);
    if (rc != BAM_RESULT_OK) {
        return rc;
    }
    if (cmds == NULL) {
        return BAM_RESULT_NULL_POINTER;
    }

    for (i = 0u; i < 64u; ++i) {
        BamEntity ent = BAM_ENTITY_NULL;
        rc = inst->api.cmd_spawn_empty(cmds, &ent);
        if (rc != BAM_RESULT_OK) {
            return rc;
        }
        rc = inst->api.cmd_insert(
            cmds,
            ent,
            inst->pos_comp,
            (const uint8_t*)&sample_pos,
            sizeof(sample_pos)
        );
        if (rc != BAM_RESULT_OK) {
            return rc;
        }
    }

    rc = inst->api.cmd_checkpoint(cmds, &checkpoint);
    if (rc != BAM_RESULT_OK) {
        return rc;
    }
    rc = inst->api.cmd_spawn_empty(cmds, &rolled_entity);
    if (rc != BAM_RESULT_OK) {
        (void)inst->api.cmd_rollback(cmds, checkpoint);
        return rc;
    }
    return inst->api.cmd_rollback(cmds, checkpoint);
}

static BamResult sys_host_api_roundtrip(BamSystemCtx* sys_ctx, void* user_data) {
    BamInstance* inst = (BamInstance*)user_data;
    BamIter* iter = NULL;
    const BamHostCtx* host_ctx;
    BamResult rc;
    uint64_t acc = 0u;
    size_t i;
    const BamStrView sym_name = bam_str_view_len((const uint8_t*)"bench.sym", 9u);

    if (inst == NULL || sys_ctx == NULL) {
        return BAM_RESULT_NULL_POINTER;
    }
    if (!inst->has_roundtrip_slots) {
        return BAM_RESULT_UNSUPPORTED;
    }

    rc = inst->api.sys_query(sys_ctx, 0u, &iter);
    if (rc != BAM_RESULT_OK) {
        return rc;
    }
    if (iter == NULL) {
        return BAM_RESULT_NULL_POINTER;
    }
    host_ctx = inst->api.sys_host_ctx(sys_ctx);
    if (host_ctx == NULL) {
        host_ctx = inst->host_ctx;
    }

    for (i = 0u; i < 64u; ++i) {
        const BenchSimParams* params =
            (const BenchSimParams*)(const void*)inst->api.res_ptr(iter, 0u);
        BamChangeTicks ticks;
        BamSymbol sym = BAM_SYMBOL_INVALID;
        uint32_t dt_bits = 0u;

        if (params == NULL) {
            return BAM_RESULT_NOT_FOUND;
        }
        rc = inst->api.res_ticks(iter, 0u, &ticks);
        if (rc != BAM_RESULT_OK) {
            return rc;
        }
        rc = inst->api.intern_symbol(host_ctx, &sym_name, &sym);
        if (rc != BAM_RESULT_OK) {
            return rc;
        }
        memcpy(&dt_bits, &params->dt, sizeof(dt_bits));
        acc += (uint64_t)ticks.this_run + (uint64_t)params->tick + (uint64_t)dt_bits + (uint64_t)sym;
    }

    inst->sink = acc;
    return BAM_RESULT_OK;
}

static BamResult register_bench_system(
    BamInstance* inst,
    BamLoadCtx* load_ctx,
    const char* name,
    const BamQueryDesc* queries,
    size_t query_count,
    BamSystemFn callback
) {
    BamSystemDesc sys_desc;
    memset(&sys_desc, 0, sizeof(sys_desc));
    sys_desc.struct_size = (uint32_t)sizeof(BamSystemDesc);
    sys_desc.flags = 0u;
    sys_desc.name = name;
    sys_desc.queries = queries;
    sys_desc.query_count = query_count;
    sys_desc.callback = callback;
    sys_desc.user_data = inst;
    sys_desc.schedule = BAM_SCHEDULE_UPDATE;
    return inst->api.register_system(load_ctx, &sys_desc);
}

BAM_EXPORT const BamManifest* bamabi_manifest(void) {
    return &g_manifest;
}

BAM_EXPORT BamResult bamabi_init(
    const BamHostApi* api,
    BamLoadCtx* load_ctx,
    BamInstance** instance_out
) {
    BamInstance* inst;
    BamResult rc;

    if (api == NULL || load_ctx == NULL || instance_out == NULL) {
        return BAM_RESULT_NULL_POINTER;
    }
    *instance_out = NULL;

    inst = (BamInstance*)calloc(1u, sizeof(BamInstance));
    if (inst == NULL) {
        return BAM_RESULT_INTERNAL_ERROR;
    }

    rc = bam_host_api_copy_bounded(&inst->api, sizeof(inst->api), api);
    if (rc != BAM_RESULT_OK) {
        free(inst);
        return rc;
    }
    if (inst->api.load_host_ctx == NULL ||
        inst->api.resolve_component == NULL ||
        inst->api.register_component == NULL ||
        inst->api.register_system == NULL ||
        inst->api.sys_host_ctx == NULL ||
        inst->api.sys_query == NULL ||
        inst->api.sys_commands == NULL ||
        inst->api.iter_next_chunk == NULL ||
        inst->api.iter_column_readonly == NULL ||
        inst->api.iter_column_mutable == NULL ||
        inst->api.res_ptr == NULL ||
        inst->api.cmd_spawn_empty == NULL ||
        inst->api.cmd_spawn == NULL ||
        inst->api.cmd_insert == NULL) {
        free(inst);
        return BAM_RESULT_NULL_POINTER;
    }

    inst->has_iter_next_chunk_batch =
        (bam_host_api_covers_slot(
             &inst->api,
             BAM_HOST_API_OFFSET_ITER_NEXT_CHUNK_BATCH,
             sizeof(BamIterNextChunkBatchFn)
         ) != BAM_FALSE) &&
        (inst->api.iter_next_chunk_batch != NULL);
    inst->has_checkpoint_rollback =
        (bam_host_api_covers_slot(
             &inst->api,
             BAM_HOST_API_OFFSET_CMD_CHECKPOINT,
             sizeof(BamCmdCheckpointFn)
         ) != BAM_FALSE) &&
        (inst->api.cmd_checkpoint != NULL) &&
        (bam_host_api_covers_slot(
             &inst->api,
             BAM_HOST_API_OFFSET_CMD_ROLLBACK,
             sizeof(BamCmdRollbackFn)
         ) != BAM_FALSE) &&
        (inst->api.cmd_rollback != NULL);
    inst->has_roundtrip_slots =
        (bam_host_api_covers_slot(
             &inst->api,
             BAM_HOST_API_OFFSET_RES_TICKS,
             sizeof(BamResTicksFn)
         ) != BAM_FALSE) &&
        (inst->api.res_ticks != NULL) &&
        (bam_host_api_covers_slot(
             &inst->api,
             BAM_HOST_API_OFFSET_INTERN_SYMBOL,
             sizeof(BamInternSymbolFn)
         ) != BAM_FALSE) &&
        (inst->api.intern_symbol != NULL);

    inst->host_ctx = inst->api.load_host_ctx(load_ctx);
    if (inst->host_ctx == NULL) {
        free(inst);
        return BAM_RESULT_NULL_POINTER;
    }

    rc = resolve_or_register_pod(
        inst,
        load_ctx,
        "bench::Position",
        sizeof(BenchPosition),
        BAM_ALIGNOF(BenchPosition),
        &inst->pos_comp
    );
    if (rc != BAM_RESULT_OK) {
        free(inst);
        return rc;
    }
    rc = resolve_or_register_pod(
        inst,
        load_ctx,
        "bench::Velocity",
        sizeof(BenchVelocity),
        BAM_ALIGNOF(BenchVelocity),
        &inst->vel_comp
    );
    if (rc != BAM_RESULT_OK) {
        free(inst);
        return rc;
    }
    rc = resolve_or_register_pod(
        inst,
        load_ctx,
        "bench::SimParams",
        sizeof(BenchSimParams),
        BAM_ALIGNOF(BenchSimParams),
        &inst->sim_comp
    );
    if (rc != BAM_RESULT_OK) {
        free(inst);
        return rc;
    }

    inst->dense_terms[0] = bam_term_desc(inst->pos_comp, BAM_ACCESS_WRITE, 0u);
    inst->dense_terms[1] = bam_term_desc(inst->vel_comp, BAM_ACCESS_READ, 0u);
    inst->dense_query.struct_size = (uint32_t)sizeof(BamQueryDesc);
    inst->dense_query.flags = 0u;
    inst->dense_query.terms = inst->dense_terms;
    inst->dense_query.term_count = 2u;

    inst->roundtrip_terms[0] = bam_term_desc(inst->sim_comp, BAM_ACCESS_RESOURCE_READ, 0u);
    inst->roundtrip_query.struct_size = (uint32_t)sizeof(BamQueryDesc);
    inst->roundtrip_query.flags = 0u;
    inst->roundtrip_query.terms = inst->roundtrip_terms;
    inst->roundtrip_query.term_count = 1u;

    rc = register_bench_system(inst, load_ctx, "bench_noop", NULL, 0u, sys_noop);
    if (rc != BAM_RESULT_OK) {
        free(inst);
        return rc;
    }
    rc = register_bench_system(inst, load_ctx, "bench_dense_update", &inst->dense_query, 1u, sys_dense_update);
    if (rc != BAM_RESULT_OK) {
        free(inst);
        return rc;
    }
    rc = register_bench_system(inst, load_ctx, "bench_command_burst", NULL, 0u, sys_command_burst);
    if (rc != BAM_RESULT_OK) {
        free(inst);
        return rc;
    }
    rc = register_bench_system(
        inst,
        load_ctx,
        "bench_host_api_roundtrip",
        &inst->roundtrip_query,
        1u,
        sys_host_api_roundtrip
    );
    if (rc != BAM_RESULT_OK) {
        free(inst);
        return rc;
    }

    *instance_out = inst;
    return BAM_RESULT_OK;
}

BAM_EXPORT void bamabi_shutdown(BamInstance* instance) {
    if (instance != NULL) {
        free(instance);
    }
}
