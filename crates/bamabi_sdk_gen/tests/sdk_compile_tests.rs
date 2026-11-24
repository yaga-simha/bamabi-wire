//! Polyglot compiler verification tests for `bamabi_sdk_gen` (`bamabi.h`, `bamabi.hpp`, `bamabi.zig`).
//!
//! Exports the ABI `0.3.0` SDK bundle to a temporary directory and compiles/executes strict
//! ISO C++17 (`-std=c++17 -Wall -Wextra -Wpedantic -Werror`) and Zig `0.16` (`b.addTranslateC`)
//! verification harnesses when the host toolchain provides `c++` / `zig`.
//!
//! Why this suite exists: string-matching unit tests cannot catch C++ standard-compliance warnings,
//! `static_assert` layout failures, `ScopedTransaction` RAII lifetime bugs, or Zig `translate-c`
//! type-translation mismatches. These tests prove the emitted files actually compile and execute.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// RAII temporary directory guard that deletes its directory on drop so compiler test artifacts
/// never leak into the system temp directory even when assertions panic.
struct TempDirGuard(PathBuf);

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("{prefix}_{}_{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Scenario: exports `bamabi.h` and `bamabi.hpp`, compiles a strict ISO C++17 test exercising
/// `HostContextView`, `LoadContextView`, `ChunkView`, `CommandsView`, `ScopedTransaction`, and
/// `SystemContextView` under `-std=c++17 -Wall -Wextra -Wpedantic -Werror`, and runs it.
///
/// Proves:
/// - `bamabi.hpp` and `bamabi.h` compile cleanly under strict C++17 warnings-as-errors.
/// - `bam::offsets::*` `static_assert`s hold on the host C++ ABI.
/// - Prefix-checked extension slots (`asset_load`, `intern_symbol`, `bind_named_object`) safely
///   return `BAM_RESULT_UNSUPPORTED` when zeroed/absent instead of dereferencing null pointers.
/// - `ScopedTransaction` rolls back automatically on scope exit when uncommitted and suppresses
///   rollback once `commit()` is called.
#[test]
fn exported_cpp17_header_compiles_and_passes_strict_checks() {
    let Ok(check) = Command::new("c++").arg("--version").output() else {
        return;
    };
    if !check.status.success() {
        return;
    }

    let tmp = TempDirGuard::new("bamabi_cpp17_test");
    bamabi_sdk_gen::export_sdk(tmp.path()).expect("export_sdk must succeed");

    let cpp_src = tmp.path().join("verify_cpp17.cpp");
    let exe_path = tmp.path().join("verify_cpp17");
    fs::write(
        &cpp_src,
        r#"#include "bamabi.hpp"
#include <cassert>
#include <cstring>

struct BamHostCtx { int id; };
struct BamLoadCtx { BamHostCtx* h; };
struct BamIter {
    size_t step;
    BamEntity ents[2];
    float col[2];
    float col1[2];
    uint32_t added[1];
    uint32_t changed[1];
    uint32_t last_mask;
    size_t batch_calls;
};
struct BamCommands {
    size_t checkpoints;
    size_t rollbacks;
    size_t last_token;
    size_t cloned;
    size_t inserted_resources;
    size_t removed_resources;
};
struct BamSystemCtx { BamHostCtx* h; BamIter* it; BamCommands* cmds; };

static const BamHostCtx* mock_load_host_ctx(const BamLoadCtx* lc) { return lc ? lc->h : nullptr; }
static const BamHostCtx* mock_sys_host_ctx(const BamSystemCtx* sc) { return sc ? sc->h : nullptr; }
static BamResult mock_sys_query(BamSystemCtx* sc, size_t idx, BamIter** out) {
    if (idx != 0u) return BAM_RESULT_OUT_OF_RANGE;
    *out = sc->it;
    return BAM_RESULT_OK;
}
static BamResult mock_sys_commands(BamSystemCtx* sc, BamCommands** out) {
    *out = sc->cmds;
    return BAM_RESULT_OK;
}
static BamBool mock_iter_next_chunk(BamIter* it, size_t* out_len) {
    if (it->step == 0u) { it->step = 1u; *out_len = 1u; return BAM_TRUE; }
    *out_len = 0u;
    return BAM_FALSE;
}
static BamBool mock_iter_next_chunk_batch(
    BamIter* it,
    size_t* out_len,
    const BamEntity** out_entities,
    uint8_t** out_col_ptrs,
    size_t col_count,
    uint32_t mark_changed_mask
) {
    it->batch_calls += 1u;
    it->last_mask = mark_changed_mask;
    if (it->step == 0u) {
        it->step = 1u;
        *out_len = 2u;
        if (out_entities != nullptr) {
            *out_entities = it->ents;
        }
        if (out_col_ptrs != nullptr) {
            if (col_count > 0u) out_col_ptrs[0] = reinterpret_cast<uint8_t*>(it->col);
            if (col_count > 1u) out_col_ptrs[1] = reinterpret_cast<uint8_t*>(it->col1);
        }
        return BAM_TRUE;
    }
    *out_len = 0u;
    if (out_entities != nullptr) {
        *out_entities = nullptr;
    }
    if (out_col_ptrs != nullptr) {
        for (size_t i = 0u; i < col_count; ++i) {
            out_col_ptrs[i] = nullptr;
        }
    }
    return BAM_FALSE;
}
static BamResult mock_iter_reset(BamIter* it) { it->step = 0u; return BAM_RESULT_OK; }
static const BamEntity* mock_iter_entities(const BamIter* it) { return it->ents; }
static const uint8_t* mock_iter_col_ro(const BamIter* it, size_t) { return reinterpret_cast<const uint8_t*>(it->col); }
static uint8_t* mock_iter_col_mut(BamIter* it, size_t) { return reinterpret_cast<uint8_t*>(it->col); }
static BamResult mock_iter_column_ticks(
    const BamIter* it,
    size_t,
    const uint32_t** out_added,
    const uint32_t** out_changed,
    uint32_t* out_last_run,
    uint32_t* out_this_run
) {
    *out_added = it->added;
    *out_changed = it->changed;
    *out_last_run = 10u;
    *out_this_run = 12u;
    return BAM_RESULT_OK;
}
static BamResult mock_cmd_checkpoint(BamCommands* c, size_t* out_tok) {
    c->checkpoints += 1u;
    *out_tok = c->checkpoints;
    return BAM_RESULT_OK;
}
static BamResult mock_cmd_rollback(BamCommands* c, size_t tok) {
    c->rollbacks += 1u;
    c->last_token = tok;
    return BAM_RESULT_OK;
}
static BamResult mock_cmd_clone_entity(BamCommands* c, BamEntity src, BamEntity dst) {
    if (bam_entity_is_null(src) || bam_entity_is_null(dst)) return BAM_RESULT_INVALID_ENTITY;
    c->cloned += 1u;
    return BAM_RESULT_OK;
}
static BamResult mock_cmd_insert_resource(
    BamCommands* c,
    BamComponent res,
    const uint8_t* ptr,
    size_t len
) {
    if (!bam_component_is_valid(res)) return BAM_RESULT_INVALID_COMPONENT;
    if (len > 0u && ptr == nullptr) return BAM_RESULT_NULL_POINTER;
    c->inserted_resources += 1u;
    return BAM_RESULT_OK;
}
static BamResult mock_cmd_remove_resource(BamCommands* c, BamComponent res) {
    if (!bam_component_is_valid(res)) return BAM_RESULT_INVALID_COMPONENT;
    c->removed_resources += 1u;
    return BAM_RESULT_OK;
}

int main() {
    BamHostApi api;
    std::memset(&api, 0, sizeof(api));
    api.struct_size = static_cast<uint32_t>(sizeof(BamHostApi));
    api.abi_version = BAM_ABI_VERSION;
    api.load_host_ctx = mock_load_host_ctx;
    api.sys_host_ctx = mock_sys_host_ctx;
    api.sys_query = mock_sys_query;
    api.sys_commands = mock_sys_commands;
    api.iter_next_chunk = mock_iter_next_chunk;
    api.iter_next_chunk_batch = mock_iter_next_chunk_batch;
    api.iter_reset = mock_iter_reset;
    api.iter_entities = mock_iter_entities;
    api.iter_column_readonly = mock_iter_col_ro;
    api.iter_column_mutable = mock_iter_col_mut;
    api.iter_column_mutable_untracked = mock_iter_col_mut;
    api.iter_column_ticks = mock_iter_column_ticks;
    api.cmd_checkpoint = mock_cmd_checkpoint;
    api.cmd_rollback = mock_cmd_rollback;
    api.cmd_clone_entity = mock_cmd_clone_entity;
    api.cmd_insert_resource = mock_cmd_insert_resource;
    api.cmd_remove_resource = mock_cmd_remove_resource;

    BamHostCtx h{42};
    BamLoadCtx lc{&h};
    BamIter it{0u, {7ull, 8ull}, {3.5f, 4.5f}, {10.0f, 20.0f}, {11u}, {12u}, 0u, 0u};
    BamCommands cmds{0u, 0u, 0u, 0u, 0u, 0u};
    BamSystemCtx sc{&h, &it, &cmds};

    bam::LoadContextView lview(api, &lc);
    assert(lview.valid());
    bam::HostContextView hview = lview.host_ctx();
    assert(hview.valid());
    assert(hview.get_proc_address("bam.v0.missing") == nullptr);
    BamAssetHandle ah = bam_asset_handle_none();
    assert(hview.asset_load(1u, "mod://a/b.png", ah) == BAM_RESULT_UNSUPPORTED);
    BamSymbol sym = BAM_SYMBOL_INVALID;
    assert(hview.intern_symbol("hero", sym) == BAM_RESULT_UNSUPPORTED);
    assert(hview.bind_named_object("hero", 7ull) == BAM_RESULT_UNSUPPORTED);

    bam::SystemContextView sview(api, &sc);
    assert(sview.valid());
    assert(sview.host_ctx().raw() == &h);
    bam::ChunkView cview;
    assert(bam::is_ok(sview.query_view(0u, cview)));
    size_t n = 0u;
    assert(cview.next_chunk(n) && n == 1u && cview.len() == 1u);
    assert(cview.entities()[0] == 7ull);
    assert(cview.column_readonly<float>(0u)[0] == 3.5f);
    cview.column_mutable<float>(0u)[0] = 9.0f;
    assert(cview.column_mutable_untracked<float>(0u)[0] == 9.0f);
    const uint32_t* added_ticks = nullptr;
    const uint32_t* changed_ticks = nullptr;
    uint32_t last_run = 0u;
    uint32_t this_run = 0u;
    assert(bam::is_ok(cview.column_ticks(0u, added_ticks, changed_ticks, last_run, this_run)));
    assert(added_ticks[0] == 11u && changed_ticks[0] == 12u && last_run == 10u && this_run == 12u);
    assert(bam::is_ok(cview.reset()));

    // Slot 68 (iter_next_chunk_batch): supported host, non-null & null out_entities, exhaustion zeroing.
    assert(cview.supports_next_chunk_batch());
    size_t batch_len = 0u;
    const BamEntity* batch_ents = nullptr;
    uint8_t* col_ptrs[2] = {nullptr, nullptr};
    assert(cview.next_chunk_batch(batch_len, &batch_ents, col_ptrs, 2u, 0x1u));
    assert(batch_len == 2u && cview.len() == 2u);
    assert(batch_ents != nullptr && batch_ents[0] == 7ull && batch_ents[1] == 8ull);
    assert(col_ptrs[0] != nullptr && col_ptrs[1] != nullptr);
    assert(reinterpret_cast<const float*>(col_ptrs[0])[0] == 9.0f);
    assert(reinterpret_cast<const float*>(col_ptrs[0])[1] == 4.5f);
    assert(reinterpret_cast<const float*>(col_ptrs[1])[0] == 10.0f);
    assert(reinterpret_cast<const float*>(col_ptrs[1])[1] == 20.0f);
    assert(it.last_mask == 0x1u && it.batch_calls == 1u);

    // Exhaustion must return false and zero out_len, cview.len(), batch_ents, and col_ptrs.
    assert(!cview.next_chunk_batch(batch_len, &batch_ents, col_ptrs, 2u, 0x1u));
    assert(batch_len == 0u && cview.len() == 0u);
    assert(batch_ents == nullptr && col_ptrs[0] == nullptr && col_ptrs[1] == nullptr);

    // Unchecked path with out_entities == nullptr and col_count == 2.
    assert(bam::is_ok(cview.reset()));
    assert(cview.next_chunk_batch_unchecked(batch_len, nullptr, col_ptrs, 2u, 0x3u));
    assert(batch_len == 2u && cview.len() == 2u && it.last_mask == 0x3u);
    assert(col_ptrs[0] != nullptr && col_ptrs[1] != nullptr);
    assert(!cview.next_chunk_batch_unchecked(batch_len, nullptr, col_ptrs, 2u, 0x3u));
    assert(batch_len == 0u && cview.len() == 0u);
    assert(col_ptrs[0] == nullptr && col_ptrs[1] == nullptr);

    // Null iter_next_chunk_batch slot must report unsupported and return false.
    BamHostApi null_batch_api = api;
    null_batch_api.iter_next_chunk_batch = nullptr;
    bam::ChunkView null_batch_cview(null_batch_api, &it, 1u);
    assert(!null_batch_cview.supports_next_chunk_batch());
    batch_len = 99u;
    assert(!null_batch_cview.next_chunk_batch(batch_len, &batch_ents, col_ptrs, 2u, 0x1u));
    assert(batch_len == 0u && null_batch_cview.len() == 0u);

    // BAM_HOST_API_MIN_V0_3_SIZE-only host (even with non-null fn ptr) must not dispatch slot 68 and falls back to next_chunk.
    assert(bam::is_ok(cview.reset()));
    const size_t calls_before_v03 = it.batch_calls;
    BamHostApi v03_api = api;
    v03_api.struct_size = static_cast<uint32_t>(BAM_HOST_API_MIN_V0_3_SIZE);
    bam::ChunkView v03_cview(v03_api, &it);
    assert(!v03_cview.supports_next_chunk_batch());
    batch_len = 77u;
    assert(!v03_cview.next_chunk_batch(batch_len, &batch_ents, col_ptrs, 2u, 0x1u));
    assert(batch_len == 0u && v03_cview.len() == 0u && it.batch_calls == calls_before_v03);
    assert(v03_cview.next_chunk(batch_len) && batch_len == 1u && v03_cview.len() == 1u);
    assert(bam::is_ok(v03_cview.reset()));

    bam::CommandsView cmd_view;
    assert(bam::is_ok(sview.commands_view(cmd_view)));
    assert(bam::is_ok(cmd_view.clone_entity(7ull, 8ull)));
    float res_val = 4.25f;
    assert(bam::is_ok(cmd_view.insert_resource_pod(3u, res_val)));
    assert(bam::is_ok(cmd_view.remove_resource(3u)));
    assert(cmds.cloned == 1u && cmds.inserted_resources == 1u && cmds.removed_resources == 1u);
    {
        bam::ScopedTransaction tx(cmd_view);
        assert(tx.active() && tx.token() == 1u);
    }
    assert(cmds.rollbacks == 1u && cmds.last_token == 1u);
    {
        bam::ScopedTransaction tx(cmd_view);
        assert(tx.active() && tx.token() == 2u);
        tx.commit();
    }
    assert(cmds.rollbacks == 1u);

#define BAM_TEST_STR_IMPL(x) #x
#define BAM_TEST_STR(x) BAM_TEST_STR_IMPL(x)
#if defined(__GNUC__) || defined(__clang__)
    assert(std::strcmp(BAM_TEST_STR(BAM_RESTRICT), "__restrict__") == 0);
#endif
    float src_col[2] = {1.25f, 2.5f};
    float dst_col[2] = {0.0f, 0.0f};
    bam::copy_column_restricted(dst_col, src_col, 2u);
    assert(dst_col[0] == 1.25f && dst_col[1] == 2.5f);

    // ABI 0.4.0 C++17 helpers, constructors & slot guards
    assert(bam::fnv1a_64_hash_system("mod_a", "sys_b") == bam::fnv1a_64_str("mod_a::sys_b"));
    assert(bam::hash_field_schema(0ull, "hp", "i32", 0ull) != 0ull);
    BamEntity resolved_obj = BAM_ENTITY_NULL;
    assert(hview.resolve_named_object("hero", resolved_obj) == BAM_RESULT_UNSUPPORTED);
    BamStrView sym_str = bam_str_view(nullptr);
    assert(hview.resolve_symbol_str(1u, sym_str) == BAM_RESULT_UNSUPPORTED);
    BamTimeInfo tinfo = bam_time_info();
    assert(sview.time_info(tinfo) == BAM_RESULT_UNSUPPORTED);
    uint32_t lr = 0u, tr = 0u;
    assert(sview.current_ticks(lr, tr) == BAM_RESULT_UNSUPPORTED);
    const BamEntity* rel_ents = nullptr;
    size_t rel_len = 0u;
    assert(sview.relation_targets(1ull, 2u, rel_ents, rel_len) == BAM_RESULT_UNSUPPORTED);
    assert(sview.relation_sources(1ull, 2u, rel_ents, rel_len) == BAM_RESULT_UNSUPPORTED);
    const void* edge_ptr = nullptr;
    size_t edge_sz = 0u;
    assert(sview.relation_edge_data(1ull, 2u, 3ull, edge_ptr, edge_sz) == BAM_RESULT_UNSUPPORTED);
    assert(cmd_view.despawn_batch(nullptr, 0u) == BAM_RESULT_UNSUPPORTED);
    assert(cmd_view.insert_batch(nullptr, 0u, 1u, nullptr, 0u, 0u) == BAM_RESULT_UNSUPPORTED);
    assert(cmd_view.remove_batch(nullptr, 0u, 1u) == BAM_RESULT_UNSUPPORTED);
    assert(cmd_view.spawn_batch_ex(nullptr, 0u, 0u, nullptr) == BAM_RESULT_UNSUPPORTED);
    const float* msg_slice_ptr = nullptr;
    size_t msg_slice_cnt = 0u;
    assert(cview.msg_read_slice<float>(0u, msg_slice_ptr, msg_slice_cnt) == BAM_RESULT_UNSUPPORTED);
    assert(cview.column_rows_mutable_untracked(0u) == nullptr);
    BamRowRange rr = bam_row_range(0u, 2u);
    assert(cview.mark_changed_ranges(0u, &rr, 1u) == BAM_RESULT_UNSUPPORTED);
    BamParOptions par_opts = bam_par_options();
    assert(cview.par_for_each_chunk_ex(nullptr, nullptr, &par_opts) == BAM_RESULT_UNSUPPORTED);
    BamComponent reg_out = BAM_COMPONENT_INVALID;
    BamComponentDesc rdesc = bam_desc_component_with_layout("cpp::Res", 4u, 4u, nullptr);
    assert(lview.register_resource(rdesc, reg_out) == BAM_RESULT_UNSUPPORTED);
    assert(lview.register_message(rdesc, reg_out) == BAM_RESULT_UNSUPPORTED);

    return 0;
}
"#,
    )
    .expect("write C++17 test");

    let compile = Command::new("c++")
        .args([
            "-std=c++17",
            "-Wall",
            "-Wextra",
            "-Wpedantic",
            "-Werror",
            "-I",
        ])
        .arg(tmp.path())
        .arg(&cpp_src)
        .arg("-o")
        .arg(&exe_path)
        .output()
        .expect("invoke c++");
    assert!(
        compile.status.success(),
        "C++17 compilation failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    let run = Command::new(&exe_path).output().expect("run C++17 binary");
    assert!(
        run.status.success(),
        "C++17 test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
}

/// Scenario: exports `bamabi.h` and `bamabi.zig`, builds a Zig test suite via `b.addTranslateC`
/// verifying all 18 `BamResult` round-trips and semantic compilation of all 73 slot wrappers on
/// `HostContextView`, `LoadContextView`, `ChunkView`, `CommandsView`, and `SystemContextView`.
///
/// Proves:
/// - `bamabi.h` translates cleanly via Zig `0.16` `b.addTranslateC` into `"bam_c"`.
/// - `bamabi.zig` `comptime` offset assertions (`STRUCT_SIZE`, `ABI_VERSION`, `GET_PROC_ADDRESS`,
///   and `MIGRATE_COMPONENT_COLUMN`) match `c.BamHostApi` exactly.
/// - All 18 `BamResult` codes round-trip losslessly through `checkResult` / `mapError`.
/// - All 73 slot wrapper methods type-check cleanly and enforce `null` / `error.Unsupported` guards.
#[test]
fn exported_zig_sdk_compiles_and_passes_with_translate_c() {
    let Ok(ver_out) = Command::new("zig").arg("version").output() else {
        return;
    };
    if !ver_out.status.success() {
        return;
    }

    let tmp = TempDirGuard::new("bamabi_zig_test");
    bamabi_sdk_gen::export_sdk(tmp.path()).expect("export_sdk must succeed");
    let zig_cache = tmp.path().join("zig-global-cache");
    fs::create_dir_all(&zig_cache).expect("create zig cache dir");

    fs::write(
        tmp.path().join("build.zig"),
        r#"const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const tc = b.addTranslateC(.{
        .root_source_file = b.path("bamabi.h"),
        .target = target,
        .optimize = optimize,
    });
    const bam_c_mod = tc.createModule();

    const bam_mod = b.createModule(.{
        .root_source_file = b.path("bamabi.zig"),
        .target = target,
        .optimize = optimize,
        .imports = &.{
            .{ .name = "bam_c", .module = bam_c_mod },
        },
    });

    const test_mod = b.createModule(.{
        .root_source_file = b.path("test_zig.zig"),
        .target = target,
        .optimize = optimize,
        .imports = &.{
            .{ .name = "bamabi", .module = bam_mod },
            .{ .name = "bam_c", .module = bam_c_mod },
        },
    });

    const tests = b.addTest(.{ .root_module = test_mod });
    const run_tests = b.addRunArtifact(tests);
    const test_step = b.step("test", "Run Zig SDK tests");
    test_step.dependOn(&run_tests.step);
}
"#,
    )
    .expect("write build.zig");

    fs::write(
        tmp.path().join("test_zig.zig"),
        r#"const std = @import("std");
const bam = @import("bamabi");
const c = @import("bam_c");

test "all 18 BamResult codes round-trip losslessly" {
    try bam.checkResult(c.BAM_RESULT_OK);
    const codes = [_]c.BamResult{
        c.BAM_RESULT_INVALID_COMPONENT,
        c.BAM_RESULT_ACCESS_CONFLICT,
        c.BAM_RESULT_PANIC_CAUGHT,
        c.BAM_RESULT_NULL_POINTER,
        c.BAM_RESULT_INVALID_UTF8,
        c.BAM_RESULT_PERMISSION_DENIED,
        c.BAM_RESULT_INVALID_VERSION,
        c.BAM_RESULT_INVALID_ARGUMENT,
        c.BAM_RESULT_OUT_OF_RANGE,
        c.BAM_RESULT_NOT_FOUND,
        c.BAM_RESULT_INVALID_STATE,
        c.BAM_RESULT_INVALID_ENTITY,
        c.BAM_RESULT_CAPACITY_EXCEEDED,
        c.BAM_RESULT_UNSUPPORTED,
        c.BAM_RESULT_TYPE_MISMATCH,
        c.BAM_RESULT_CYCLE_DETECTED,
        c.BAM_RESULT_INTERNAL_ERROR,
    };
    for (codes) |code| {
        if (bam.checkResult(code)) |_| {
            return error.TestUnexpectedResult;
        } else |err| {
            try std.testing.expectEqual(code, bam.mapError(err));
        }
    }
}

test "all Zig views compile and enforce unsupported/null guards" {
    var api: c.BamHostApi = std.mem.zeroes(c.BamHostApi);
    api.struct_size = @sizeOf(c.BamHostApi);
    api.abi_version = c.BAM_ABI_VERSION;

    _ = &bam.HostContextView.getProcAddress;
    _ = &bam.HostContextView.lastError;
    _ = &bam.HostContextView.resolveComponent;
    _ = &bam.HostContextView.assetLoad;
    _ = &bam.HostContextView.assetGetLoadState;
    _ = &bam.HostContextView.assetRelease;
    _ = &bam.HostContextView.logDiagnostic;
    _ = &bam.HostContextView.reportGuestFault;
    _ = &bam.HostContextView.resolveComponentByUuid;
    _ = &bam.HostContextView.componentLayoutInfo;
    _ = &bam.HostContextView.internSymbol;
    _ = &bam.HostContextView.bindNamedObject;
    _ = &bam.HostContextView.resolveNamedObject;
    _ = &bam.HostContextView.resolveSymbolStr;

    _ = &bam.LoadContextView.hostCtx;
    _ = &bam.LoadContextView.registerComponent;
    _ = &bam.LoadContextView.registerSystem;
    _ = &bam.LoadContextView.registerObserver;
    _ = &bam.LoadContextView.registerRelation;
    _ = &bam.LoadContextView.registerSystemHandle;
    _ = &bam.LoadContextView.configureScheduleSet;
    _ = &bam.LoadContextView.mountSystemInSchedule;
    _ = &bam.LoadContextView.migrateComponentColumn;
    _ = &bam.LoadContextView.registerResource;
    _ = &bam.LoadContextView.registerMessage;

    _ = &bam.ChunkView.nextChunk;
    _ = &bam.ChunkView.supportsNextChunkBatch;
    _ = &bam.ChunkView.nextChunkBatchUnchecked;
    _ = &bam.ChunkView.nextChunkBatch;
    _ = &bam.ChunkView.reset;
    _ = &bam.ChunkView.entities;
    _ = &bam.ChunkView.markChangedRange;
    _ = &bam.ChunkView.columnRowsReadonly;
    _ = &bam.ChunkView.columnRowsMutable;
    _ = &bam.ChunkView.columnRowsMutableUntracked;
    _ = &bam.ChunkView.markChangedRanges;
    _ = &bam.ChunkView.columnInfo;
    _ = &bam.ChunkView.resTicks;
    _ = &bam.ChunkView.parForEachChunk;
    _ = &bam.ChunkView.parForEachChunkReadonly;
    _ = &bam.ChunkView.parForEachChunkEx;
    _ = &bam.ChunkView.msgSend;
    _ = &bam.ChunkView.msgReadSlice;
    _ = &bam.ChunkView.readRemoved;
    _ = &bam.ChunkView.totalCount;
    _ = &bam.ChunkView.upCascade;
    _ = &bam.ChunkView.columnTicks;

    _ = &bam.CommandsView.spawnEmpty;
    _ = &bam.CommandsView.spawn;
    _ = &bam.CommandsView.spawnPersistent;
    _ = &bam.CommandsView.spawnBatch;
    _ = &bam.CommandsView.despawn;
    _ = &bam.CommandsView.despawnAll;
    _ = &bam.CommandsView.insert;
    _ = &bam.CommandsView.insertDefault;
    _ = &bam.CommandsView.insertIfNew;
    _ = &bam.CommandsView.remove;
    _ = &bam.CommandsView.trigger;
    _ = &bam.CommandsView.triggerEx;
    _ = &bam.CommandsView.observe;
    _ = &bam.CommandsView.spawnScene;
    _ = &bam.CommandsView.checkpoint;
    _ = &bam.CommandsView.rollback;
    _ = &bam.CommandsView.insertRelation;
    _ = &bam.CommandsView.removeRelation;
    _ = &bam.CommandsView.cloneEntity;
    _ = &bam.CommandsView.insertResource;
    _ = &bam.CommandsView.removeResource;
    _ = &bam.CommandsView.despawnBatch;
    _ = &bam.CommandsView.insertBatch;
    _ = &bam.CommandsView.removeBatch;
    _ = &bam.CommandsView.spawnBatchEx;

    _ = &bam.ScopedTransaction.init;
    _ = &bam.ScopedTransaction.commit;
    _ = &bam.ScopedTransaction.rollbackNow;
    _ = &bam.ScopedTransaction.deinit;

    _ = &bam.SystemContextView.hostCtx;
    _ = &bam.SystemContextView.query;
    _ = &bam.SystemContextView.commands;
    _ = &bam.SystemContextView.entityChildren;
    _ = &bam.SystemContextView.currentTicks;
    _ = &bam.SystemContextView.timeInfo;
    _ = &bam.SystemContextView.relationTargets;
    _ = &bam.SystemContextView.relationSources;
    _ = &bam.SystemContextView.relationEdgeData;

    try std.testing.expectEqual(
        bam.fnv1a64Str("mod_a::sys_b"),
        bam.fnv1a64HashSystem("mod_a", "sys_b"),
    );
    try std.testing.expect(bam.hashFieldSchema(0, "hp", "i32", 0) != 0);

    var dummy_byte: u8 = 0;
    const hctx: *const c.BamHostCtx = @ptrCast(&dummy_byte);
    const hv = bam.HostContextView{ .api = &api, .ctx = hctx };
    try std.testing.expectEqual(@as(?*const anyopaque, null), hv.getProcAddress("bam.v0.x"));
    try std.testing.expectError(error.Unsupported, hv.internSymbol("hero"));

    const it: *c.BamIter = @ptrCast(&dummy_byte);
    const cv = bam.ChunkView{ .api = &api, .iter = it };
    try std.testing.expectEqual(@as(?[*]const f32, null), cv.columnReadonly(f32, 0));
    try std.testing.expectEqual(@as(?[*]f32, null), cv.columnMutable(f32, 0));
    try std.testing.expectEqual(@as(?[*]f32, null), cv.columnMutableUntracked(f32, 0));
    try std.testing.expectEqual(@as(?*const f32, null), cv.resPtr(f32, 0));
    try std.testing.expectEqual(@as(?*f32, null), cv.resMutPtr(f32, 0));
    var buf = [_]f32{0.0};
    try std.testing.expectError(error.Unsupported, cv.msgReadBatch(f32, 0, &buf));
    try std.testing.expectError(error.Unsupported, cv.msgWriteBatch(f32, 0, &buf));
    try std.testing.expect(!cv.supportsNextChunkBatch());
    var cv_mut = cv;
    var empty_cols = [_]?[*]u8{ null, null };
    try std.testing.expectError(error.Unsupported, cv_mut.nextChunkBatch(null, &empty_cols, 1));

    const sctx: *c.BamSystemCtx = @ptrCast(&dummy_byte);
    const sv = bam.SystemContextView{ .api = &api, .ctx = sctx };
    try std.testing.expectEqual(@as(?*const f32, null), sv.entityGet(f32, 0, 1, 0));
    try std.testing.expectEqual(@as(?*f32, null), sv.entityGetMut(f32, 0, 1, 0));
    try std.testing.expectEqual(@as(?*const f32, null), sv.resPtr(f32, 1, null));
    try std.testing.expectEqual(@as(?*f32, null), sv.resMutPtr(f32, 1, null));

    try std.testing.expectError(error.Unsupported, cv.columnTicks(0));
    const cmds_ptr: *c.BamCommands = @ptrCast(&dummy_byte);
    const cmds_view = bam.CommandsView{ .api = &api, .cmds = cmds_ptr };
    try std.testing.expectError(error.Unsupported, cmds_view.cloneEntity(1, 2));
    try std.testing.expectError(error.Unsupported, cmds_view.insertResource(1, "ab"));
    try std.testing.expectError(error.Unsupported, cmds_view.removeResource(1));
    try std.testing.expectError(error.Unsupported, bam.ScopedTransaction.init(cmds_view));
}

const MockCmdState = struct {
    checkpoints: usize = 0,
    rollbacks: usize = 0,
    last_token: usize = 0,
    cloned: usize = 0,
    inserted_res: usize = 0,
    removed_res: usize = 0,
};

fn zigMockCheckpoint(cmds: ?*c.BamCommands, out_tok: [*c]usize) callconv(.c) c.BamResult {
    const st: *MockCmdState = @ptrCast(@alignCast(cmds.?));
    st.checkpoints += 1;
    out_tok[0] = st.checkpoints;
    return c.BAM_RESULT_OK;
}

fn zigMockRollback(cmds: ?*c.BamCommands, tok: usize) callconv(.c) c.BamResult {
    const st: *MockCmdState = @ptrCast(@alignCast(cmds.?));
    st.rollbacks += 1;
    st.last_token = tok;
    return c.BAM_RESULT_OK;
}

fn zigMockCloneEntity(cmds: ?*c.BamCommands, _: c.BamEntity, _: c.BamEntity) callconv(.c) c.BamResult {
    const st: *MockCmdState = @ptrCast(@alignCast(cmds.?));
    st.cloned += 1;
    return c.BAM_RESULT_OK;
}

fn zigMockInsertResource(cmds: ?*c.BamCommands, _: c.BamComponent, _: [*c]const u8, _: usize) callconv(.c) c.BamResult {
    const st: *MockCmdState = @ptrCast(@alignCast(cmds.?));
    st.inserted_res += 1;
    return c.BAM_RESULT_OK;
}

fn zigMockRemoveResource(cmds: ?*c.BamCommands, _: c.BamComponent) callconv(.c) c.BamResult {
    const st: *MockCmdState = @ptrCast(@alignCast(cmds.?));
    st.removed_res += 1;
    return c.BAM_RESULT_OK;
}

test "Zig ScopedTransaction and new command wrappers execute cleanly" {
    var api: c.BamHostApi = std.mem.zeroes(c.BamHostApi);
    api.struct_size = @sizeOf(c.BamHostApi);
    api.abi_version = c.BAM_ABI_VERSION;
    api.cmd_checkpoint = zigMockCheckpoint;
    api.cmd_rollback = zigMockRollback;
    api.cmd_clone_entity = zigMockCloneEntity;
    api.cmd_insert_resource = zigMockInsertResource;
    api.cmd_remove_resource = zigMockRemoveResource;

    var state = MockCmdState{};
    const cmds = bam.CommandsView{ .api = &api, .cmds = @ptrCast(&state) };

    try cmds.cloneEntity(7, 8);
    try cmds.insertResource(3, "data");
    try cmds.removeResource(3);
    try std.testing.expectEqual(@as(usize, 1), state.cloned);
    try std.testing.expectEqual(@as(usize, 1), state.inserted_res);
    try std.testing.expectEqual(@as(usize, 1), state.removed_res);

    {
        var tx = try bam.ScopedTransaction.init(cmds);
        defer tx.deinit();
        try std.testing.expectEqual(@as(usize, 1), tx.token);
    }
    try std.testing.expectEqual(@as(usize, 1), state.rollbacks);
    try std.testing.expectEqual(@as(usize, 1), state.last_token);

    {
        var tx = try bam.ScopedTransaction.init(cmds);
        defer tx.deinit();
        try std.testing.expectEqual(@as(usize, 2), tx.token);
        tx.commit();
    }
    try std.testing.expectEqual(@as(usize, 1), state.rollbacks);

    {
        var tx = try bam.ScopedTransaction.init(cmds);
        defer tx.deinit();
        try std.testing.expectEqual(@as(usize, 3), tx.token);
        try tx.rollbackNow();
        try tx.rollbackNow(); // idempotent
    }
    try std.testing.expectEqual(@as(usize, 2), state.rollbacks);
    try std.testing.expectEqual(@as(usize, 3), state.last_token);
}

const MockBatchIter = struct {
    step: usize = 0,
    calls: usize = 0,
    last_mask: u32 = 0,
    last_col_count: usize = 0,
    ents: [2]c.BamEntity = .{ 101, 202 },
    col0: [2]f32 = .{ 1.5, 2.5 },
    col1: [2]f32 = .{ 3.5, 4.5 },
};

fn zigMockIterNextChunkBatch(
    iter_opt: ?*c.BamIter,
    out_len_opt: [*c]usize,
    out_entities_opt: [*c][*c]const c.BamEntity,
    out_col_ptrs_opt: [*c][*c]u8,
    col_count: usize,
    mark_changed_mask: u32,
) callconv(.c) c.BamBool {
    const it: *MockBatchIter = @ptrCast(@alignCast(iter_opt.?));
    it.calls += 1;
    it.last_mask = mark_changed_mask;
    it.last_col_count = col_count;
    if (it.step == 0) {
        it.step = 1;
        out_len_opt[0] = 2;
        if (out_entities_opt != null) {
            out_entities_opt[0] = @constCast(&it.ents);
        }
        if (out_col_ptrs_opt != null) {
            if (col_count > 0) out_col_ptrs_opt[0] = @ptrCast(&it.col0);
            if (col_count > 1) out_col_ptrs_opt[1] = @ptrCast(&it.col1);
        }
        return c.BAM_TRUE;
    }
    out_len_opt[0] = 0;
    if (out_entities_opt != null) {
        out_entities_opt[0] = null;
    }
    if (out_col_ptrs_opt != null) {
        for (0..col_count) |i| {
            out_col_ptrs_opt[i] = null;
        }
    }
    return c.BAM_FALSE;
}

fn zigMockIterReset(iter_opt: ?*c.BamIter) callconv(.c) c.BamResult {
    const it: *MockBatchIter = @ptrCast(@alignCast(iter_opt.?));
    it.step = 0;
    return c.BAM_RESULT_OK;
}

test "Zig ChunkView.nextChunkBatch handles supported, col_count=0, exhaustion, and MIN_V0_3_SIZE hosts" {
    var api: c.BamHostApi = std.mem.zeroes(c.BamHostApi);
    api.struct_size = @sizeOf(c.BamHostApi);
    api.abi_version = c.BAM_ABI_VERSION;
    api.iter_next_chunk_batch = zigMockIterNextChunkBatch;
    api.iter_reset = zigMockIterReset;

    var state = MockBatchIter{};
    var cview = bam.ChunkView{ .api = &api, .iter = @ptrCast(&state) };
    try std.testing.expect(cview.supportsNextChunkBatch());

    // Supported host with non-null out_entities and 2 columns.
    var ents: ?[*]const c.BamEntity = null;
    var col_ptrs = [_]?[*]u8{ null, null };
    const n1 = try cview.nextChunkBatch(&ents, &col_ptrs, 0x1);
    try std.testing.expectEqual(@as(?usize, 2), n1);
    try std.testing.expectEqual(@as(usize, 2), cview.len);
    try std.testing.expectEqual(@as(u32, 0x1), state.last_mask);
    try std.testing.expectEqual(@as(usize, 2), state.last_col_count);
    try std.testing.expectEqual(@as(c.BamEntity, 101), ents.?[0]);
    try std.testing.expectEqual(@as(c.BamEntity, 202), ents.?[1]);
    const c0: [*]const f32 = @ptrCast(@alignCast(col_ptrs[0].?));
    const c1: [*]const f32 = @ptrCast(@alignCast(col_ptrs[1].?));
    try std.testing.expectEqual(@as(f32, 1.5), c0[0]);
    try std.testing.expectEqual(@as(f32, 2.5), c0[1]);
    try std.testing.expectEqual(@as(f32, 3.5), c1[0]);
    try std.testing.expectEqual(@as(f32, 4.5), c1[1]);

    // Exhaustion returns null and zeroes cview.len, ents, and col_ptrs.
    const n2 = try cview.nextChunkBatch(&ents, &col_ptrs, 0x1);
    try std.testing.expectEqual(@as(?usize, null), n2);
    try std.testing.expectEqual(@as(usize, 0), cview.len);
    try std.testing.expectEqual(@as(?[*]const c.BamEntity, null), ents);
    try std.testing.expectEqual(@as(?[*]u8, null), col_ptrs[0]);
    try std.testing.expectEqual(@as(?[*]u8, null), col_ptrs[1]);

    // Unchecked path with out_entities == null and col_count == 2.
    try cview.reset();
    const un1 = cview.nextChunkBatchUnchecked(null, &col_ptrs, 0x3);
    try std.testing.expectEqual(@as(?usize, 2), un1);
    try std.testing.expectEqual(@as(usize, 2), cview.len);
    try std.testing.expectEqual(@as(u32, 0x3), state.last_mask);
    try std.testing.expect(col_ptrs[0] != null and col_ptrs[1] != null);
    const un2 = cview.nextChunkBatchUnchecked(null, &col_ptrs, 0x3);
    try std.testing.expectEqual(@as(?usize, null), un2);
    try std.testing.expectEqual(@as(usize, 0), cview.len);
    try std.testing.expectEqual(@as(?[*]u8, null), col_ptrs[0]);
    try std.testing.expectEqual(@as(?[*]u8, null), col_ptrs[1]);

    // col_count == 0 (empty slice).
    try cview.reset();
    var empty_cols = [_]?[*]u8{};
    const z1 = try cview.nextChunkBatch(&ents, &empty_cols, 0x0);
    try std.testing.expectEqual(@as(?usize, 2), z1);
    try std.testing.expectEqual(@as(usize, 0), state.last_col_count);
    try std.testing.expectEqual(@as(c.BamEntity, 101), ents.?[0]);
    const z2 = try cview.nextChunkBatch(&ents, &empty_cols, 0x0);
    try std.testing.expectEqual(@as(?usize, null), z2);
    try std.testing.expectEqual(@as(?[*]const c.BamEntity, null), ents);

    // Null iter_next_chunk_batch slot returns error.Unsupported.
    var null_api = api;
    null_api.iter_next_chunk_batch = null;
    var null_cv = bam.ChunkView{ .api = &null_api, .iter = @ptrCast(&state) };
    try std.testing.expect(!null_cv.supportsNextChunkBatch());
    try std.testing.expectError(error.Unsupported, null_cv.nextChunkBatch(null, &col_ptrs, 0x1));

    // BAM_HOST_API_MIN_V0_3_SIZE-only host (even with non-null fn ptr) returns error.Unsupported without invoking slot 68.
    const min_v03_size: u32 = @intCast(bam.offsets.MIGRATE_COMPONENT_COLUMN + @sizeOf(c.BamMigrateComponentColumnFn));
    try std.testing.expectEqual(bam.offsets.ITER_NEXT_CHUNK_BATCH, @as(usize, min_v03_size));
    const calls_before = state.calls;
    var v03_api = api;
    v03_api.struct_size = min_v03_size;
    var v03_cv = bam.ChunkView{ .api = &v03_api, .iter = @ptrCast(&state) };
    try std.testing.expect(!v03_cv.supportsNextChunkBatch());
    try std.testing.expectError(error.Unsupported, v03_cv.nextChunkBatch(&ents, &col_ptrs, 0x1));
    try std.testing.expectEqual(calls_before, state.calls);
}
"#,
    )
    .expect("write test_zig.zig");

    let zig_out = Command::new("zig")
        .args(["build", "test"])
        .current_dir(tmp.path())
        .env("ZIG_GLOBAL_CACHE_DIR", &zig_cache)
        .output()
        .expect("invoke zig build test");
    assert!(
        zig_out.status.success(),
        "Zig SDK build/test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&zig_out.stdout),
        String::from_utf8_lossy(&zig_out.stderr),
    );
}

