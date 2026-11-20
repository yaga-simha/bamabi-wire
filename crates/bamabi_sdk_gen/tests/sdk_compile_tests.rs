//! Polyglot compiler verification tests for `bamabi_sdk_gen` (`bamabi.h`, `bamabi.hpp`, `bamabi.zig`).
//!
//! Exports the ABI `0.3.0` SDK bundle to a temporary directory and compiles/executes strict
//! ISO C++17 (`-std=c++17 -Wall -Wextra -Wpedantic -Werror`) and Zig (`b.addTranslateC`)
//! verification harnesses when the host toolchain provides `c++` / `zig`.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

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
struct BamIter { size_t step; BamEntity ents[1]; float col[1]; };
struct BamCommands { size_t checkpoints; size_t rollbacks; size_t last_token; };
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
static BamResult mock_iter_reset(BamIter* it) { it->step = 0u; return BAM_RESULT_OK; }
static const BamEntity* mock_iter_entities(const BamIter* it) { return it->ents; }
static const uint8_t* mock_iter_col_ro(const BamIter* it, size_t) { return reinterpret_cast<const uint8_t*>(it->col); }
static uint8_t* mock_iter_col_mut(BamIter* it, size_t) { return reinterpret_cast<uint8_t*>(it->col); }
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
    api.iter_reset = mock_iter_reset;
    api.iter_entities = mock_iter_entities;
    api.iter_column_readonly = mock_iter_col_ro;
    api.iter_column_mutable = mock_iter_col_mut;
    api.iter_column_mutable_untracked = mock_iter_col_mut;
    api.cmd_checkpoint = mock_cmd_checkpoint;
    api.cmd_rollback = mock_cmd_rollback;

    BamHostCtx h{42};
    BamLoadCtx lc{&h};
    BamIter it{0u, {7ull}, {3.5f}};
    BamCommands cmds{0u, 0u, 0u};
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
    assert(bam::is_ok(cview.reset()));

    bam::CommandsView cmd_view;
    assert(bam::is_ok(sview.commands_view(cmd_view)));
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
/// verifying all 18 `BamResult` round-trips and semantic compilation of all 68 slot wrappers on
/// `HostContextView`, `LoadContextView`, `ChunkView`, `CommandsView`, and `SystemContextView`.
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

    _ = &bam.LoadContextView.hostCtx;
    _ = &bam.LoadContextView.registerComponent;
    _ = &bam.LoadContextView.registerSystem;
    _ = &bam.LoadContextView.registerObserver;
    _ = &bam.LoadContextView.registerRelation;
    _ = &bam.LoadContextView.registerSystemHandle;
    _ = &bam.LoadContextView.configureScheduleSet;
    _ = &bam.LoadContextView.mountSystemInSchedule;
    _ = &bam.LoadContextView.migrateComponentColumn;

    _ = &bam.ChunkView.nextChunk;
    _ = &bam.ChunkView.reset;
    _ = &bam.ChunkView.entities;
    _ = &bam.ChunkView.markChangedRange;
    _ = &bam.ChunkView.columnRowsReadonly;
    _ = &bam.ChunkView.columnRowsMutable;
    _ = &bam.ChunkView.columnInfo;
    _ = &bam.ChunkView.resTicks;
    _ = &bam.ChunkView.parForEachChunk;
    _ = &bam.ChunkView.parForEachChunkReadonly;
    _ = &bam.ChunkView.msgSend;
    _ = &bam.ChunkView.readRemoved;
    _ = &bam.ChunkView.totalCount;
    _ = &bam.ChunkView.upCascade;

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

    _ = &bam.SystemContextView.hostCtx;
    _ = &bam.SystemContextView.query;
    _ = &bam.SystemContextView.commands;
    _ = &bam.SystemContextView.entityChildren;

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

    const sctx: *c.BamSystemCtx = @ptrCast(&dummy_byte);
    const sv = bam.SystemContextView{ .api = &api, .ctx = sctx };
    try std.testing.expectEqual(@as(?*const f32, null), sv.entityGet(f32, 0, 1, 0));
    try std.testing.expectEqual(@as(?*f32, null), sv.entityGetMut(f32, 0, 1, 0));
    try std.testing.expectEqual(@as(?*const f32, null), sv.resPtr(f32, 1, null));
    try std.testing.expectEqual(@as(?*f32, null), sv.resMutPtr(f32, 1, null));
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
