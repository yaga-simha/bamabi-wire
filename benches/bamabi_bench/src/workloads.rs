//! Benchmark workloads, native Rust baseline, strict correctness checks, and statistics.

use core::{
    ffi::{CStr, c_void},
    hint::black_box,
    mem::MaybeUninit,
    sync::atomic::{AtomicU64, Ordering},
};
use std::{path::PathBuf, sync::Arc, time::Instant};

use bamabi_guest::prelude::*;
use bamabi_sys::{BamQueryDesc, BamSystemDesc, BamTermDesc};

use crate::{
    host::{BENCH_DT, BenchMessage, HostWorld, OwnedHostWorld, Position, Velocity},
    loader::LoadedGuest,
};

/// Guest-side POD message mirror matching [`BenchMessage`] (`16` bytes, align `8`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, AbiPod, GuestComponent)]
#[guest_component(name = "bench::BenchMessage", no_schema_hash)]
pub(crate) struct GuestBenchMsg {
    pub(crate) seq: u64,
    pub(crate) payload: u32,
    pub(crate) flags: u32,
}

const _: () = {
    assert!(core::mem::size_of::<GuestBenchMsg>() == 16);
    assert!(core::mem::align_of::<GuestBenchMsg>() == 8);
};

/// Supported benchmark workload identifiers.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) enum WorkloadKind {
    Noop,
    DenseQueryUpdate,
    CommandBurst,
    HostApiRoundtrip,
    MessageIdlePoll,
    MessageBurst256,
    MessageBurst4096,
}

impl WorkloadKind {
    /// Default four benchmark workloads in canonical execution order.
    pub(crate) const ALL: [Self; 4] = [
        Self::Noop,
        Self::DenseQueryUpdate,
        Self::CommandBurst,
        Self::HostApiRoundtrip,
    ];

    /// All three message-streaming workloads (`message` / `message_burst` alias).
    pub(crate) const ALL_MESSAGES: [Self; 3] = [
        Self::MessageIdlePoll,
        Self::MessageBurst256,
        Self::MessageBurst4096,
    ];

    /// Parses a CLI workload identifier.
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            "noop" => Ok(Self::Noop),
            "dense_query_update" => Ok(Self::DenseQueryUpdate),
            "command_burst" => Ok(Self::CommandBurst),
            "host_api_roundtrip" => Ok(Self::HostApiRoundtrip),
            "message_idle_poll" => Ok(Self::MessageIdlePoll),
            "message_burst_256" => Ok(Self::MessageBurst256),
            "message_burst_4096" => Ok(Self::MessageBurst4096),
            other => Err(format!(
                "unknown workload `{other}` (expected one of: noop, dense_query_update, command_burst, host_api_roundtrip, message_idle_poll, message_burst_256, message_burst_4096, message, message_burst)"
            )),
        }
    }

    /// Canonical CLI / JSON workload identifier string.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Noop => "noop",
            Self::DenseQueryUpdate => "dense_query_update",
            Self::CommandBurst => "command_burst",
            Self::HostApiRoundtrip => "host_api_roundtrip",
            Self::MessageIdlePoll => "message_idle_poll",
            Self::MessageBurst256 => "message_burst_256",
            Self::MessageBurst4096 => "message_burst_4096",
        }
    }
}

/// Target guest specification parsed from `--guest <lang>:<profile>=<path>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuestSpec {
    pub(crate) lang: String,
    pub(crate) profile: String,
    pub(crate) path: PathBuf,
}

impl GuestSpec {
    /// Returns `<lang>:<profile>`.
    pub(crate) fn label(&self) -> String {
        format!("{}:{}", self.lang, self.profile)
    }
}

/// Top-level benchmark execution configuration.
#[derive(Debug, Clone)]
pub(crate) struct BenchConfig {
    pub(crate) guests: Vec<GuestSpec>,
    pub(crate) workloads: Vec<WorkloadKind>,
    pub(crate) entities: Vec<usize>,
    pub(crate) chunks: Vec<usize>,
    pub(crate) quick: bool,
    pub(crate) warmup_samples: usize,
    pub(crate) timed_samples: usize,
}

/// Descriptive timing statistics computed over `timed_samples` runs (`ns/op`).
#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) struct TimingStats {
    pub(crate) min_ns: f64,
    pub(crate) median_ns: f64,
    pub(crate) p95_ns: f64,
    pub(crate) mean_ns: f64,
}

/// Computes `min`, `median`, `p95`, and `mean` from non-empty sample timings (`ns`).
pub(crate) fn compute_stats(samples_ns: &[f64]) -> TimingStats {
    assert!(!samples_ns.is_empty(), "samples_ns must not be empty");
    let mut sorted = samples_ns.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let min_ns = sorted[0];
    let median_ns = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) * 0.5
    };
    let p95_idx = ((0.95 * (n - 1) as f64).ceil() as usize).min(n - 1);
    let p95_ns = sorted[p95_idx];
    let sum: f64 = sorted.iter().sum();
    let mean_ns = sum / (n as f64);
    TimingStats {
        min_ns,
        median_ns,
        p95_ns,
        mean_ns,
    }
}

/// Single measured benchmark row emitted in Markdown and JSON reports.
#[derive(Debug, Clone)]
pub(crate) struct BenchRow {
    pub(crate) subject: String,
    pub(crate) lang: String,
    pub(crate) profile: String,
    pub(crate) workload: WorkloadKind,
    pub(crate) entities: Option<usize>,
    pub(crate) chunk_size: Option<usize>,
    pub(crate) stats: TimingStats,
    pub(crate) ns_per_entity: Option<f64>,
    pub(crate) m_entities_per_sec: Option<f64>,
    pub(crate) ratio_vs_native: f64,
    pub(crate) verified: bool,
}

/// Full benchmark run output across `native` baseline and all configured guests.
#[derive(Debug, Clone)]
pub(crate) struct BenchReport {
    pub(crate) quick: bool,
    pub(crate) warmup_samples: usize,
    pub(crate) timed_samples: usize,
    pub(crate) guest_manifests: Vec<(String, String, String)>,
    pub(crate) rows: Vec<BenchRow>,
}

/// Key uniquely identifying a `(workload, entities, chunk_size)` baseline measurement.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
struct BaselineKey {
    workload: WorkloadKind,
    entities: Option<usize>,
    chunk_size: Option<usize>,
}

// ============================================================================
// Native In-Process Rust Baseline Implementations
// ============================================================================

#[inline(never)]
fn native_noop() -> FfiResult {
    black_box(FfiResult::OK)
}

#[inline(always)]
fn integrate_dense_chunk(pos: &mut [Position], vel: &[Velocity], dt: f32) {
    assert_eq!(pos.len(), vel.len());
    for (p, v) in pos.iter_mut().zip(vel.iter()) {
        p.x += v.vx * dt;
        p.y += v.vy * dt;
        p.z += v.vz * dt;
        p.w += v.vw * dt;
    }
}

#[inline(never)]
pub(crate) fn native_dense_update(world: &HostWorld) -> FfiResult {
    let pos_handle = world.pos_handle;
    let vel_handle = world.vel_handle;
    let dt = BENCH_DT;
    for chunk in &world.chunks {
        chunk.mark_changed(pos_handle);
        // SAFETY: `setup_dense_entities` populates `pos_handle` (`Position`) and `vel_handle`
        // (`Velocity`) with `chunk.entities.len()` elements on every chunk, backing storage is
        // 8-byte word aligned, and native execution borrows these two distinct columns exclusively.
        let (pos, vel) = unsafe {
            (
                core::slice::from_raw_parts_mut(
                    chunk.dense_col_mut_ptr::<Position>(pos_handle),
                    chunk.entities.len(),
                ),
                chunk.dense_slice_unchecked::<Velocity>(vel_handle),
            )
        };
        integrate_dense_chunk(pos, vel, dt);
    }
    FfiResult::OK
}

#[inline(never)]
pub(crate) fn native_command_burst(world: &mut HostWorld) -> FfiResult {
    let pos_handle = world.pos_handle;
    let queue = world.commands_mut();
    for i in 0..64u32 {
        let ent = queue.push_native_spawn();
        let pos = Position {
            x: i as f32,
            y: (i as f32) * 2.0,
            z: 0.0,
            w: 1.0,
        };
        let status = queue.push_native_insert_pod(ent, pos_handle, &pos);
        if !status.is_ok() {
            return status;
        }
    }
    let cp = queue.checkpoint();
    let _rolled_back = queue.push_native_spawn();
    queue.rollback(cp)
}