/// Verifies `bamabi.h` compiles cleanly under strict 32-bit `i686` ISO C99 (`-std=c99 -pedantic -Werror`),
/// proving `BAM_ALIGNOF(uint64_t)` matches C target ABI alignment (`4` on `i686`) rather than GCC's
/// preferred `__alignof__(uint64_t) == 8`.
#[test]
fn bamabi_h_compiles_for_32bit_i686_c99_pedantic() {
    let Ok(ver_out) = Command::new("zig").arg("version").output() else {
        return;
    };
    if !ver_out.status.success() {
        return;
    }

    let tmp = TempDirGuard::new("bamabi_i686_c99_test");
    bamabi_sdk_gen::export_sdk(tmp.path()).expect("export_sdk must succeed");
    let zig_cache = tmp.path().join("zig-global-cache");
    fs::create_dir_all(&zig_cache).expect("create zig cache dir");

    let c_src = tmp.path().join("verify_i686.c");
    let obj_out = tmp.path().join("verify_i686.o");
    fs::write(
        &c_src,
        r#"#include "bamabi.h"
#include <stdint.h>

BAM_STATIC_ASSERT(sizeof(uintptr_t) == 4u, i686_ptr_size);
BAM_STATIC_ASSERT(BAM_ALIGNOF(uint64_t) == 4u, i686_u64_abi_align);
BAM_STATIC_ASSERT(sizeof(BamHostApi) == 368u, i686_host_api_size);

int bamabi_i686_c99_smoke(void) {
    return (int)bam_version_is_compatible(BAM_ABI_VERSION, BAM_ABI_VERSION);
}
"#,
    )
    .expect("write verify_i686.c");

    let cc_out = Command::new("zig")
        .args([
            "cc",
            "-target",
            "x86-linux-gnu",
            "-std=c99",
            "-Wall",
            "-Wextra",
            "-Wpedantic",
            "-Werror",
            "-I",
            tmp.path().to_str().expect("utf8 path"),
            "-c",
            c_src.to_str().expect("utf8 path"),
            "-o",
            obj_out.to_str().expect("utf8 path"),
        ])
        .env("ZIG_GLOBAL_CACHE_DIR", &zig_cache)
        .output()
        .expect("invoke zig cc");
    assert!(
        cc_out.status.success(),
        "i686 C99 pedantic compile failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&cc_out.stdout),
        String::from_utf8_lossy(&cc_out.stderr),
    );
}

