const std = @import("std");

fn resolveSdkLazyPath(b: *std.Build, sdk_dir: []const u8, filename: []const u8) std.Build.LazyPath {
    const joined = b.pathJoin(&.{ sdk_dir, filename });
    if (std.fs.path.isAbsolute(sdk_dir)) {
        return .{ .cwd_relative = joined };
    }
    return b.path(joined);
}

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const sdk_dir = b.option(
        []const u8,
        "sdk-dir",
        "Path to exported bamabi SDK directory containing bamabi.h and bamabi.zig",
    ) orelse "../../../target/bench_sdk";

    const tc = b.addTranslateC(.{
        .root_source_file = resolveSdkLazyPath(b, sdk_dir, "bamabi.h"),
        .target = target,
        .optimize = optimize,
    });
    const bam_c_mod = tc.createModule();

    const bam_mod = b.createModule(.{
        .root_source_file = resolveSdkLazyPath(b, sdk_dir, "bamabi.zig"),
        .target = target,
        .optimize = optimize,
        .imports = &.{
            .{ .name = "bam_c", .module = bam_c_mod },
        },
    });

    const guest_mod = b.createModule(.{
        .root_source_file = b.path("bench_guest.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
        .imports = &.{
            .{ .name = "bamabi", .module = bam_mod },
            .{ .name = "bam_c", .module = bam_c_mod },
        },
    });

    const lib = b.addLibrary(.{
        .name = "bench_zig",
        .linkage = .dynamic,
        .root_module = guest_mod,
    });
    b.installArtifact(lib);
}