#[inline(never)]
fn native_host_api_roundtrip(world: &HostWorld) -> FfiResult {
    let mut acc: u64 = 0;
    for _ in 0..64 {
        let Some((params, ticks)) = world.read_sim_params_with_ticks() else {
            return FfiResult::NOT_FOUND;
        };
        let sym = world.shared.intern_symbol_str("bench.sym");
        acc = acc
            .wrapping_add(u64::from(params.dt.to_bits()))
            .wrapping_add(u64::from(ticks.changed))
            .wrapping_add(u64::from(sym.to_raw()));
    }
    black_box(acc);
    FfiResult::OK
}

#[inline(always)]
fn read_bench_message_unaligned(raw: &[MaybeUninit<u8>]) -> BenchMessage {
    let mut msg = MaybeUninit::<BenchMessage>::uninit();
    // SAFETY: `msg` provides `16` writable bytes and `raw` has length `16` initialized bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(
            raw.as_ptr(),
            (&raw mut msg).cast::<MaybeUninit<u8>>(),
            core::mem::size_of::<BenchMessage>(),
        );
        msg.assume_init()
    }
}

#[inline(never)]
fn native_message_idle_poll(world: &HostWorld, cursors: &mut [usize; 8]) -> u64 {
    let mut sum = 0u64;
    let queue = world.bench_message_queue();
    for cursor in cursors.iter_mut() {
        if let Some(q) = queue {
            for raw in q.cursor(cursor) {
                if raw.len() == core::mem::size_of::<BenchMessage>() {
                    let msg = read_bench_message_unaligned(raw);
                    sum = sum.wrapping_add(msg.seq);
                }
            }
        }
    }
    black_box(sum)
}

#[inline(never)]
fn native_message_burst(world: &HostWorld, cursor: &mut usize) -> u64 {
    let mut sum = 0u64;
    if let Some(q) = world.bench_message_queue() {
        for raw in q.cursor(cursor) {
            if raw.len() == core::mem::size_of::<BenchMessage>() {
                let msg = read_bench_message_unaligned(raw);
                sum = sum.wrapping_add(msg.seq);
            }
        }
    }
    black_box(sum)
}

// ============================================================================
// Verification Helpers
// ============================================================================

/// Returns `(sum_x, sum_y, sum_z, sum_w)` across all `Position` components.
pub(crate) fn position_checksum(positions: &[Position]) -> [f64; 4] {
    let mut sums = [0.0f64; 4];
    for p in positions {
        sums[0] += f64::from(p.x);
        sums[1] += f64::from(p.y);
        sums[2] += f64::from(p.z);
        sums[3] += f64::from(p.w);
    }
    sums
}

#[inline]
fn approx_eq_rel(a: f64, b: f64, rel_tol: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() <= rel_tol * scale
}

/// Computes the expected native `Position` array for `(entity_count, chunk_size)` after `ticks`.
fn compute_native_reference_positions(
    entity_count: usize,
    chunk_size: usize,
    ticks: usize,
) -> Vec<Position> {
    let mut ref_world = HostWorld::new();
    ref_world.setup_dense_entities(entity_count, chunk_size);
    for _ in 0..ticks {
        let status = native_dense_update(&ref_world);
        assert!(status.is_ok());
    }
    ref_world.read_all_positions()
}

fn verify_dense_positions(
    subject: &str,
    actual: &[Position],
    expected: &[Position],
) -> Result<(), String> {
    if actual.len() != expected.len() {
        return Err(format!(
            "{subject}: dense_query_update entity count mismatch (got {}, expected {})",
            actual.len(),
            expected.len()
        ));
    }
    const TOL: f64 = 1e-3;
    for (idx, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        if !approx_eq_rel(f64::from(a.x), f64::from(e.x), TOL)
            || !approx_eq_rel(f64::from(a.y), f64::from(e.y), TOL)
            || !approx_eq_rel(f64::from(a.z), f64::from(e.z), TOL)
            || !approx_eq_rel(f64::from(a.w), f64::from(e.w), TOL)
        {
            return Err(format!(
                "{subject}: dense_query_update mismatch at entity {idx}: got {a:?}, expected {e:?}"
            ));
        }
    }
    let sum_actual = position_checksum(actual);
    let sum_expected = position_checksum(expected);
    for axis in 0..4 {
        if !approx_eq_rel(sum_actual[axis], sum_expected[axis], TOL) {
            return Err(format!(
                "{subject}: dense_query_update checksum mismatch on axis {axis}: got {}, expected {}",
                sum_actual[axis], sum_expected[axis]
            ));
        }
    }
    Ok(())
}

fn verify_command_burst_stats(subject: &str, world: &HostWorld) -> Result<(), String> {
    let stats = world.command_stats();
    if stats.committed_spawns != 64 {
        return Err(format!(
            "{subject}: command_burst committed_spawns = {}, expected 64",
            stats.committed_spawns
        ));
    }
    if stats.committed_inserts != 64 {
        return Err(format!(
            "{subject}: command_burst committed_inserts = {}, expected 64",
            stats.committed_inserts
        ));
    }
    if stats.checkpoints != 1 {
        return Err(format!(
            "{subject}: command_burst checkpoints = {}, expected 1",
            stats.checkpoints
        ));
    }
    if stats.rollbacks != 1 {
        return Err(format!(
            "{subject}: command_burst rollbacks = {}, expected 1",
            stats.rollbacks
        ));
    }
    if stats.rolled_back_ops == 0 {
        return Err(format!(
            "{subject}: command_burst rolled_back_ops = 0, expected >= 1"
        ));
    }
    if stats.last_insert_component != world.pos_handle
        || stats.last_insert_len != core::mem::size_of::<Position>()
    {
        return Err(format!(
            "{subject}: command_burst last insert component/len ({:?}, {}) != Position ({:?}, {})",
            stats.last_insert_component,
            stats.last_insert_len,
            world.pos_handle,
            core::mem::size_of::<Position>()
        ));
    }
    Ok(())
}

fn verify_host_api_roundtrip_calls(
    subject: &str,
    world: &HostWorld,
    require_res_ticks: bool,
) -> Result<(), String> {
    let intern_calls = world.intern_symbol_calls();
    if intern_calls != 64 {
        return Err(format!(
            "{subject}: host_api_roundtrip intern_symbol calls = {intern_calls}, expected 64"
        ));
    }
    if require_res_ticks {
        let ticks_calls = world.res_ticks_calls();
        if ticks_calls != 64 {
            return Err(format!(
                "{subject}: host_api_roundtrip res_ticks calls = {ticks_calls}, expected 64"
            ));
        }
    }
    Ok(())
}

// ============================================================================
// In-Process Guest & Raw C-ABI Message Benchmark Helpers
// ============================================================================

/// Owns an in-process `bamabi_guest` [`BamInstance`](bamabi_guest::raw::BamInstance)
/// bound to an [`OwnedHostWorld`], guaranteeing clean teardown on drop.
pub(crate) struct InProcessGuest {
    instance: *mut bamabi_guest::raw::BamInstance,
}

impl Drop for InProcessGuest {
    fn drop(&mut self) {
        if !self.instance.is_null() {
            // SAFETY: `self.instance` came from `init_guest_instance` and has not been shut down yet.
            unsafe {
                bamabi_guest::raw::shutdown_guest_instance(self.instance);
            }
            self.instance = core::ptr::null_mut();
        }
    }
}