/// Verifies `component_section_for::<T>()`, `ModComponent::BAM_FIELD_META`, `render_zig_component_header`,
/// `FfiDisabled` ZST exclusion, `BamRowRange`/`BamTimeInfo` mapping, `#[repr(u8)]` enum ZIG_DECL,
/// and `bamabi.zig` `Bam*`/`BAM_*`/`bam_version_is_compatible` re-exports against Zig `0.16`.
#[test]
fn rendered_zig_component_header_and_enum_decls_compile_and_pass_zig_0_16() {
    let Ok(ver_out) = Command::new("zig").arg("version").output() else {
        return;
    };
    if !ver_out.status.success() {
        return;
    }

    #[derive(bamabi_macros::ModComponent)]
    #[mod_component(name = "zig_test::V04Widget")]
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct V04Widget {
        disabled: bamabi_sys::FfiDisabled,
        rows: bamabi_sys::BamRowRange,
        timing: bamabi_sys::BamTimeInfo,
        count: u32,
        reserved0: u32,
    }

    #[derive(bamabi_macros::ModComponent)]
    #[mod_component(name = "zig_test::WidgetMode")]
    #[repr(u8)]
    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    enum WidgetMode {
        #[default]
        Idle = 0,
        Active = 1,
    }

    let sections = [
        bamabi_sdk_gen::component_section_for::<V04Widget>(),
        bamabi_sdk_gen::component_section_for::<WidgetMode>(),
    ];
    assert!(
        !sections[0].fields.is_empty(),
        "component_section_for must populate fields from T::BAM_FIELD_META"
    );
    let zig_comp = bamabi_sdk_gen::render_zig_component_header("bamabi", &sections);
    assert!(zig_comp.contains("const std = @import(\"std\");"));
    assert!(zig_comp.contains("bam.BamRowRange"));
    assert!(zig_comp.contains("bam.BamTimeInfo"));
    assert!(zig_comp.contains(".@\"align\" = "));
    assert!(!zig_comp.contains("    disabled:"));

    let tmp = TempDirGuard::new("bamabi_zig_comp_test");
    bamabi_sdk_gen::export_sdk(tmp.path()).expect("export_sdk must succeed");
    fs::write(tmp.path().join("components.zig"), &zig_comp).expect("write components.zig");
    let zig_cache = tmp.path().join("zig-global-cache");
    fs::create_dir_all(&zig_cache).expect("create zig cache dir");

    fs::write(
        tmp.path().join("build.zig"),
        r#"const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const tc = b.addTranslateC(.{
        .root_source_file = b.path("bamabi.h"),
        .target = target,
        .optimize = optimize,
    });
    const bam_c_mod = tc.createModule();

    const bam_mod = b.createModule(.{
        .root_source_file = b.path("bamabi.zig"),
        .target = target,
        .optimize = optimize,
        .imports = &.{
            .{ .name = "bam_c", .module = bam_c_mod },
        },
    });

    const comp_mod = b.createModule(.{
        .root_source_file = b.path("components.zig"),
        .target = target,
        .optimize = optimize,
        .imports = &.{
            .{ .name = "bamabi", .module = bam_mod },
        },
    });

    const test_mod = b.createModule(.{
        .root_source_file = b.path("test_comp.zig"),
        .target = target,
        .optimize = optimize,
        .imports = &.{
            .{ .name = "bamabi", .module = bam_mod },
            .{ .name = "components", .module = comp_mod },
        },
    });

    const tests = b.addTest(.{ .root_module = test_mod });
    const run_tests = b.addRunArtifact(tests);
    const test_step = b.step("test", "Run Zig component header tests");
    test_step.dependOn(&run_tests.step);
}
"#,
    )
    .expect("write build.zig");

    fs::write(
        tmp.path().join("test_comp.zig"),
        r#"const std = @import("std");
const bam = @import("bamabi");
const comp = @import("components");

test "re-exported bamabi.zig symbols and rendered component headers compile on Zig 0.16" {
    try std.testing.expectEqual(@as(u32, 90), bam.BAM_HOST_API_SLOT_COUNT);
    try std.testing.expect(bam.bam_version_is_compatible(bam.BAM_ABI_VERSION, bam.BAM_ABI_VERSION) != 0);
    const rr = bam.BamRowRange{ .start_row = 1, .row_count = 3 };
    const ti = std.mem.zeroes(bam.BamTimeInfo);
    const w = comp.V04Widget{
        .rows = rr,
        .timing = ti,
        .count = 7,
        .reserved0 = 0,
    };
    try std.testing.expectEqual(@as(u32, 7), w.count);
    try std.testing.expectEqual(@as(usize, comp.ZigTestV04WidgetMeta.field_count), comp.ZigTestV04WidgetMeta.fields.len);
    try std.testing.expect(comp.ZigTestV04WidgetMeta.fields.len == 5);
    try std.testing.expectEqual(comp.WidgetMode.Idle, @as(comp.WidgetMode, @enumFromInt(0)));
    try std.testing.expectEqual(comp.WidgetMode.Active, @as(comp.WidgetMode, @enumFromInt(1)));
}
"#,
    )
    .expect("write test_comp.zig");

    let zig_out = Command::new("zig")
        .args(["build", "test"])
        .current_dir(tmp.path())
        .env("ZIG_GLOBAL_CACHE_DIR", &zig_cache)
        .output()
        .expect("invoke zig build test");
    assert!(
        zig_out.status.success(),
        "Zig component header build/test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&zig_out.stdout),
        String::from_utf8_lossy(&zig_out.stderr),
    );
}
