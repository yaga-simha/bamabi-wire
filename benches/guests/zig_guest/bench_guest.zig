const std = @import("std");
const bam = @import("bamabi");
const c = @import("bam_c");

const BenchPosition = extern struct {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
};

const BenchVelocity = extern struct {
    vx: f32,
    vy: f32,
    vz: f32,
    vw: f32,
};

const BenchSimParams = extern struct {
    dt: f32,
    damping: f32,
    tick: u32,
    _pad: u32,
};

comptime {
    if (@sizeOf(BenchPosition) != 16 or @alignOf(BenchPosition) != 4) {
        @compileError("BenchPosition layout mismatch");
    }
    if (@sizeOf(BenchVelocity) != 16 or @alignOf(BenchVelocity) != 4) {
        @compileError("BenchVelocity layout mismatch");
    }
    if (@sizeOf(BenchSimParams) != 16 or @alignOf(BenchSimParams) != 4) {
        @compileError("BenchSimParams layout mismatch");
    }
}

const BenchInstance = struct {
    api: c.BamHostApi,
    host_ctx: ?*const c.BamHostCtx,
    pos_comp: c.BamComponent,
    vel_comp: c.BamComponent,
    sim_comp: c.BamComponent,
    dense_terms: [2]c.BamTermDesc,
    dense_query: [1]c.BamQueryDesc,
    roundtrip_terms: [1]c.BamTermDesc,
    roundtrip_query: [1]c.BamQueryDesc,
    sink: u64,
};

const g_manifest: c.BamManifest = .{
    .struct_size = @sizeOf(c.BamManifest),
    .abi_version = c.BAM_ABI_VERSION,
    .name = "bench_guest_zig",
    .version = "0.3.0",
    .flags = 0,
    .reserved0 = 0,
    .save_state = null,
    .restore_state = null,
};

const SipHasher128 = struct {
    v0: u64,
    v1: u64,
    v2: u64,
    v3: u64,
    tail: [8]u8,
    tail_len: usize,
    total_len: usize,

    fn init(k0: u64, k1: u64) SipHasher128 {
        return .{
            .v0 = 0x736f6d6570736575 ^ k0,
            .v1 = (0x646f72616e646f6d ^ k1) ^ 0xee,
            .v2 = 0x6c7967656e657261 ^ k0,
            .v3 = 0x7465646279746573 ^ k1,
            .tail = [_]u8{0} ** 8,
            .tail_len = 0,
            .total_len = 0,
        };
    }

    inline fn rotl64(x: u64, comptime bits: u6) u64 {
        return (x << bits) | (x >> (@as(u6, @intCast(64 - @as(u7, bits)))));
    }

    inline fn sipRound(self: *SipHasher128) void {
        self.v0 +%= self.v1;
        self.v1 = rotl64(self.v1, 13) ^ self.v0;
        self.v0 = rotl64(self.v0, 32);
        self.v2 +%= self.v3;
        self.v3 = rotl64(self.v3, 16) ^ self.v2;
        self.v0 +%= self.v3;
        self.v3 = rotl64(self.v3, 21) ^ self.v0;
        self.v2 +%= self.v1;
        self.v1 = rotl64(self.v1, 17) ^ self.v2;
        self.v2 = rotl64(self.v2, 32);
    }

    fn write(self: *SipHasher128, bytes: []const u8) void {
        var idx: usize = 0;
        self.total_len += bytes.len;
        if (self.tail_len > 0) {
            while (idx < bytes.len and self.tail_len < 8) {
                self.tail[self.tail_len] = bytes[idx];
                self.tail_len += 1;
                idx += 1;
            }
            if (self.tail_len == 8) {
                const m = std.mem.readInt(u64, &self.tail, .little);
                self.v3 ^= m;
                self.sipRound();
                self.sipRound();
                self.v0 ^= m;
                self.tail_len = 0;
            }
        }
        while (idx + 8 <= bytes.len) {
            const m = std.mem.readInt(u64, bytes[idx..][0..8], .little);
            self.v3 ^= m;
            self.sipRound();
            self.sipRound();
            self.v0 ^= m;
            idx += 8;
        }
        while (idx < bytes.len and self.tail_len < 8) {
            self.tail[self.tail_len] = bytes[idx];
            self.tail_len += 1;
            idx += 1;
        }
    }

    fn finish(self: *SipHasher128) [16]u8 {
        var b: u64 = (@as(u64, self.total_len) & 0xFF) << 56;
        for (0..self.tail_len) |i| {
            b |= @as(u64, self.tail[i]) << @intCast(8 * i);
        }
        self.v3 ^= b;
        self.sipRound();
        self.sipRound();
        self.v0 ^= b;
        self.v2 ^= 0xee;
        self.sipRound();
        self.sipRound();
        self.sipRound();
        self.sipRound();
        const h0 = self.v0 ^ self.v1 ^ self.v2 ^ self.v3;
        self.v1 ^= 0xdd;
        self.sipRound();
        self.sipRound();
        self.sipRound();
        self.sipRound();
        const h1 = self.v0 ^ self.v1 ^ self.v2 ^ self.v3;
        var out: [16]u8 = undefined;
        std.mem.writeInt(u64, out[0..8], h0, .little);
        std.mem.writeInt(u64, out[8..16], h1, .little);
        return out;
    }
};