impl InProcessGuest {
    /// Initializes an in-process `bamabi_guest` mod against `world`.
    pub(crate) fn load<F>(
        mod_name: &CStr,
        world: &mut OwnedHostWorld,
        build_fn: F,
    ) -> Result<Self, String>
    where
        F: FnOnce(&mut GuestApp<'_>) -> FfiResult,
    {
        let manifest = bamabi_guest::BamManifest::new(mod_name.as_ptr(), c"0.4.0".as_ptr());
        let mut instance: *mut bamabi_guest::raw::BamInstance = core::ptr::null_mut();
        // SAFETY: `world.api_ptr()` points to a valid 73-slot `BamHostApi`, `world.load_ctx_ptr()`
        // points to an exclusively held `HostWorld` during initialization, `instance` is writable
        // stack storage, and `manifest` references valid static C strings.
        let status = unsafe {
            bamabi_guest::raw::init_guest_instance(
                world.api_ptr(),
                world.load_ctx_ptr(),
                &mut instance,
                &manifest,
                |inst, load| bamabi_guest::build_guest_fn(inst, load, build_fn),
            )
        };
        if !status.is_ok() {
            return Err(format!(
                "in-process guest `{}` init failed: {status} ({})",
                mod_name.to_string_lossy(),
                world.last_error_string()
            ));
        }
        if world.has_fault() {
            return Err(format!(
                "in-process guest `{}` reported fault during init: {:?}",
                mod_name.to_string_lossy(),
                world.faults()
            ));
        }
        Ok(Self { instance })
    }

    /// Clears host system references and shuts down the in-process guest cleanly.
    pub(crate) fn shutdown(mut self, world: &mut OwnedHostWorld) -> Result<(), String> {
        world.clear_systems_for_unload();
        if !self.instance.is_null() {
            // SAFETY: `self.instance` came from `init_guest_instance` and host system
            // references were cleared via `world.clear_systems_for_unload()`.
            unsafe {
                bamabi_guest::raw::shutdown_guest_instance(self.instance);
            }
            self.instance = core::ptr::null_mut();
        }
        check_no_fault("in_process_guest", world)
    }
}

struct RawBatchState {
    api: *const bamabi_sys::BamHostApi,
    checksum: Arc<AtomicU64>,
}

struct RawBatchGuard {
    states: Vec<*mut RawBatchState>,
}

impl Drop for RawBatchGuard {
    fn drop(&mut self) {
        for ptr in self.states.drain(..) {
            if !ptr.is_null() {
                // SAFETY: Each pointer was allocated via `Box::into_raw(Box::new(RawBatchState { .. }))`
                // and is reclaimed once after host systems are cleared or on guard drop.
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
    }
}

impl RawBatchGuard {
    fn shutdown(mut self, world: &mut OwnedHostWorld) {
        world.clear_systems_for_unload();
        for ptr in self.states.drain(..) {
            if !ptr.is_null() {
                // SAFETY: Host systems were cleared via `clear_systems_for_unload()`; `ptr` came
                // from `Box::into_raw` and is uniquely owned by `self`.
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
    }
}

unsafe extern "C" fn raw_batch_system_cb(
    sys_ctx: *mut bamabi_sys::BamSystemCtx,
    user_data: *mut c_void,
) -> FfiResult {
    if sys_ctx.is_null() || user_data.is_null() {
        return FfiResult::NULL_POINTER;
    }
    // SAFETY: `user_data` points to a live `RawBatchState` owned by `RawBatchGuard`.
    let state = unsafe { &*user_data.cast::<RawBatchState>() };
    // SAFETY: `state.api` points to `HostWorld`'s validated `BamHostApi`.
    let api = unsafe { &*state.api };
    let (Some(sys_query), Some(msg_read_batch)) = (api.sys_query, api.msg_read_batch) else {
        return FfiResult::NULL_POINTER;
    };
    let mut iter = core::ptr::null_mut();
    // SAFETY: `sys_ctx` is valid for this callback invocation and has query 0 (`MESSAGE_READ`).
    let q_res = unsafe { sys_query(sys_ctx, 0, &mut iter) };
    if !q_res.is_ok() {
        return q_res;
    }
    let mut stack_buf = [BenchMessage::default(); 32];
    let mut sum = 0u64;
    loop {
        let mut read = 0usize;
        // SAFETY: `iter` is query 0, `stack_buf` provides `32 * 16 = 512` writable bytes,
        // and `read` is writable `usize` storage.
        let res = unsafe {
            msg_read_batch(
                iter,
                0,
                stack_buf.as_mut_ptr().cast::<u8>(),
                stack_buf.len(),
                &mut read,
            )
        };
        if !res.is_ok() {
            return res;
        }
        for msg in &stack_buf[..read] {
            sum = sum.wrapping_add(msg.seq);
        }
        if read < stack_buf.len() {
            break;
        }
    }
    state.checksum.fetch_add(sum, Ordering::Relaxed);
    FfiResult::OK
}

fn register_raw_batch_systems(
    world: &mut OwnedHostWorld,
    names: &[&CStr],
    checksum: &Arc<AtomicU64>,
) -> Result<(Vec<usize>, RawBatchGuard), String> {
    let Some(reg_sys) = world.api.register_system else {
        return Err("HostWorld missing register_system".to_owned());
    };
    let terms = [BamTermDesc::message_read(world.bench_msg_handle)];
    let queries = [BamQueryDesc::new(terms.as_ptr(), terms.len())];
    let api_ptr = world.api_ptr();
    let mut guard = RawBatchGuard {
        states: Vec::with_capacity(names.len()),
    };
    let mut indices = Vec::with_capacity(names.len());
    for &name in names {
        let raw_state = Box::into_raw(Box::new(RawBatchState {
            api: api_ptr,
            checksum: Arc::clone(checksum),
        }));
        guard.states.push(raw_state);
        let desc = BamSystemDesc::new(
            name.as_ptr(),
            queries.as_ptr(),
            queries.len(),
            raw_batch_system_cb,
            raw_state.cast::<c_void>(),
        );
        // SAFETY: `world.load_ctx_ptr()` and `desc` are valid during registration.
        let status = unsafe { reg_sys(world.load_ctx_ptr(), &desc) };
        if !status.is_ok() {
            return Err(format!(
                "c_abi:raw_batch register_system `{}` failed: {status}",
                name.to_string_lossy()
            ));
        }
        let name_str = name
            .to_str()
            .map_err(|_| "raw_batch system name is not valid UTF-8".to_owned())?;
        let idx = world
            .find_system(name_str)
            .ok_or_else(|| format!("registered raw_batch system `{name_str}` missing"))?;
        indices.push(idx);
    }
    Ok((indices, guard))
}

#[cfg(feature = "hotpath-alloc")]
fn assert_zero_alloc<F>(label: &str, f: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    let (count, bytes, res) = crate::alloc_probe::measure(f);
    res?;
    if bytes != 0 || count != 0 {
        return Err(format!(
            "{label}: expected 0 heap allocations on steady-state tick, got count={count}, bytes={bytes}"
        ));
    }
    Ok(())
}

macro_rules! hp_profile_ticks {
    ($label:literal, $samples:expr, $zero_alloc_check:expr, $ctx:expr, $tick:expr, $reset:expr) => {{
        #[cfg(feature = "hotpath")]
        {
            let mut __ctx = $ctx;
            let mut __tick = $tick;
            let mut __reset = $reset;
            let total = ($samples).max(1);
            for idx in 0..=total {
                let check_now = idx == total && $zero_alloc_check;
                hotpath::measure_block!($label, {
                    #[cfg(feature = "hotpath-alloc")]
                    if check_now {
                        assert_zero_alloc($label, || __tick(&mut __ctx))?;
                    } else {
                        __tick(&mut __ctx)?;
                    }
                    #[cfg(not(feature = "hotpath-alloc"))]
                    {
                        let _ = check_now;
                        __tick(&mut __ctx)?;
                    }
                });
                __reset(&mut __ctx);
            }
        }
        #[cfg(not(feature = "hotpath"))]
        {
            let _ = ($samples, $zero_alloc_check, $ctx, $tick, $reset);
        }
    }};
}

// ============================================================================
// Benchmark Execution Loop
// ============================================================================

fn batch_iters_for(workload: WorkloadKind, entities: usize, quick: bool) -> usize {
    match (workload, quick) {
        (WorkloadKind::Noop, true) | (WorkloadKind::MessageIdlePoll, false) => 2_000,
        (WorkloadKind::Noop, false) => 10_000,
        (WorkloadKind::DenseQueryUpdate, true) => {
            if entities <= 2_000 {
                15
            } else {
                2
            }
        }
        (WorkloadKind::DenseQueryUpdate, false) => {
            if entities <= 2_000 {
                50
            } else {
                5
            }
        }
        (WorkloadKind::CommandBurst | WorkloadKind::HostApiRoundtrip, true) => 40,
        (WorkloadKind::CommandBurst | WorkloadKind::HostApiRoundtrip, false) => 150,
        (WorkloadKind::MessageIdlePoll, true) => 500,
        (WorkloadKind::MessageBurst256 | WorkloadKind::MessageBurst4096, _) => 1,
    }
}

fn measure_samples<F>(
    warmup_samples: usize,
    timed_samples: usize,
    batch_iters: usize,
    mut f: F,
) -> Result<TimingStats, String>
where
    F: FnMut() -> Result<(), String>,
{
    for _ in 0..warmup_samples {
        for _ in 0..batch_iters {
            f()?;
        }
    }
    let mut samples = Vec::with_capacity(timed_samples);
    for _ in 0..timed_samples {
        let start = Instant::now();
        for _ in 0..batch_iters {
            f()?;
        }
        let elapsed_ns = start.elapsed().as_nanos() as f64;
        samples.push(elapsed_ns / (batch_iters as f64));
    }
    Ok(compute_stats(&samples))
}

fn measure_command_burst<W, F, R>(
    world: &mut W,
    warmup_samples: usize,
    timed_samples: usize,
    batch_iters: usize,
    mut run_once: F,
    mut reset_after_batch: R,
) -> Result<TimingStats, String>
where
    F: FnMut(&mut W) -> Result<(), String>,
    R: FnMut(&mut W),
{
    for _ in 0..warmup_samples {
        for _ in 0..batch_iters {
            run_once(world)?;
        }
        reset_after_batch(world);
    }
    let mut samples = Vec::with_capacity(timed_samples);
    for _ in 0..timed_samples {
        let start = Instant::now();
        for _ in 0..batch_iters {
            run_once(world)?;
        }
        let elapsed_ns = start.elapsed().as_nanos() as f64;
        reset_after_batch(world);
        samples.push(elapsed_ns / (batch_iters as f64));
    }
    Ok(compute_stats(&samples))
}

fn measure_message_burst<W, F, R>(
    world: &mut W,
    warmup_samples: usize,
    timed_samples: usize,
    mut run_once: F,
    mut reset_after_tick: R,
) -> Result<TimingStats, String>
where
    F: FnMut(&mut W) -> Result<(), String>,
    R: FnMut(&mut W),
{
    let warmups = warmup_samples.max(2);
    for _ in 0..warmups {
        run_once(world)?;
        reset_after_tick(world);
    }
    let mut samples = Vec::with_capacity(timed_samples);
    for _ in 0..timed_samples {
        let start = Instant::now();
        run_once(world)?;
        let elapsed_ns = start.elapsed().as_nanos() as f64;
        reset_after_tick(world);
        samples.push(elapsed_ns);
    }
    Ok(compute_stats(&samples))
}

fn run_system_slice(world: &OwnedHostWorld, indices: &[usize], label: &str) -> Result<(), String> {
    for &idx in indices {
        let s = world.run_system(idx);
        if !s.is_ok() {
            return Err(format!("{label}: system[{idx}] returned {s}"));
        }
    }
    Ok(())
}

fn push_message_row(
    rows: &mut Vec<BenchRow>,
    subject: &str,
    lang: &str,
    profile: &str,
    workload: WorkloadKind,
    stats: TimingStats,
    baseline_ns: f64,
) {
    rows.push(BenchRow {
        subject: subject.to_owned(),
        lang: lang.to_owned(),
        profile: profile.to_owned(),
        workload,
        entities: None,
        chunk_size: None,
        stats,
        ns_per_entity: None,
        m_entities_per_sec: None,
        ratio_vs_native: stats.median_ns / baseline_ns.max(1e-9),
        verified: true,
    });
}

const IDLE_SYSTEM_NAMES: [&CStr; 8] = [
    c"msg_idle_0",
    c"msg_idle_1",
    c"msg_idle_2",
    c"msg_idle_3",
    c"msg_idle_4",
    c"msg_idle_5",
    c"msg_idle_6",
    c"msg_idle_7",
];

fn run_message_idle_poll_subjects(
    config: &BenchConfig,
    baselines: &mut Vec<(BaselineKey, f64)>,
    rows: &mut Vec<BenchRow>,
) -> Result<(), String> {
    let workload = WorkloadKind::MessageIdlePoll;
    let batch = batch_iters_for(workload, 0, config.quick);

    // 1. native: 8 direct slice/cursor reads over HostWorld message queue.
    let native_base = {
        let mut world = HostWorld::new();
        world.clear_bench_messages();
        let mut cursors = [0usize; 8];
        if native_message_idle_poll(&world, &mut cursors) != 0 {
            return Err("native: message_idle_poll yielded non-zero checksum".to_owned());
        }
        let stats = measure_samples(
            config.warmup_samples.max(1),
            config.timed_samples,
            batch,
            || {
                if native_message_idle_poll(&world, &mut cursors) != 0 {
                    return Err("native: message_idle_poll yielded non-zero checksum".to_owned());
                }
                Ok(())
            },
        )?;
        let tick = |state: &mut (&HostWorld, &mut [usize; 8])| -> Result<(), String> {
            if native_message_idle_poll(state.0, state.1) != 0 {
                return Err("native: message_idle_poll yielded non-zero checksum".to_owned());
            }
            Ok(())
        };
        let reset = |_: &mut (&HostWorld, &mut [usize; 8])| {};
        let ctx = (&*world, &mut cursors);
        hp_profile_ticks!(
            "message_idle_poll::native",
            config.timed_samples,
            true,
            ctx,
            tick,
            reset
        );
        let base = stats.median_ns;
        baselines.push((
            BaselineKey {
                workload,
                entities: None,
                chunk_size: None,
            },
            base,
        ));
        push_message_row(rows, "native", "native", "release", workload, stats, base);
        base
    };

    // 2. rust_sdk:drain_messages (8 systems with on_message + read_messages).
    {
        let subject = "rust_sdk:drain_messages";
        let mut world = HostWorld::new();
        world.clear_bench_messages();
        let checksum = Arc::new(AtomicU64::new(0));
        let body_runs = Arc::new(AtomicU64::new(0));
        let guest = InProcessGuest::load(c"bench_msg_idle_drain", &mut world, |app| {
            for &name in &IDLE_SYSTEM_NAMES {
                let csum = Arc::clone(&checksum);
                let runs = Arc::clone(&body_runs);
                let rc = app
                    .system(name)
                    .schedule(ScheduleId::UPDATE)
                    .run_if_condition(conditions::on_message::<GuestBenchMsg>())
                    .query(|q| q.message_read::<GuestBenchMsg>())
                    .build(move |ctx| {
                        runs.fetch_add(1, Ordering::Relaxed);
                        let msgs = ctx.read_messages::<GuestBenchMsg>(0)?;
                        let mut sum = 0u64;
                        for m in &msgs {
                            sum = sum.wrapping_add(m.seq);
                        }
                        csum.fetch_add(sum, Ordering::Relaxed);
                        Ok(())
                    });
                if !rc.is_ok() {
                    return rc;
                }
            }
            FfiResult::OK
        })?;
        let mut sys_indices = [0usize; 8];
        for (slot, &name) in IDLE_SYSTEM_NAMES.iter().enumerate() {
            let name_str = name
                .to_str()
                .map_err(|_| "invalid system name".to_owned())?;
            sys_indices[slot] = world
                .find_system(name_str)
                .ok_or_else(|| format!("{subject}: missing system `{name_str}`"))?;
        }
        run_system_slice(&world, &sys_indices, subject)?;
        check_no_fault(subject, &world)?;
        if body_runs.load(Ordering::Relaxed) != 0 || checksum.load(Ordering::Relaxed) != 0 {
            return Err(format!("{subject}: idle poll body ran unexpectedly"));
        }
        let stats = measure_samples(
            config.warmup_samples.max(1),
            config.timed_samples,
            batch,
            || run_system_slice(&world, &sys_indices, subject),
        )?;
        let tick = |w: &mut &OwnedHostWorld| run_system_slice(w, &sys_indices, subject);
        let reset = |_: &mut &OwnedHostWorld| {};
        hp_profile_ticks!(
            "message_idle_poll::rust_sdk:drain_messages",
            config.timed_samples,
            true,
            &world,
            tick,
            reset
        );
        check_no_fault(subject, &world)?;
        if body_runs.load(Ordering::Relaxed) != 0 || checksum.load(Ordering::Relaxed) != 0 {
            return Err(format!("{subject}: idle poll body ran unexpectedly"));
        }
        guest.shutdown(&mut world)?;
        push_message_row(
            rows,
            subject,
            "rust_sdk",
            "drain_messages",
            workload,
            stats,
            native_base,
        );
    }

    // 3. rust_sdk:reused_vec (8 systems with on_message + local Vec + drain_messages_into_vec).
    {
        let subject = "rust_sdk:reused_vec";
        let mut world = HostWorld::new();
        world.clear_bench_messages();
        let checksum = Arc::new(AtomicU64::new(0));
        let body_runs = Arc::new(AtomicU64::new(0));
        let guest = InProcessGuest::load(c"bench_msg_idle_reused", &mut world, |app| {
            for &name in &IDLE_SYSTEM_NAMES {
                let csum = Arc::clone(&checksum);
                let runs = Arc::clone(&body_runs);
                let rc = app
                    .system(name)
                    .schedule(ScheduleId::UPDATE)
                    .run_if_condition(conditions::on_message::<GuestBenchMsg>())
                    .query(|q| q.message_read::<GuestBenchMsg>())
                    .build_with_local(Vec::<GuestBenchMsg>::with_capacity(64), move |ctx, buf| {
                        runs.fetch_add(1, Ordering::Relaxed);
                        buf.clear();
                        let _read = ctx.drain_messages_into_vec(0, buf, DrainOptions::default())?;
                        let mut sum = 0u64;
                        for m in buf.iter() {
                            sum = sum.wrapping_add(m.seq);
                        }
                        csum.fetch_add(sum, Ordering::Relaxed);
                        Ok(())
                    });
                if !rc.is_ok() {
                    return rc;
                }
            }
            FfiResult::OK
        })?;
        let mut sys_indices = [0usize; 8];
        for (slot, &name) in IDLE_SYSTEM_NAMES.iter().enumerate() {
            let name_str = name
                .to_str()
                .map_err(|_| "invalid system name".to_owned())?;
            sys_indices[slot] = world
                .find_system(name_str)
                .ok_or_else(|| format!("{subject}: missing system `{name_str}`"))?;
        }
        run_system_slice(&world, &sys_indices, subject)?;
        check_no_fault(subject, &world)?;
        if body_runs.load(Ordering::Relaxed) != 0 || checksum.load(Ordering::Relaxed) != 0 {
            return Err(format!("{subject}: idle poll body ran unexpectedly"));
        }
        let stats = measure_samples(
            config.warmup_samples.max(1),
            config.timed_samples,
            batch,
            || run_system_slice(&world, &sys_indices, subject),
        )?;
        let tick = |w: &mut &OwnedHostWorld| run_system_slice(w, &sys_indices, subject);
        let reset = |_: &mut &OwnedHostWorld| {};
        hp_profile_ticks!(
            "message_idle_poll::rust_sdk:reused_vec",
            config.timed_samples,
            true,
            &world,
            tick,
            reset
        );
        check_no_fault(subject, &world)?;
        if body_runs.load(Ordering::Relaxed) != 0 || checksum.load(Ordering::Relaxed) != 0 {
            return Err(format!("{subject}: idle poll body ran unexpectedly"));
        }
        guest.shutdown(&mut world)?;
        push_message_row(
            rows,
            subject,
            "rust_sdk",
            "reused_vec",
            workload,
            stats,
            native_base,
        );
    }

    // 4. c_abi:raw_batch (8 raw C-ABI systems reading into [BenchMessage; 32] stack buffer).
    {
        let subject = "c_abi:raw_batch";
        let mut world = HostWorld::new();
        world.clear_bench_messages();
        let checksum = Arc::new(AtomicU64::new(0));
        let (sys_indices, raw_guard) =
            register_raw_batch_systems(&mut world, &IDLE_SYSTEM_NAMES, &checksum)?;
        run_system_slice(&world, &sys_indices, subject)?;
        if checksum.load(Ordering::Relaxed) != 0 {
            return Err(format!("{subject}: idle poll returned non-zero checksum"));
        }
        let stats = measure_samples(
            config.warmup_samples.max(1),
            config.timed_samples,
            batch,
            || run_system_slice(&world, &sys_indices, subject),
        )?;
        let tick = |w: &mut &OwnedHostWorld| run_system_slice(w, &sys_indices, subject);
        let reset = |_: &mut &OwnedHostWorld| {};
        hp_profile_ticks!(
            "message_idle_poll::c_abi:raw_batch",
            config.timed_samples,
            true,
            &world,
            tick,
            reset
        );
        if checksum.load(Ordering::Relaxed) != 0 {
            return Err(format!("{subject}: idle poll returned non-zero checksum"));
        }
        raw_guard.shutdown(&mut world);
        push_message_row(
            rows,
            subject,
            "c_abi",
            "raw_batch",
            workload,
            stats,
            native_base,
        );
    }

    Ok(())
}

fn run_message_burst_subjects(
    config: &BenchConfig,
    workload: WorkloadKind,
    burst_len: usize,
    expected_sum: u64,
    baselines: &mut Vec<(BaselineKey, f64)>,
    rows: &mut Vec<BenchRow>,
) -> Result<(), String> {
    // 1. native
    let native_base = {
        let mut world = HostWorld::new();
        world.enqueue_bench_messages(burst_len);
        let mut cursor = 0usize;
        let check_sum = native_message_burst(&world, &mut cursor);
        if check_sum != expected_sum || cursor != burst_len {
            return Err(format!(
                "native: {} checksum {check_sum} (cursor {cursor}) != expected {expected_sum} ({burst_len})",
                workload.as_str()
            ));
        }
        cursor = 0;

        let mut ctx = (&*world, &mut cursor);
        let stats = measure_message_burst(
            &mut ctx,
            config.warmup_samples,
            config.timed_samples,
            |(w, c)| {
                let sum = native_message_burst(w, c);
                if sum != expected_sum {
                    return Err(format!(
                        "native: {} checksum {sum} != expected {expected_sum}",
                        workload.as_str()
                    ));
                }
                Ok(())
            },
            |(_, c)| **c = 0,
        )?;
        let tick = |state: &mut (&HostWorld, &mut usize)| -> Result<(), String> {
            let sum = native_message_burst(state.0, state.1);
            if sum != expected_sum {
                return Err(format!(
                    "native: {} checksum {sum} != expected {expected_sum}",
                    workload.as_str()
                ));
            }
            Ok(())
        };
        let reset = |state: &mut (&HostWorld, &mut usize)| *state.1 = 0;
        if burst_len == 256 {
            hp_profile_ticks!(
                "message_burst_256::native",
                config.timed_samples,
                true,
                ctx,
                tick,
                reset
            );
        } else {
            hp_profile_ticks!(
                "message_burst_4096::native",
                config.timed_samples,
                true,
                ctx,
                tick,
                reset
            );
        }
        let base = stats.median_ns;
        baselines.push((
            BaselineKey {
                workload,
                entities: None,
                chunk_size: None,
            },
            base,
        ));
        push_message_row(rows, "native", "native", "release", workload, stats, base);
        base
    };

    // 2. rust_sdk:drain_messages (DrainOptions::default())
    {
        let subject = "rust_sdk:drain_messages";
        let mut world = HostWorld::new();
        world.enqueue_bench_messages(burst_len);
        let checksum = Arc::new(AtomicU64::new(0));
        let guest = InProcessGuest::load(c"bench_msg_burst_drain", &mut world, |app| {
            let csum = Arc::clone(&checksum);
            app.system(c"msg_burst_drain")
                .schedule(ScheduleId::UPDATE)
                .query(|q| q.message_read::<GuestBenchMsg>())
                .build(move |ctx| {
                    let msgs = ctx
                        .drain_messages_with_options::<GuestBenchMsg>(0, DrainOptions::default())?;
                    let mut sum = 0u64;
                    for m in &msgs {
                        sum = sum.wrapping_add(m.seq);
                    }
                    csum.store(sum, Ordering::Relaxed);
                    Ok(())
                })
        })?;
        let sys_idx = world
            .find_system("msg_burst_drain")
            .ok_or_else(|| format!("{subject}: missing system `msg_burst_drain`"))?;
        checksum.store(0, Ordering::Relaxed);
        let status = world.run_system(sys_idx);
        if !status.is_ok() {
            return Err(format!("{subject}: msg_burst_drain returned {status}"));
        }
        check_no_fault(subject, &world)?;
        let got = checksum.load(Ordering::Relaxed);
        if got != expected_sum {
            return Err(format!(
                "{subject}: {} checksum {got} != expected {expected_sum}",
                workload.as_str()
            ));
        }
        world.rewind_msg_cursors();

        let stats = measure_message_burst(
            &mut world,
            config.warmup_samples,
            config.timed_samples,
            |w| {
                let s = w.run_system(sys_idx);
                if !s.is_ok() {
                    return Err(format!("{subject}: msg_burst_drain returned {s}"));
                }
                Ok(())
            },
            |w| w.rewind_msg_cursors(),
        )?;
        let tick = |w: &mut &OwnedHostWorld| -> Result<(), String> {
            let s = w.run_system(sys_idx);
            if !s.is_ok() {
                return Err(format!("{subject}: msg_burst_drain returned {s}"));
            }
            Ok(())
        };
        let reset = |w: &mut &OwnedHostWorld| w.rewind_msg_cursors();
        if burst_len == 256 {
            hp_profile_ticks!(
                "message_burst_256::rust_sdk:drain_messages",
                config.timed_samples,
                false,
                &world,
                tick,
                reset
            );
        } else {
            hp_profile_ticks!(
                "message_burst_4096::rust_sdk:drain_messages",
                config.timed_samples,
                false,
                &world,
                tick,
                reset
            );
        }
        check_no_fault(subject, &world)?;
        guest.shutdown(&mut world)?;
        push_message_row(
            rows,
            subject,
            "rust_sdk",
            "drain_messages",
            workload,
            stats,
            native_base,
        );
    }

    // 3. rust_sdk:reused_vec (pre-allocated local Vec + drain_messages_into_vec)
    {
        let subject = "rust_sdk:reused_vec";
        let drain_opts = if burst_len == 256 {
            DrainOptions::new(256, 1024)
        } else {
            DrainOptions::new(256, 4096)
        };
        let mut world = HostWorld::new();
        world.enqueue_bench_messages(burst_len);
        let checksum = Arc::new(AtomicU64::new(0));
        let guest = InProcessGuest::load(c"bench_msg_burst_reused", &mut world, |app| {
            let csum = Arc::clone(&checksum);
            app.system(c"msg_burst_reused")
                .schedule(ScheduleId::UPDATE)
                .query(|q| q.message_read::<GuestBenchMsg>())
                .build_with_local(
                    Vec::<GuestBenchMsg>::with_capacity(burst_len),
                    move |ctx, buf| {
                        buf.clear();
                        let _read = ctx.drain_messages_into_vec(0, buf, drain_opts)?;
                        let mut sum = 0u64;
                        for m in buf.iter() {
                            sum = sum.wrapping_add(m.seq);
                        }
                        csum.store(sum, Ordering::Relaxed);
                        Ok(())
                    },
                )
        })?;
        let sys_idx = world
            .find_system("msg_burst_reused")
            .ok_or_else(|| format!("{subject}: missing system `msg_burst_reused`"))?;

        world.reset_msg_read_batch_calls();
        checksum.store(0, Ordering::Relaxed);
        let status = world.run_system(sys_idx);
        if !status.is_ok() {
            return Err(format!("{subject}: msg_burst_reused returned {status}"));
        }
        check_no_fault(subject, &world)?;
        let got = checksum.load(Ordering::Relaxed);
        if got != expected_sum {
            return Err(format!(
                "{subject}: {} checksum {got} != expected {expected_sum}",
                workload.as_str()
            ));
        }
        if burst_len == 256 {
            let calls = world.msg_read_batch_calls();
            if calls > 2 {
                return Err(format!(
                    "{subject}: message_burst_256 msg_read_batch_calls = {calls}, expected <= 2"
                ));
            }
        }
        world.rewind_msg_cursors();

        let stats = measure_message_burst(
            &mut world,
            config.warmup_samples,
            config.timed_samples,
            |w| {
                let s = w.run_system(sys_idx);
                if !s.is_ok() {
                    return Err(format!("{subject}: msg_burst_reused returned {s}"));
                }
                Ok(())
            },
            |w| w.rewind_msg_cursors(),
        )?;
        let tick = |w: &mut &OwnedHostWorld| -> Result<(), String> {
            checksum.store(0, Ordering::Relaxed);
            let s = w.run_system(sys_idx);
            if !s.is_ok() {
                return Err(format!("{subject}: msg_burst_reused returned {s}"));
            }
            let sum = checksum.load(Ordering::Relaxed);
            if sum != expected_sum {
                return Err(format!(
                    "{subject}: {} checksum {sum} != expected {expected_sum}",
                    workload.as_str()
                ));
            }
            Ok(())
        };
        let reset = |w: &mut &OwnedHostWorld| w.rewind_msg_cursors();
        if burst_len == 256 {
            hp_profile_ticks!(
                "message_burst_256::rust_sdk:reused_vec",
                config.timed_samples,
                true,
                &world,
                tick,
                reset
            );
        } else {
            hp_profile_ticks!(
                "message_burst_4096::rust_sdk:reused_vec",
                config.timed_samples,
                true,
                &world,
                tick,
                reset
            );
        }
        check_no_fault(subject, &world)?;
        guest.shutdown(&mut world)?;
        push_message_row(
            rows,
            subject,
            "rust_sdk",
            "reused_vec",
            workload,
            stats,
            native_base,
        );
    }

    // 4. c_abi:raw_batch ([BenchMessage; 32] stack loop)
    {
        let subject = "c_abi:raw_batch";
        let mut world = HostWorld::new();
        world.enqueue_bench_messages(burst_len);
        let checksum = Arc::new(AtomicU64::new(0));
        let (sys_indices, raw_guard) =
            register_raw_batch_systems(&mut world, &[c"msg_burst_raw"], &checksum)?;
        let sys_idx = sys_indices[0];

        checksum.store(0, Ordering::Relaxed);
        let status = world.run_system(sys_idx);
        if !status.is_ok() {
            return Err(format!("{subject}: msg_burst_raw returned {status}"));
        }
        let got = checksum.load(Ordering::Relaxed);
        if got != expected_sum {
            return Err(format!(
                "{subject}: {} checksum {got} != expected {expected_sum}",
                workload.as_str()
            ));
        }
        world.rewind_msg_cursors();

        let stats = measure_message_burst(
            &mut world,
            config.warmup_samples,
            config.timed_samples,
            |w| {
                let s = w.run_system(sys_idx);
                if !s.is_ok() {
                    return Err(format!("{subject}: msg_burst_raw returned {s}"));
                }
                Ok(())
            },
            |w| w.rewind_msg_cursors(),
        )?;
        let tick = |w: &mut &OwnedHostWorld| -> Result<(), String> {
            checksum.store(0, Ordering::Relaxed);
            let s = w.run_system(sys_idx);
            if !s.is_ok() {
                return Err(format!("{subject}: msg_burst_raw returned {s}"));
            }
            let sum = checksum.load(Ordering::Relaxed);
            if sum != expected_sum {
                return Err(format!(
                    "{subject}: {} checksum {sum} != expected {expected_sum}",
                    workload.as_str()
                ));
            }
            Ok(())
        };
        let reset = |w: &mut &OwnedHostWorld| w.rewind_msg_cursors();
        if burst_len == 256 {
            hp_profile_ticks!(
                "message_burst_256::c_abi:raw_batch",
                config.timed_samples,
                true,
                &world,
                tick,
                reset
            );
        } else {
            hp_profile_ticks!(
                "message_burst_4096::c_abi:raw_batch",
                config.timed_samples,
                true,
                &world,
                tick,
                reset
            );
        }
        raw_guard.shutdown(&mut world);
        push_message_row(
            rows,
            subject,
            "c_abi",
            "raw_batch",
            workload,
            stats,
            native_base,
        );
    }

    Ok(())
}

/// Runs the native Rust baseline and every configured guest library, verifying correctness before
/// recording timings.
pub(crate) fn run_benchmarks(config: &BenchConfig) -> Result<BenchReport, String> {
    let mut rows = Vec::new();
    let mut baselines: Vec<(BaselineKey, f64)> = Vec::new();
    let mut dense_refs: Vec<((usize, usize), Vec<Position>)> = Vec::new();

    // Pre-compute native reference positions for every (entities, chunk_size) pair.
    if config.workloads.contains(&WorkloadKind::DenseQueryUpdate) {
        for &entity_count in &config.entities {
            for &chunk_size in &config.chunks {
                let expected = compute_native_reference_positions(entity_count, chunk_size, 3);
                dense_refs.push(((entity_count, chunk_size), expected));
            }
        }
    }

    // 1. Run and verify native Rust baseline.
    {
        let mut world = HostWorld::new();
        for &workload in &config.workloads {
            match workload {
                WorkloadKind::Noop => {
                    let status = native_noop();
                    if !status.is_ok() {
                        return Err(format!("native noop failed: {status}"));
                    }
                    let batch = batch_iters_for(workload, 0, config.quick);
                    let stats = measure_samples(
                        config.warmup_samples,
                        config.timed_samples,
                        batch,
                        || {
                            let s = native_noop();
                            if s.is_ok() {
                                Ok(())
                            } else {
                                Err(format!("native noop failed: {s}"))
                            }
                        },
                    )?;
                    baselines.push((
                        BaselineKey {
                            workload,
                            entities: None,
                            chunk_size: None,
                        },
                        stats.median_ns,
                    ));
                    rows.push(BenchRow {
                        subject: "native".to_owned(),
                        lang: "native".to_owned(),
                        profile: "release".to_owned(),
                        workload,
                        entities: None,
                        chunk_size: None,
                        stats,
                        ns_per_entity: None,
                        m_entities_per_sec: None,
                        ratio_vs_native: 1.0,
                        verified: true,
                    });
                }
                WorkloadKind::DenseQueryUpdate => {
                    for &entity_count in &config.entities {
                        for &chunk_size in &config.chunks {
                            world.setup_dense_entities(entity_count, chunk_size);
                            for _ in 0..3 {
                                let s = native_dense_update(&world);
                                if !s.is_ok() {
                                    return Err(format!("native dense_update failed: {s}"));
                                }
                            }
                            let expected = dense_refs
                                .iter()
                                .find(|(k, _)| *k == (entity_count, chunk_size))
                                .map(|(_, v)| v.as_slice())
                                .expect("precomputed dense reference");
                            verify_dense_positions(
                                "native",
                                &world.read_all_positions(),
                                expected,
                            )?;
                            world.reset_dense_positions();

                            let batch = batch_iters_for(workload, entity_count, config.quick);
                            let stats = measure_samples(
                                config.warmup_samples,
                                config.timed_samples,
                                batch,
                                || {
                                    let s = native_dense_update(&world);
                                    if s.is_ok() {
                                        Ok(())
                                    } else {
                                        Err(format!("native dense_update failed: {s}"))
                                    }
                                },
                            )?;
                            let ns_ent = stats.median_ns / (entity_count.max(1) as f64);
                            let m_ent_s = 1000.0 / ns_ent;
                            baselines.push((
                                BaselineKey {
                                    workload,
                                    entities: Some(entity_count),
                                    chunk_size: Some(chunk_size),
                                },
                                stats.median_ns,
                            ));
                            rows.push(BenchRow {
                                subject: "native".to_owned(),
                                lang: "native".to_owned(),
                                profile: "release".to_owned(),
                                workload,
                                entities: Some(entity_count),
                                chunk_size: Some(chunk_size),
                                stats,
                                ns_per_entity: Some(ns_ent),
                                m_entities_per_sec: Some(m_ent_s),
                                ratio_vs_native: 1.0,
                                verified: true,
                            });
                        }
                    }
                }
                WorkloadKind::CommandBurst => {
                    world.reset_commands();
                    let s = native_command_burst(&mut world);
                    if !s.is_ok() {
                        return Err(format!("native command_burst failed: {s}"));
                    }
                    verify_command_burst_stats("native", &world)?;
                    world.reset_commands();

                    let batch = batch_iters_for(workload, 0, config.quick);
                    world.reserve_command_capacity(batch * 160);
                    let stats = measure_command_burst(
                        &mut world,
                        config.warmup_samples,
                        config.timed_samples,
                        batch,
                        |w| {
                            let s = native_command_burst(w);
                            if s.is_ok() {
                                Ok(())
                            } else {
                                Err(format!("native command_burst failed: {s}"))
                            }
                        },
                        |w| w.reset_commands(),
                    )?;
                    baselines.push((
                        BaselineKey {
                            workload,
                            entities: None,
                            chunk_size: None,
                        },
                        stats.median_ns,
                    ));
                    rows.push(BenchRow {
                        subject: "native".to_owned(),
                        lang: "native".to_owned(),
                        profile: "release".to_owned(),
                        workload,
                        entities: None,
                        chunk_size: None,
                        stats,
                        ns_per_entity: None,
                        m_entities_per_sec: None,
                        ratio_vs_native: 1.0,
                        verified: true,
                    });
                }
                WorkloadKind::HostApiRoundtrip => {
                    world.reset_intern_symbol_calls();
                    let s = native_host_api_roundtrip(&world);
                    if !s.is_ok() {
                        return Err(format!("native host_api_roundtrip failed: {s}"));
                    }
                    verify_host_api_roundtrip_calls("native", &world, false)?;

                    let batch = batch_iters_for(workload, 0, config.quick);
                    let stats = measure_samples(
                        config.warmup_samples,
                        config.timed_samples,
                        batch,
                        || {
                            let s = native_host_api_roundtrip(&world);
                            if s.is_ok() {
                                Ok(())
                            } else {
                                Err(format!("native host_api_roundtrip failed: {s}"))
                            }
                        },
                    )?;
                    baselines.push((
                        BaselineKey {
                            workload,
                            entities: None,
                            chunk_size: None,
                        },
                        stats.median_ns,
                    ));
                    rows.push(BenchRow {
                        subject: "native".to_owned(),
                        lang: "native".to_owned(),
                        profile: "release".to_owned(),
                        workload,
                        entities: None,
                        chunk_size: None,
                        stats,
                        ns_per_entity: None,
                        m_entities_per_sec: None,
                        ratio_vs_native: 1.0,
                        verified: true,
                    });
                }
                WorkloadKind::MessageIdlePoll => {
                    run_message_idle_poll_subjects(config, &mut baselines, &mut rows)?;
                }
                WorkloadKind::MessageBurst256 => {
                    run_message_burst_subjects(
                        config,
                        WorkloadKind::MessageBurst256,
                        256,
                        32_640,
                        &mut baselines,
                        &mut rows,
                    )?;
                }
                WorkloadKind::MessageBurst4096 => {
                    run_message_burst_subjects(
                        config,
                        WorkloadKind::MessageBurst4096,
                        4_096,
                        8_386_560,
                        &mut baselines,
                        &mut rows,
                    )?;
                }
            }
        }
    }

    // 2. Load, verify, benchmark, and cleanly shut down each polyglot guest.
    let mut guest_manifests = Vec::with_capacity(config.guests.len());
    for spec in &config.guests {
        let mut world = HostWorld::new();
        let guest = LoadedGuest::load_and_init(&spec.path, &mut world)?;
        let subject = spec.label();
        guest_manifests.push((
            subject.clone(),
            guest.manifest.name.clone(),
            guest.manifest.version.clone(),
        ));

        let systems = guest.systems;
        for &workload in &config.workloads {
            match workload {
                WorkloadKind::Noop => {
                    let status = world.run_system(systems.noop);
                    if !status.is_ok() {
                        return Err(format!("{subject}: bench_noop returned {status}"));
                    }
                    check_no_fault(&subject, &world)?;
                    let batch = batch_iters_for(workload, 0, config.quick);
                    let stats = measure_samples(
                        config.warmup_samples,
                        config.timed_samples,
                        batch,
                        || {
                            let s = world.run_system(systems.noop);
                            if s.is_ok() {
                                Ok(())
                            } else {
                                Err(format!("{subject}: bench_noop returned {s}"))
                            }
                        },
                    )?;
                    check_no_fault(&subject, &world)?;
                    let base = lookup_baseline(&baselines, workload, None, None);
                    rows.push(BenchRow {
                        subject: subject.clone(),
                        lang: spec.lang.clone(),
                        profile: spec.profile.clone(),
                        workload,
                        entities: None,
                        chunk_size: None,
                        stats,
                        ns_per_entity: None,
                        m_entities_per_sec: None,
                        ratio_vs_native: stats.median_ns / base,
                        verified: true,
                    });
                }
                WorkloadKind::DenseQueryUpdate => {
                    for &entity_count in &config.entities {
                        for &chunk_size in &config.chunks {
                            world.setup_dense_entities(entity_count, chunk_size);
                            for _ in 0..3 {
                                let s = world.run_system(systems.dense_update);
                                if !s.is_ok() {
                                    return Err(format!(
                                        "{subject}: bench_dense_update returned {s}"
                                    ));
                                }
                            }
                            check_no_fault(&subject, &world)?;
                            let expected = dense_refs
                                .iter()
                                .find(|(k, _)| *k == (entity_count, chunk_size))
                                .map(|(_, v)| v.as_slice())
                                .expect("precomputed dense reference");
                            verify_dense_positions(
                                &subject,
                                &world.read_all_positions(),
                                expected,
                            )?;
                            world.reset_dense_positions();

                            let batch = batch_iters_for(workload, entity_count, config.quick);
                            let stats = measure_samples(
                                config.warmup_samples,
                                config.timed_samples,
                                batch,
                                || {
                                    let s = world.run_system(systems.dense_update);
                                    if s.is_ok() {
                                        Ok(())
                                    } else {
                                        Err(format!("{subject}: bench_dense_update returned {s}"))
                                    }
                                },
                            )?;
                            check_no_fault(&subject, &world)?;
                            let ns_ent = stats.median_ns / (entity_count.max(1) as f64);
                            let m_ent_s = 1000.0 / ns_ent;
                            let base = lookup_baseline(
                                &baselines,
                                workload,
                                Some(entity_count),
                                Some(chunk_size),
                            );
                            rows.push(BenchRow {
                                subject: subject.clone(),
                                lang: spec.lang.clone(),
                                profile: spec.profile.clone(),
                                workload,
                                entities: Some(entity_count),
                                chunk_size: Some(chunk_size),
                                stats,
                                ns_per_entity: Some(ns_ent),
                                m_entities_per_sec: Some(m_ent_s),
                                ratio_vs_native: stats.median_ns / base,
                                verified: true,
                            });
                        }
                    }
                }
                WorkloadKind::CommandBurst => {
                    world.reset_commands();
                    let s = world.run_system(systems.command_burst);
                    if !s.is_ok() {
                        return Err(format!("{subject}: bench_command_burst returned {s}"));
                    }
                    check_no_fault(&subject, &world)?;
                    verify_command_burst_stats(&subject, &world)?;
                    world.reset_commands();

                    let batch = batch_iters_for(workload, 0, config.quick);
                    world.reserve_command_capacity(batch * 160);
                    let stats = measure_command_burst(
                        &mut world,
                        config.warmup_samples,
                        config.timed_samples,
                        batch,
                        |w| {
                            let s = w.run_system(systems.command_burst);
                            if s.is_ok() {
                                Ok(())
                            } else {
                                Err(format!("{subject}: bench_command_burst returned {s}"))
                            }
                        },
                        |w| w.reset_commands(),
                    )?;
                    check_no_fault(&subject, &world)?;
                    let base = lookup_baseline(&baselines, workload, None, None);
                    rows.push(BenchRow {
                        subject: subject.clone(),
                        lang: spec.lang.clone(),
                        profile: spec.profile.clone(),
                        workload,
                        entities: None,
                        chunk_size: None,
                        stats,
                        ns_per_entity: None,
                        m_entities_per_sec: None,
                        ratio_vs_native: stats.median_ns / base,
                        verified: true,
                    });
                }
                WorkloadKind::HostApiRoundtrip => {
                    world.reset_intern_symbol_calls();
                    world.reset_res_ticks_calls();
                    let s = world.run_system(systems.host_api_roundtrip);
                    if !s.is_ok() {
                        return Err(format!("{subject}: bench_host_api_roundtrip returned {s}"));
                    }
                    check_no_fault(&subject, &world)?;
                    verify_host_api_roundtrip_calls(&subject, &world, true)?;

                    let batch = batch_iters_for(workload, 0, config.quick);
                    let stats = measure_samples(
                        config.warmup_samples,
                        config.timed_samples,
                        batch,
                        || {
                            let s = world.run_system(systems.host_api_roundtrip);
                            if s.is_ok() {
                                Ok(())
                            } else {
                                Err(format!("{subject}: bench_host_api_roundtrip returned {s}"))
                            }
                        },
                    )?;
                    check_no_fault(&subject, &world)?;
                    let base = lookup_baseline(&baselines, workload, None, None);
                    rows.push(BenchRow {
                        subject: subject.clone(),
                        lang: spec.lang.clone(),
                        profile: spec.profile.clone(),
                        workload,
                        entities: None,
                        chunk_size: None,
                        stats,
                        ns_per_entity: None,
                        m_entities_per_sec: None,
                        ratio_vs_native: stats.median_ns / base,
                        verified: true,
                    });
                }
                WorkloadKind::MessageIdlePoll => {
                    if let Some(sys_idx) = systems.message_idle_poll {
                        world.clear_bench_messages();
                        let s = world.run_system(sys_idx);
                        if !s.is_ok() {
                            return Err(format!("{subject}: bench_message_idle_poll returned {s}"));
                        }
                        check_no_fault(&subject, &world)?;
                        let batch = batch_iters_for(workload, 0, config.quick);
                        let stats = measure_samples(
                            config.warmup_samples,
                            config.timed_samples,
                            batch,
                            || {
                                let status = world.run_system(sys_idx);
                                if status.is_ok() {
                                    Ok(())
                                } else {
                                    Err(format!(
                                        "{subject}: bench_message_idle_poll returned {status}"
                                    ))
                                }
                            },
                        )?;
                        check_no_fault(&subject, &world)?;
                        let base = lookup_baseline(&baselines, workload, None, None);
                        push_message_row(
                            &mut rows,
                            &subject,
                            &spec.lang,
                            &spec.profile,
                            workload,
                            stats,
                            base,
                        );
                    }
                }
                WorkloadKind::MessageBurst256 | WorkloadKind::MessageBurst4096 => {
                    let (burst_len, sys_opt, sys_label) = match workload {
                        WorkloadKind::MessageBurst256 => {
                            (256, systems.message_burst_256, "bench_message_burst_256")
                        }
                        WorkloadKind::MessageBurst4096 => (
                            4_096,
                            systems.message_burst_4096,
                            "bench_message_burst_4096",
                        ),
                        _ => unreachable!(),
                    };
                    if let Some(sys_idx) = sys_opt {
                        world.enqueue_bench_messages(burst_len);
                        let s = world.run_system(sys_idx);
                        if !s.is_ok() {
                            return Err(format!("{subject}: {sys_label} returned {s}"));
                        }
                        check_no_fault(&subject, &world)?;
                        world.rewind_msg_cursors();
                        let stats = measure_message_burst(
                            &mut world,
                            config.warmup_samples,
                            config.timed_samples,
                            |w| {
                                let status = w.run_system(sys_idx);
                                if status.is_ok() {
                                    Ok(())
                                } else {
                                    Err(format!("{subject}: {sys_label} returned {status}"))
                                }
                            },
                            |w| w.rewind_msg_cursors(),
                        )?;
                        check_no_fault(&subject, &world)?;
                        let base = lookup_baseline(&baselines, workload, None, None);
                        push_message_row(
                            &mut rows,
                            &subject,
                            &spec.lang,
                            &spec.profile,
                            workload,
                            stats,
                            base,
                        );
                    }
                }
            }
        }

        guest.shutdown(&mut world)?;
    }

    Ok(BenchReport {
        quick: config.quick,
        warmup_samples: config.warmup_samples,
        timed_samples: config.timed_samples,
        guest_manifests,
        rows,
    })
}

fn check_no_fault(subject: &str, world: &HostWorld) -> Result<(), String> {
    if world.has_fault() {
        Err(format!(
            "{subject}: guest reported fault(s): {:?}",
            world.faults()
        ))
    } else {
        Ok(())
    }
}

fn lookup_baseline(
    baselines: &[(BaselineKey, f64)],
    workload: WorkloadKind,
    entities: Option<usize>,
    chunk_size: Option<usize>,
) -> f64 {
    let key = BaselineKey {
        workload,
        entities,
        chunk_size,
    };
    baselines
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(1.0, |(_, v)| (*v).max(1e-9))
}
