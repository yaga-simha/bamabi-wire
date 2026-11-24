#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Rust `bamabi_guest` safe-SDK benchmark guest mod (`ABI 0.4.0`).
//!
//! Exports the four shared wire-contract benchmark systems under `ScheduleId::UPDATE`:
//! - `bench_noop`
//! - `bench_dense_update`
//! - `bench_command_burst`
//! - `bench_host_api_roundtrip`

use core::ffi::CStr;

use bamabi_guest::prelude::*;

/// Canonical `bench::Position` POD component (16 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, AbiPod, GuestComponent)]
#[guest_component(name = "bench::Position", no_schema_hash)]
struct Position {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}

/// Canonical `bench::Velocity` POD component (16 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, AbiPod, GuestComponent)]
#[guest_component(name = "bench::Velocity", no_schema_hash)]
struct Velocity {
    vx: f32,
    vy: f32,
    vz: f32,
    vw: f32,
}

/// Canonical `bench::SimParams` POD resource (16 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, AbiPod, GuestComponent)]
#[guest_component(name = "bench::SimParams", no_schema_hash)]
struct SimParams {
    dt: f32,
    damping: f32,
    tick: u32,
    _pad: u32,
}

const _: () = {
    assert!(core::mem::size_of::<Position>() == 16);
    assert!(core::mem::align_of::<Position>() == 4);
    assert!(core::mem::size_of::<Velocity>() == 16);
    assert!(core::mem::align_of::<Velocity>() == 4);
    assert!(core::mem::size_of::<SimParams>() == 16);
    assert!(core::mem::align_of::<SimParams>() == 4);
};

/// Resolves `T` as an init-validated [`TypedComponent<T>`] on the host world, registering it if
/// the host has not pre-registered it.
fn resolve_or_register_typed<T: GuestComponent>(
    app: &mut GuestApp<'_>,
) -> Result<TypedComponent<T>, FfiResult> {
    match app.resolve_component_typed::<T>() {
        Ok(comp) => Ok(comp),
        Err(FfiResult::NOT_FOUND) => app.register_component_typed::<T>(),
        Err(err) => Err(err),
    }
}

/// Benchmark guest plugin implementing all four ABI `0.4.0` benchmark workloads.
#[derive(Default)]
pub struct BenchGuestRust;

impl GuestPlugin for BenchGuestRust {
    const NAME: &'static CStr = c"bench_guest_rust";
    const VERSION: &'static CStr = c"0.4.0";

    fn build(&mut self, app: &mut GuestApp<'_>) -> FfiResult {
        let pos_comp = match resolve_or_register_typed::<Position>(app) {
            Ok(c) => c,
            Err(err) => return err,
        };
        if let Err(err) = resolve_or_register_typed::<Velocity>(app) {
            return err;
        }
        if let Err(err) = resolve_or_register_typed::<SimParams>(app) {
            return err;
        }

        let rc = app
            .system(c"bench_noop")
            .schedule(ScheduleId::UPDATE)
            .build(|_ctx| FfiResult::OK);
        if !rc.is_ok() {
            return rc;
        }

        let rc = app
            .system(c"bench_dense_update")
            .schedule(ScheduleId::UPDATE)
            .query_typed::<(&mut Position, &Velocity)>()
            .build(|ctx| {
                const DT: f32 = 0.016;
                ctx.single_query::<(&mut Position, &Velocity)>().for_each(
                    |(positions, velocities)| {
                        for (pos, vel) in positions.iter_mut().zip(velocities.iter()) {
                            pos.x += vel.vx * DT;
                            pos.y += vel.vy * DT;
                            pos.z += vel.vz * DT;
                            pos.w += vel.vw * DT;
                        }
                    },
                )
            });
        if !rc.is_ok() {
            return rc;
        }

        let rc = app
            .system(c"bench_command_burst")
            .schedule(ScheduleId::UPDATE)
            .build(move |ctx| {
                let cmds = ctx.commands();
                let sample_pos = Position {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                    w: 4.0,
                };
                for _ in 0..64 {
                    let entity = match cmds.spawn_empty() {
                        Ok(e) => e,
                        Err(err) => return err,
                    };
                    let status = cmds.insert_typed(entity, pos_comp, &sample_pos);
                    if !status.is_ok() {
                        return status;
                    }
                }

                let checkpoint = match cmds.checkpoint() {
                    Ok(cp) => cp,
                    Err(err) => return err,
                };
                if let Err(err) = cmds.spawn_empty() {
                    let _ = cmds.rollback(checkpoint);
                    return err;
                }
                cmds.rollback(checkpoint)
            });
        if !rc.is_ok() {
            return rc;
        }

        app.system(c"bench_host_api_roundtrip")
            .schedule(ScheduleId::UPDATE)
            .query(|q| q.resource_read::<SimParams>())
            .build_with_local(0u64, |ctx, sink| {
                let mut acc = 0u64;
                for _ in 0..64 {
                    let res = match ctx.res_with_ticks::<SimParams>(0) {
                        Ok(r) => r,
                        Err(err) => return err,
                    };
                    let ticks = res.ticks();
                    let sym = match ctx.intern_symbol("bench.sym") {
                        Ok(s) => s,
                        Err(err) => return err,
                    };
                    acc = acc
                        .wrapping_add(u64::from(ticks.this_run))
                        .wrapping_add(u64::from(res.tick))
                        .wrapping_add(u64::from(res.dt.to_bits()))
                        .wrapping_add(u64::from(sym.to_raw()));
                }
                *sink = core::hint::black_box(acc);
                FfiResult::OK
            })
    }
}

export_mod!(plugin: BenchGuestRust);