fn bamUuidFromParts(
    canonical_name: []const u8,
    size: usize,
    align_val: usize,
    field_schema_hash: u64,
) c.BamComponentUuid {
    var st = SipHasher128.init(c.BAM_SIPHASH_KEY_0, c.BAM_SIPHASH_KEY_1);
    if (canonical_name.len > 0) {
        st.write(canonical_name);
    }
    const sep = [_]u8{0xFF};
    st.write(&sep);
    var le_buf: [8]u8 = undefined;
    std.mem.writeInt(u64, &le_buf, @as(u64, size), .little);
    st.write(&le_buf);
    std.mem.writeInt(u64, &le_buf, @as(u64, align_val), .little);
    st.write(&le_buf);
    std.mem.writeInt(u64, &le_buf, field_schema_hash, .little);
    st.write(&le_buf);
    return .{ .bytes = st.finish() };
}

fn integrateChunk(
    noalias pos: [*]BenchPosition,
    noalias vel: [*]const BenchVelocity,
    len: usize,
) void {
    const dt: f32 = 0.016;
    for (0..len) |i| {
        pos[i].x += vel[i].vx * dt;
        pos[i].y += vel[i].vy * dt;
        pos[i].z += vel[i].vz * dt;
        pos[i].w += vel[i].vw * dt;
    }
}

fn resolveOrRegisterPod(
    lview: bam.LoadContextView,
    hview: bam.HostContextView,
    name: [:0]const u8,
    size: usize,
    align_val: usize,
) bam.BamError!c.BamComponent {
    const uuid = bamUuidFromParts(name, size, align_val, 0);
    return hview.resolveComponent(name.ptr, &uuid) catch |err| switch (err) {
        error.NotFound => blk: {
            var desc: c.BamComponentDesc = std.mem.zeroes(c.BamComponentDesc);
            desc.struct_size = @sizeOf(c.BamComponentDesc);
            desc.flags = 0;
            desc.name = name.ptr;
            desc.uuid = uuid;
            desc.size = size;
            desc.@"align" = align_val;
            break :blk try lview.registerComponent(&desc);
        },
        else => return err,
    };
}

fn sysNoop(_: ?*c.BamSystemCtx, _: ?*anyopaque) callconv(.c) c.BamResult {
    return c.BAM_RESULT_OK;
}

fn sysDenseUpdate(sys_ctx_opt: ?*c.BamSystemCtx, user_data: ?*anyopaque) callconv(.c) c.BamResult {
    const inst: *BenchInstance = @ptrCast(@alignCast(user_data orelse return c.BAM_RESULT_NULL_POINTER));
    const sctx = sys_ctx_opt orelse return c.BAM_RESULT_NULL_POINTER;
    const sview = bam.SystemContextView{ .api = &inst.api, .ctx = sctx };
    var cview = sview.query(0) catch |err| return bam.mapError(err);

    while (cview.nextChunk()) |chunk_len| {
        if (chunk_len == 0) continue;
        const pos = cview.columnMutable(BenchPosition, 0) orelse return c.BAM_RESULT_NULL_POINTER;
        const vel = cview.columnReadonly(BenchVelocity, 1) orelse return c.BAM_RESULT_NULL_POINTER;
        integrateChunk(pos, vel, chunk_len);
    }
    return c.BAM_RESULT_OK;
}

