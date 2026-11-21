//! Benchmark workloads, native Rust baseline, strict correctness checks, and statistics.

use core::hint::black_box;
use std::{path::PathBuf, time::Instant};

use bamabi_sys::FfiResult;

use crate::{
    host::{BENCH_DT, HostWorld, Position, Velocity},
    loader::LoadedGuest,
};

/// Supported benchmark workload identifiers.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) enum WorkloadKind {
    Noop,
    DenseQueryUpdate,
    CommandBurst,
    HostApiRoundtrip,
}

impl WorkloadKind {
    /// All four benchmark workloads in canonical execution order.
    pub(crate) const ALL: [Self; 4] = [
        Self::Noop,
        Self::DenseQueryUpdate,
        Self::CommandBurst,
        Self::HostApiRoundtrip,
    ];

    /// Parses a CLI workload identifier.
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            "noop" => Ok(Self::Noop),
            "dense_query_update" => Ok(Self::DenseQueryUpdate),
            "command_burst" => Ok(Self::CommandBurst),
            "host_api_roundtrip" => Ok(Self::HostApiRoundtrip),
            other => Err(format!(
                "unknown workload `{other}` (expected one of: noop, dense_query_update, command_burst, host_api_roundtrip)"
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

#[inline(never)]
pub(crate) fn native_dense_update(world: &HostWorld) -> FfiResult {
    let pos_handle = world.pos_handle;
    let vel_handle = world.vel_handle;
    let dt = BENCH_DT;
    for chunk in &world.chunks {
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
        assert_eq!(pos.len(), vel.len());
        chunk.mark_changed(pos_handle);
        for (p, v) in pos.iter_mut().zip(vel.iter()) {
            p.x += v.vx * dt;
            p.y += v.vy * dt;
            p.z += v.vz * dt;
            p.w += v.vw * dt;
        }
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
// Benchmark Execution Loop
// ============================================================================

fn batch_iters_for(workload: WorkloadKind, entities: usize, quick: bool) -> usize {
    match (workload, quick) {
        (WorkloadKind::Noop, true) => 2_000,
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