fn sysCommandBurst(sys_ctx_opt: ?*c.BamSystemCtx, user_data: ?*anyopaque) callconv(.c) c.BamResult {
    const inst: *BenchInstance = @ptrCast(@alignCast(user_data orelse return c.BAM_RESULT_NULL_POINTER));
    const sctx = sys_ctx_opt orelse return c.BAM_RESULT_NULL_POINTER;
    const sview = bam.SystemContextView{ .api = &inst.api, .ctx = sctx };
    const cmds = sview.commands() catch |err| return bam.mapError(err);

    const sample_pos = BenchPosition{ .x = 1.0, .y = 2.0, .z = 3.0, .w = 4.0 };
    var dummy_comp: c.BamComponent = c.BAM_COMPONENT_INVALID;
    var dummy_ptr: ?[*]const u8 = null;
    const empty_comps: [*]const c.BamComponent = @ptrCast(&dummy_comp);
    const empty_ptrs: [*]const ?[*]const u8 = @ptrCast(&dummy_ptr);

    for (0..64) |_| {
        const ent = cmds.spawn(empty_comps, empty_ptrs, 0) catch |err| return bam.mapError(err);
        cmds.insert(ent, inst.pos_comp, std.mem.asBytes(&sample_pos)) catch |err| {
            return bam.mapError(err);
        };
    }

    const token = cmds.checkpoint() catch |err| return bam.mapError(err);
    _ = cmds.spawn(empty_comps, empty_ptrs, 0) catch |err| {
        cmds.rollback(token) catch {};
        return bam.mapError(err);
    };
    cmds.rollback(token) catch |err| return bam.mapError(err);
    return c.BAM_RESULT_OK;
}

fn sysHostApiRoundtrip(sys_ctx_opt: ?*c.BamSystemCtx, user_data: ?*anyopaque) callconv(.c) c.BamResult {
    const inst: *BenchInstance = @ptrCast(@alignCast(user_data orelse return c.BAM_RESULT_NULL_POINTER));
    const sctx = sys_ctx_opt orelse return c.BAM_RESULT_NULL_POINTER;
    const sview = bam.SystemContextView{ .api = &inst.api, .ctx = sctx };
    const cview = sview.query(0) catch |err| return bam.mapError(err);
    const hview = sview.hostCtx() catch bam.HostContextView{
        .api = &inst.api,
        .ctx = inst.host_ctx orelse return c.BAM_RESULT_NULL_POINTER,
    };

    var acc: u64 = 0;
    for (0..64) |_| {
        const params = cview.resPtr(BenchSimParams, 0) orelse return c.BAM_RESULT_NOT_FOUND;
        const ticks = cview.resTicks(0) catch |err| return bam.mapError(err);
        const sym = hview.internSymbol("bench.sym") catch |err| return bam.mapError(err);
        const dt_bits: u32 = @bitCast(params.dt);
        acc +%= @as(u64, ticks.this_run) +% @as(u64, params.tick) +% @as(u64, dt_bits) +% @as(u64, sym);
    }

    const sink_ptr: *volatile u64 = &inst.sink;
    sink_ptr.* = acc;
    return c.BAM_RESULT_OK;
}

fn registerBenchSystem(
    inst: *BenchInstance,
    lview: bam.LoadContextView,
    name: [*:0]const u8,
    queries: ?[*]const c.BamQueryDesc,
    query_count: usize,
    callback: c.BamSystemFn,
) bam.BamError!void {
    var sys_desc: c.BamSystemDesc = std.mem.zeroes(c.BamSystemDesc);
    sys_desc.struct_size = @sizeOf(c.BamSystemDesc);
    sys_desc.flags = 0;
    sys_desc.name = name;
    sys_desc.queries = queries;
    sys_desc.query_count = query_count;
    sys_desc.callback = callback;
    sys_desc.user_data = inst;
    sys_desc.schedule = c.BAM_SCHEDULE_UPDATE;
    return lview.registerSystem(&sys_desc);
}

export fn bamabi_manifest() callconv(.c) *const c.BamManifest {
    return &g_manifest;
}

export fn bamabi_init(
    api_opt: ?*const c.BamHostApi,
    load_ctx_opt: ?*c.BamLoadCtx,
    instance_out_opt: ?*?*c.BamInstance,
) callconv(.c) c.BamResult {
    const api_ptr = api_opt orelse return c.BAM_RESULT_NULL_POINTER;
    const load_ctx = load_ctx_opt orelse return c.BAM_RESULT_NULL_POINTER;
    const instance_out = instance_out_opt orelse return c.BAM_RESULT_NULL_POINTER;
    instance_out.* = null;

    const inst = std.heap.page_allocator.create(BenchInstance) catch {
        return c.BAM_RESULT_INTERNAL_ERROR;
    };
    errdefer std.heap.page_allocator.destroy(inst);
    inst.* = .{
        .api = std.mem.zeroes(c.BamHostApi),
        .host_ctx = null,
        .pos_comp = c.BAM_COMPONENT_INVALID,
        .vel_comp = c.BAM_COMPONENT_INVALID,
        .sim_comp = c.BAM_COMPONENT_INVALID,
        .dense_terms = std.mem.zeroes([2]c.BamTermDesc),
        .dense_query = std.mem.zeroes([1]c.BamQueryDesc),
        .roundtrip_terms = std.mem.zeroes([1]c.BamTermDesc),
        .roundtrip_query = std.mem.zeroes([1]c.BamQueryDesc),
        .sink = 0,
    };

    const copy_rc = c.bam_host_api_copy_bounded(&inst.api, @sizeOf(c.BamHostApi), api_ptr);
    if (copy_rc != c.BAM_RESULT_OK) {
        return copy_rc;
    }

    const lview = bam.LoadContextView{ .api = &inst.api, .ctx = load_ctx };
    const hview = lview.hostCtx() catch |err| return bam.mapError(err);
    inst.host_ctx = hview.ctx;

    inst.pos_comp = resolveOrRegisterPod(
        lview,
        hview,
        "bench::Position",
        @sizeOf(BenchPosition),
        @alignOf(BenchPosition),
    ) catch |err| return bam.mapError(err);
    inst.vel_comp = resolveOrRegisterPod(
        lview,
        hview,
        "bench::Velocity",
        @sizeOf(BenchVelocity),
        @alignOf(BenchVelocity),
    ) catch |err| return bam.mapError(err);
    inst.sim_comp = resolveOrRegisterPod(
        lview,
        hview,
        "bench::SimParams",
        @sizeOf(BenchSimParams),
        @alignOf(BenchSimParams),
    ) catch |err| return bam.mapError(err);

    inst.dense_terms[0] = c.bam_term_desc(inst.pos_comp, c.BAM_ACCESS_WRITE, 0);
    inst.dense_terms[1] = c.bam_term_desc(inst.vel_comp, c.BAM_ACCESS_READ, 0);
    inst.dense_query[0] = .{
        .struct_size = @sizeOf(c.BamQueryDesc),
        .flags = 0,
        .terms = &inst.dense_terms,
        .term_count = 2,
    };

    inst.roundtrip_terms[0] = c.bam_term_desc(inst.sim_comp, c.BAM_ACCESS_RESOURCE_READ, 0);
    inst.roundtrip_query[0] = .{
        .struct_size = @sizeOf(c.BamQueryDesc),
        .flags = 0,
        .terms = &inst.roundtrip_terms,
        .term_count = 1,
    };

    registerBenchSystem(inst, lview, "bench_noop", null, 0, sysNoop) catch |err| {
        return bam.mapError(err);
    };
    registerBenchSystem(inst, lview, "bench_dense_update", &inst.dense_query, 1, sysDenseUpdate) catch |err| {
        return bam.mapError(err);
    };
    registerBenchSystem(inst, lview, "bench_command_burst", null, 0, sysCommandBurst) catch |err| {
        return bam.mapError(err);
    };
    registerBenchSystem(
        inst,
        lview,
        "bench_host_api_roundtrip",
        &inst.roundtrip_query,
        1,
        sysHostApiRoundtrip,
    ) catch |err| return bam.mapError(err);

    instance_out.* = @ptrCast(inst);
    return c.BAM_RESULT_OK;
}

export fn bamabi_shutdown(instance_opt: ?*c.BamInstance) callconv(.c) void {
    const raw = instance_opt orelse return;
    const inst: *BenchInstance = @ptrCast(@alignCast(raw));
    std.heap.page_allocator.destroy(inst);
}
