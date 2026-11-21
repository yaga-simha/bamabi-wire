//! Layer-0 (zero-Bevy, zero-external-dependency) polyglot benchmark host for `bamabi-wire`.
//!
//! Dynamically loads compiled guest `.dylib` / `.so` / `.dll` modules targeting `ABI 0.3.0`
//! (`0x0003_0000`), verifies strict functional correctness against an in-process `native` Rust
//! baseline, measures latency and throughput, and prints an aligned Markdown table plus an
//! optional JSON report.

mod host;
mod loader;
mod workloads;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use workloads::{BenchConfig, BenchReport, GuestSpec, WorkloadKind, run_benchmarks};

/// Parsed CLI actions before benchmark execution.
#[derive(Debug, PartialEq, Eq)]
enum CliAction {
    Help,
    Run(ParsedCli),
}

/// Validated command-line options for `bamabi_bench`.
#[derive(Debug, PartialEq, Eq)]
struct ParsedCli {
    guests: Vec<GuestSpec>,
    workloads: Vec<WorkloadKind>,
    entities: Vec<usize>,
    chunks: Vec<usize>,
    quick: bool,
    json_path: Option<PathBuf>,
    markdown_path: Option<PathBuf>,
}

fn print_help() {
    println!(
        "\
bamabi_bench — Layer-0 C-ABI 0.3.0 polyglot benchmark host

USAGE:
    bamabi_bench [OPTIONS]

OPTIONS:
    --guest <lang>:<profile>=<path>   Guest shared library to verify & benchmark (repeatable)
    --workloads <csv>                 Comma-separated workloads:
                                      noop,dense_query_update,command_burst,host_api_roundtrip
    --entities <csv>                  Entity counts for dense_query_update (default: 1000,100000)
    --chunks <csv>                    Chunk sizes for dense_query_update (default: 256,4096; quick: 256)
    --quick                           Fast mode with fewer warmup/timed samples
    --json <path>                     Write JSON report to <path>
    --markdown <path>                 Write Markdown report to <path>
    -h, --help                        Print this help message"
    );
}

fn parse_guest_spec(raw: &str) -> Result<GuestSpec, String> {
    let (label_part, path_str) = raw
        .split_once('=')
        .ok_or_else(|| format!("invalid --guest `{raw}`: expected `<lang>:<profile>=<path>`"))?;
    if path_str.trim().is_empty() {
        return Err(format!("invalid --guest `{raw}`: path must not be empty"));
    }
    let (lang, profile) = if let Some((l, p)) = label_part.split_once(':') {
        (l.trim(), p.trim())
    } else {
        (label_part.trim(), "default")
    };
    if lang.is_empty() || profile.is_empty() {
        return Err(format!(
            "invalid --guest `{raw}`: language and profile must not be empty"
        ));
    }
    Ok(GuestSpec {
        lang: lang.to_owned(),
        profile: profile.to_owned(),
        path: PathBuf::from(path_str),
    })
}

fn parse_usize_csv(raw: &str, flag_name: &str) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for item in raw.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            return Err(format!("empty value in {flag_name} `{raw}`"));
        }
        let val: usize = trimmed
            .parse()
            .map_err(|_| format!("invalid integer `{trimmed}` in {flag_name}"))?;
        if val == 0 {
            return Err(format!("{flag_name} entries must be >= 1 (got 0)"));
        }
        if !out.contains(&val) {
            out.push(val);
        }
    }
    if out.is_empty() {
        return Err(format!("{flag_name} must specify at least one value"));
    }
    Ok(out)
}

fn parse_workloads_csv(raw: &str) -> Result<Vec<WorkloadKind>, String> {
    let mut out = Vec::new();
    for item in raw.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            return Err(format!("empty value in --workloads `{raw}`"));
        }
        let kind = WorkloadKind::parse(trimmed)?;
        if !out.contains(&kind) {
            out.push(kind);
        }
    }
    if out.is_empty() {
        return Err("--workloads must specify at least one workload".to_owned());
    }
    Ok(out)
}

fn parse_args<I, S>(args: I) -> Result<CliAction, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let raw_args: Vec<String> = args.into_iter().map(|s| s.as_ref().to_owned()).collect();
    let mut iter = raw_args.into_iter().peekable();
    let mut guests = Vec::new();
    let mut workloads: Option<Vec<WorkloadKind>> = None;
    let mut entities: Option<Vec<usize>> = None;
    let mut chunks: Option<Vec<usize>> = None;
    let mut quick = false;
    let mut json_path: Option<PathBuf> = None;
    let mut markdown_path: Option<PathBuf> = None;

    while let Some(arg) = iter.next() {
        if arg == "-h" || arg == "--help" {
            return Ok(CliAction::Help);
        }
        if arg == "--quick" {
            quick = true;
            continue;
        }
        let (flag, inline_val) = if let Some((k, v)) = arg.split_once('=') {
            (k.to_owned(), Some(v.to_owned()))
        } else {
            (arg, None)
        };
        let take_val = |inline: Option<String>,
                        it: &mut core::iter::Peekable<std::vec::IntoIter<String>>,
                        flag_name: &str|
         -> Result<String, String> {
            match inline {
                Some(v) => Ok(v),
                None => it
                    .next()
                    .ok_or_else(|| format!("missing value for `{flag_name}`")),
            }
        };
        match flag.as_str() {
            "--guest" => {
                let val = take_val(inline_val, &mut iter, "--guest")?;
                guests.push(parse_guest_spec(&val)?);
            }
            "--workloads" => {
                let val = take_val(inline_val, &mut iter, "--workloads")?;
                workloads = Some(parse_workloads_csv(&val)?);
            }
            "--entities" => {
                let val = take_val(inline_val, &mut iter, "--entities")?;
                entities = Some(parse_usize_csv(&val, "--entities")?);
            }
            "--chunks" => {
                let val = take_val(inline_val, &mut iter, "--chunks")?;
                chunks = Some(parse_usize_csv(&val, "--chunks")?);
            }
            "--json" => {
                let val = take_val(inline_val, &mut iter, "--json")?;
                if val.trim().is_empty() {
                    return Err("--json path must not be empty".to_owned());
                }
                json_path = Some(PathBuf::from(val));
            }
            "--markdown" => {
                let val = take_val(inline_val, &mut iter, "--markdown")?;
                if val.trim().is_empty() {
                    return Err("--markdown path must not be empty".to_owned());
                }
                markdown_path = Some(PathBuf::from(val));
            }
            other => {
                return Err(format!(
                    "unknown argument `{other}` (pass --help for usage)"
                ));
            }
        }
    }

    let resolved_entities = entities.unwrap_or_else(|| vec![1_000, 100_000]);
    let resolved_chunks =
        chunks.unwrap_or_else(|| if quick { vec![256] } else { vec![256, 4_096] });

    Ok(CliAction::Run(ParsedCli {
        guests,
        workloads: workloads.unwrap_or_else(|| WorkloadKind::ALL.to_vec()),
        entities: resolved_entities,
        chunks: resolved_chunks,
        quick,
        json_path,
        markdown_path,
    }))
}

fn format_opt_usize(v: Option<usize>) -> String {
    match v {
        Some(n) => n.to_string(),
        None => "-".to_owned(),
    }
}

fn format_ns(v: f64) -> String {
    if v >= 1_000_000.0 {
        format!("{v:.0}")
    } else if v >= 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

fn format_opt_float(v: Option<f64>, decimals: usize) -> String {
    match v {
        Some(val) => format!("{val:.decimals$}"),
        None => "-".to_owned(),
    }
}

fn render_aligned_table<const N: usize>(
    out: &mut String,
    headers: [&str; N],
    right_aligned: [bool; N],
    rows: &[[String; N]],
) {
    let mut widths = [0usize; N];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = h.len().max(3);
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let push_row = |buf: &mut String, cells: &[String; N]| {
        buf.push('|');
        for (i, cell) in cells.iter().enumerate() {
            if right_aligned[i] {
                buf.push_str(&format!(" {:>w$} |", cell, w = widths[i]));
            } else {
                buf.push_str(&format!(" {:<w$} |", cell, w = widths[i]));
            }
        }
        buf.push('\n');
    };

    let header_strings: [String; N] = headers.map(str::to_owned);
    push_row(out, &header_strings);

    out.push('|');
    for (i, &w) in widths.iter().enumerate() {
        if right_aligned[i] {
            out.push_str(&format!(" {}: |", "-".repeat(w.saturating_sub(1))));
        } else {
            out.push_str(&format!(" {} |", "-".repeat(w)));
        }
    }
    out.push('\n');

    for row in rows {
        push_row(out, row);
    }
}

fn format_workload_label(
    workload: WorkloadKind,
    entities: Option<usize>,
    chunk_size: Option<usize>,
) -> String {
    match (entities, chunk_size) {
        (Some(e), Some(c)) => format!("{} ({e}, c{c})", workload.as_str()),
        _ => workload.as_str().to_owned(),
    }
}

/// Renders an aligned GitHub-flavored Markdown benchmark report from `report`.
fn render_markdown_report(report: &BenchReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "### bamabi-wire ABI 0.3.0 Polyglot Benchmark Report ({mode}, warmup={w}, samples={s})\n\n",
        mode = if report.quick { "quick" } else { "full" },
        w = report.warmup_samples,
        s = report.timed_samples,
    ));

    if !report.guest_manifests.is_empty() {
        out.push_str("#### Verified Guests\n\n");
        for (subject, name, ver) in &report.guest_manifests {
            out.push_str(&format!("- `{subject}`: `{name}` v{ver} (ABI 0.3.0)\n"));
        }
        out.push('\n');
    }

    let has_release_guest = report
        .rows
        .iter()
        .any(|r| r.lang != "native" && r.profile == "release");
    if has_release_guest {
        let mut keys: Vec<(WorkloadKind, Option<usize>, Option<usize>)> = Vec::new();
        for r in &report.rows {
            let key = (r.workload, r.entities, r.chunk_size);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }

        let mut summary_rows: Vec<[String; 4]> = Vec::with_capacity(keys.len());
        for (workload, entities, chunk_size) in keys {
            let matching: Vec<_> = report
                .rows
                .iter()
                .filter(|r| {
                    r.workload == workload && r.entities == entities && r.chunk_size == chunk_size
                })
                .collect();
            let native_cell = matching
                .iter()
                .find(|r| r.lang == "native")
                .map_or_else(|| "-".to_owned(), |r| format_ns(r.stats.median_ns));
            let fastest_cell = matching
                .iter()
                .filter(|r| r.lang != "native" && r.profile == "release")
                .min_by(|a, b| a.stats.median_ns.total_cmp(&b.stats.median_ns))
                .map_or_else(
                    || "-".to_owned(),
                    |r| {
                        format!(
                            "`{}`: {} ns ({:.2}x)",
                            r.subject,
                            format_ns(r.stats.median_ns),
                            r.ratio_vs_native
                        )
                    },
                );
            let rust_cell = matching
                .iter()
                .find(|r| r.subject == "rust:release")
                .map_or_else(
                    || "-".to_owned(),
                    |r| {
                        format!(
                            "{} ns ({:.2}x)",
                            format_ns(r.stats.median_ns),
                            r.ratio_vs_native
                        )
                    },
                );
            summary_rows.push([
                format_workload_label(workload, entities, chunk_size),
                native_cell,
                fastest_cell,
                rust_cell,
            ]);
        }

        out.push_str("#### Summary Highlights (Release Guests vs Native)\n\n");
        render_aligned_table(
            &mut out,
            [
                "Workload",
                "Native Median (ns)",
                "Fastest Release Guest",
                "Safe Rust (rust:release)",
            ],
            [false, true, false, true],
            &summary_rows,
        );
        out.push('\n');
    }

    let mut table_rows: Vec<[String; 11]> = Vec::with_capacity(report.rows.len());
    for r in &report.rows {
        table_rows.push([
            r.subject.clone(),
            r.workload.as_str().to_owned(),
            format_opt_usize(r.entities),
            format_opt_usize(r.chunk_size),
            format_ns(r.stats.min_ns),
            format_ns(r.stats.median_ns),
            format_ns(r.stats.p95_ns),
            format_ns(r.stats.mean_ns),
            format_opt_float(r.ns_per_entity, 3),
            format_opt_float(r.m_entities_per_sec, 1),
            format!("{:.2}x", r.ratio_vs_native),
        ]);
    }

    out.push_str("#### Full Permutation Results\n\n");
    render_aligned_table(
        &mut out,
        [
            "Subject",
            "Workload",
            "Entities",
            "Chunk",
            "Min (ns)",
            "Median (ns)",
            "P95 (ns)",
            "Mean (ns)",
            "ns/entity",
            "M ent/s",
            "vs Native",
        ],
        [
            false, false, true, true, true, true, true, true, true, true, true,
        ],
        &table_rows,
    );
    out
}

fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_f64(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.4}")
    } else {
        "null".to_owned()
    }
}

fn json_opt_f64(v: Option<f64>) -> String {
    match v {
        Some(val) => json_f64(val),
        None => "null".to_owned(),
    }
}

fn json_opt_usize(v: Option<usize>) -> String {
    match v {
        Some(val) => val.to_string(),
        None => "null".to_owned(),
    }
}

/// Hand-rolls a structured JSON report string without external crates.
fn render_json_report(report: &BenchReport) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"abi_version\": \"0.3.0\",\n");
    out.push_str(&format!("  \"quick\": {},\n", report.quick));
    out.push_str(&format!(
        "  \"warmup_samples\": {},\n",
        report.warmup_samples
    ));
    out.push_str(&format!("  \"timed_samples\": {},\n", report.timed_samples));
    out.push_str("  \"guests\": [\n");
    for (idx, (subject, name, ver)) in report.guest_manifests.iter().enumerate() {
        let comma = if idx + 1 < report.guest_manifests.len() {
            ","
        } else {
            ""
        };
        out.push_str(&format!(
            "    {{\"subject\": {}, \"manifest_name\": {}, \"manifest_version\": {}}}{}\n",
            escape_json_str(subject),
            escape_json_str(name),
            escape_json_str(ver),
            comma
        ));
    }
    out.push_str("  ],\n");
    out.push_str("  \"results\": [\n");
    for (idx, r) in report.rows.iter().enumerate() {
        let comma = if idx + 1 < report.rows.len() { "," } else { "" };
        out.push_str("    {\n");
        out.push_str(&format!(
            "      \"subject\": {},\n",
            escape_json_str(&r.subject)
        ));
        out.push_str(&format!("      \"lang\": {},\n", escape_json_str(&r.lang)));
        out.push_str(&format!(
            "      \"profile\": {},\n",
            escape_json_str(&r.profile)
        ));
        out.push_str(&format!(
            "      \"workload\": {},\n",
            escape_json_str(r.workload.as_str())
        ));
        out.push_str(&format!(
            "      \"entities\": {},\n",
            json_opt_usize(r.entities)
        ));
        out.push_str(&format!(
            "      \"chunk_size\": {},\n",
            json_opt_usize(r.chunk_size)
        ));
        out.push_str(&format!(
            "      \"min_ns\": {},\n",
            json_f64(r.stats.min_ns)
        ));
        out.push_str(&format!(
            "      \"median_ns\": {},\n",
            json_f64(r.stats.median_ns)
        ));
        out.push_str(&format!(
            "      \"p95_ns\": {},\n",
            json_f64(r.stats.p95_ns)
        ));
        out.push_str(&format!(
            "      \"mean_ns\": {},\n",
            json_f64(r.stats.mean_ns)
        ));
        out.push_str(&format!(
            "      \"ns_per_entity\": {},\n",
            json_opt_f64(r.ns_per_entity)
        ));
        out.push_str(&format!(
            "      \"m_entities_per_sec\": {},\n",
            json_opt_f64(r.m_entities_per_sec)
        ));
        out.push_str(&format!(
            "      \"ratio_vs_native\": {},\n",
            json_f64(r.ratio_vs_native)
        ));
        out.push_str(&format!("      \"verified\": {}\n", r.verified));
        out.push_str(&format!("    }}{comma}\n"));
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

fn write_report_file(path: &Path, label: &str, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
    }
    fs::write(path, contents.as_bytes())
        .map_err(|e| format!("failed to write {label} report to {}: {e}", path.display()))
}

fn main() -> ExitCode {
    let parsed = match parse_args(env::args().skip(1)) {
        Ok(CliAction::Help) => {
            print_help();
            return ExitCode::SUCCESS;
        }
        Ok(CliAction::Run(cli)) => cli,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let config = BenchConfig {
        guests: parsed.guests,
        workloads: parsed.workloads,
        entities: parsed.entities,
        chunks: parsed.chunks,
        quick: parsed.quick,
        warmup_samples: if parsed.quick { 5 } else { 15 },
        timed_samples: if parsed.quick { 20 } else { 60 },
    };

    let report = match run_benchmarks(&config) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("benchmark failed: {err}");
            return ExitCode::FAILURE;
        }
    };

    let md = render_markdown_report(&report);
    print!("{md}");

    if let Some(ref markdown_path) = parsed.markdown_path
        && let Err(err) = write_report_file(markdown_path, "Markdown", &md)
    {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }

    if let Some(ref json_path) = parsed.json_path {
        let json = render_json_report(&report);
        if let Err(err) = write_report_file(json_path, "JSON", &json) {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use core::ffi::c_void;

    use bamabi_sys::{
        ABI_VERSION, AccessMode, BamChangeTicks, BamColumnInfo, BamComponent, BamHostApi,
        BamLayoutInfo, BamSymbol, BamTermDesc, ComponentDesc, FfiBool, FfiEntity, FfiResult,
        StrView,
    };

    use super::*;
    use crate::host::{
        BENCH_DT, HostWorld, POSITION_NAME, POSITION_UUID, Position, SIM_PARAMS_NAME,
        SIM_PARAMS_UUID, SimParams, VELOCITY_NAME, VELOCITY_UUID, Velocity, initial_position,
        initial_velocity,
    };
    use crate::workloads::{
        compute_stats, native_command_burst, native_dense_update, position_checksum,
    };

    #[test]
    fn host_api_table_is_complete_and_valid() {
        let world = HostWorld::new();
        assert_eq!(
            world.api.struct_size as usize,
            core::mem::size_of::<BamHostApi>()
        );
        assert_eq!(world.api.abi_version, ABI_VERSION);
        assert!(world.api.has_all_slots());
        assert!(world.api.validate_for_guest(ABI_VERSION).is_ok());
    }

    #[test]
    fn pre_registered_components_resolve_by_name_and_uuid() {
        let world = HostWorld::new();
        let host_ctx = world.host_ctx_ptr();
        let resolve = world.api.resolve_component.unwrap();
        let resolve_uuid = world.api.resolve_component_by_uuid.unwrap();
        let layout_fn = world.api.component_layout_info.unwrap();

        for (c_name, uuid, expected_handle) in [
            (c"bench::Position", POSITION_UUID, world.pos_handle),
            (c"bench::Velocity", VELOCITY_UUID, world.vel_handle),
            (
                c"bench::SimParams",
                SIM_PARAMS_UUID,
                world.sim_params_handle,
            ),
        ] {
            let mut out = BamComponent::INVALID;
            // SAFETY: `host_ctx` and `c_name` are valid and `out` is stack storage.
            let res = unsafe { resolve(host_ctx, c_name.as_ptr(), &uuid, &mut out) };
            assert_eq!(res, FfiResult::OK);
            assert_eq!(out, expected_handle);

            let mut by_uuid = BamComponent::INVALID;
            // SAFETY: `host_ctx` and `uuid` are valid and `by_uuid` is stack storage.
            let res2 = unsafe { resolve_uuid(host_ctx, &uuid, &mut by_uuid) };
            assert_eq!(res2, FfiResult::OK);
            assert_eq!(by_uuid, expected_handle);

            let mut info = BamLayoutInfo::empty();
            // SAFETY: `host_ctx` is valid and `info` is writable stack storage.
            let res3 = unsafe { layout_fn(host_ctx, expected_handle, &mut info) };
            assert_eq!(res3, FfiResult::OK);
            assert_eq!(info.size, 16);
            assert_eq!(info.align, 4);
            assert_eq!(info.uuid, uuid);
        }

        // Idempotent registration of existing component succeeds; mismatched size fails.
        let reg = world.api.register_component.unwrap();
        let pos_desc = ComponentDesc::new(c"bench::Position".as_ptr(), POSITION_UUID, 16, 4);
        let mut reg_out = BamComponent::INVALID;
        // SAFETY: `load_ctx_ptr` and `pos_desc` are valid during init.
        let r_ok = unsafe { reg(world.load_ctx_ptr(), &pos_desc, &mut reg_out) };
        assert_eq!(r_ok, FfiResult::OK);
        assert_eq!(reg_out, world.pos_handle);

        let bad_desc = ComponentDesc::new(c"bench::Position".as_ptr(), POSITION_UUID, 32, 4);
        // SAFETY: `load_ctx_ptr` and `bad_desc` are valid during init.
        let r_bad = unsafe { reg(world.load_ctx_ptr(), &bad_desc, &mut reg_out) };
        assert_eq!(r_bad, FfiResult::TYPE_MISMATCH);
        let _ = (POSITION_NAME, VELOCITY_NAME, SIM_PARAMS_NAME);
    }

    #[test]
    fn dense_chunk_storage_and_reset_work_across_partial_chunks() {
        let mut world = HostWorld::new();
        world.setup_dense_entities(530, 256);
        assert_eq!(world.chunks.len(), 3);
        assert_eq!(world.chunks[0].entities.len(), 256);
        assert_eq!(world.chunks[1].entities.len(), 256);
        assert_eq!(world.chunks[2].entities.len(), 18);

        let before = world.read_all_positions();
        assert_eq!(before.len(), 530);
        assert_eq!(before[0], initial_position(0));
        assert_eq!(before[529], initial_position(529));

        assert_eq!(native_dense_update(&world), FfiResult::OK);
        assert!(world.chunks[0].is_marked_changed(world.pos_handle));
        assert!(world.chunks[2].is_marked_changed(world.pos_handle));

        let after = world.read_all_positions();
        let v0 = initial_velocity(0);
        assert!((after[0].x - (before[0].x + v0.vx * BENCH_DT)).abs() < 1e-6);

        world.reset_dense_positions();
        assert!(!world.chunks[0].is_marked_changed(world.pos_handle));
        assert_eq!(world.read_all_positions(), before);
    }

    #[test]
    fn command_queue_checkpoint_and_rollback_match_contract() {
        let mut world = HostWorld::new();
        assert_eq!(native_command_burst(&mut world), FfiResult::OK);
        let stats = world.command_stats();
        assert_eq!(stats.committed_spawns, 64);
        assert_eq!(stats.committed_inserts, 64);
        assert_eq!(stats.checkpoints, 1);
        assert_eq!(stats.rollbacks, 1);
        assert_eq!(stats.rolled_back_ops, 1);
        assert_eq!(stats.last_insert_component, world.pos_handle);
        assert_eq!(stats.last_insert_len, 16);

        // Out-of-range rollback token is rejected cleanly.
        let rb = world.api.cmd_rollback.unwrap();
        let cmds = core::ptr::from_mut(world.commands_mut()).cast::<bamabi_sys::BamCommands>();
        // SAFETY: `cmds` points to `world`'s `CommandQueue` outside guest execution.
        let bad_rb = unsafe { rb(cmds, 99_999) };
        assert_eq!(bad_rb, FfiResult::OUT_OF_RANGE);
    }

    #[test]
    fn host_api_system_execution_covers_chunks_resources_and_interning() {
        #[allow(clippy::cast_ptr_alignment)]
        unsafe fn cast_aligned_slice_mut<'a, T>(ptr: *mut u8, len: usize) -> &'a mut [T] {
            assert_eq!(ptr.align_offset(core::mem::align_of::<T>()), 0);
            // SAFETY: Caller verified `ptr` points to `len` writable aligned `T` elements.
            unsafe { core::slice::from_raw_parts_mut(ptr.cast::<T>(), len) }
        }

        #[allow(clippy::cast_ptr_alignment)]
        unsafe fn cast_aligned_slice<'a, T>(ptr: *const u8, len: usize) -> &'a [T] {
            assert_eq!(ptr.align_offset(core::mem::align_of::<T>()), 0);
            // SAFETY: Caller verified `ptr` points to `len` readable aligned `T` elements.
            unsafe { core::slice::from_raw_parts(ptr.cast::<T>(), len) }
        }

        #[allow(clippy::cast_ptr_alignment)]
        unsafe fn cast_aligned_ref<'a, T>(ptr: *const u8) -> &'a T {
            assert!(!ptr.is_null());
            assert_eq!(ptr.align_offset(core::mem::align_of::<T>()), 0);
            // SAFETY: Caller verified `ptr` is non-null, aligned, and initialized as `T`.
            unsafe { &*ptr.cast::<T>() }
        }

        unsafe extern "C" fn test_sys(
            sys_ctx: *mut bamabi_sys::BamSystemCtx,
            user_data: *mut c_void,
        ) -> FfiResult {
            // SAFETY: `user_data` points to `BamHostApi` passed by test harness.
            let api = unsafe { &*user_data.cast::<BamHostApi>() };
            // SAFETY: `sys_ctx` is valid for this callback invocation.
            let host_ctx = unsafe { (api.sys_host_ctx.unwrap())(sys_ctx) };

            let mut iter0 = core::ptr::null_mut();
            // SAFETY: Query 0 exists on this registered system.
            let q_res = unsafe { (api.sys_query.unwrap())(sys_ctx, 0, &mut iter0) };
            assert_eq!(q_res, FfiResult::OK);

            let mut total_entities = 0usize;
            // SAFETY: `iter0` is a valid query iterator.
            let cnt_res = unsafe { (api.iter_count.unwrap())(iter0, &mut total_entities) };
            assert_eq!(cnt_res, FfiResult::OK);
            assert_eq!(total_entities, 300);

            let mut len = 0usize;
            // SAFETY: `iter0` and `&mut len` are valid.
            while unsafe { (api.iter_next_chunk.unwrap())(iter0, &mut len) } == FfiBool::TRUE {
                // SAFETY: Active chunk has valid dense column 0 (`Position`).
                let pos_raw = unsafe { (api.iter_column_mutable.unwrap())(iter0, 0) };
                // SAFETY: Active chunk has valid dense column 1 (`Velocity`).
                let vel_raw = unsafe { (api.iter_column_readonly.unwrap())(iter0, 1) };
                // SAFETY: `pos_raw` and `vel_raw` cover `len` disjoint aligned elements.
                let (pos, vel) = unsafe {
                    (
                        cast_aligned_slice_mut::<Position>(pos_raw, len),
                        cast_aligned_slice::<Velocity>(vel_raw, len),
                    )
                };
                for (p, v) in pos.iter_mut().zip(vel.iter()) {
                    p.x += v.vx * BENCH_DT;
                    p.y += v.vy * BENCH_DT;
                    p.z += v.vz * BENCH_DT;
                    p.w += v.vw * BENCH_DT;
                }
            }

            // Reset and check column info on resource term (index 2).
            // SAFETY: `iter0` is valid.
            let reset_res = unsafe { (api.iter_reset.unwrap())(iter0) };
            assert_eq!(reset_res, FfiResult::OK);
            let mut info = BamColumnInfo {
                component: BamComponent::INVALID,
                mode: AccessMode::READ,
                uuid: Default::default(),
                size: 0,
                align: 0,
                storage_flags: 0,
                reserved0: 0,
            };
            // SAFETY: Term 2 is `SimParams` (`RESOURCE_READ`).
            let info_res = unsafe { (api.iter_column_info.unwrap())(iter0, 2, &mut info) };
            assert_eq!(info_res, FfiResult::OK);
            assert_eq!(info.size, 16);
            assert_eq!(info.uuid, SIM_PARAMS_UUID);

            // SAFETY: Term 2 is `SimParams`.
            let rp_raw = unsafe { (api.res_ptr.unwrap())(iter0, 2) };
            // SAFETY: `rp_raw` points to initialized `SimParams`.
            let rp = unsafe { cast_aligned_ref::<SimParams>(rp_raw) };
            assert!((rp.dt - BENCH_DT).abs() < 1e-6);

            let mut ticks = BamChangeTicks::default();
            // SAFETY: Term 2 is `SimParams`.
            let ticks_res = unsafe { (api.res_ticks.unwrap())(iter0, 2, &mut ticks) };
            assert_eq!(ticks_res, FfiResult::OK);
            assert_eq!(ticks.this_run, 1);

            let mut sym = BamSymbol::INVALID;
            // SAFETY: `host_ctx` is valid and `"bench.sym"` is `'static`.
            let intern_res = unsafe {
                (api.intern_symbol.unwrap())(host_ctx, StrView::from_str("bench.sym"), &mut sym)
            };
            assert_eq!(intern_res, FfiResult::OK);
            assert!(sym.is_valid());

            // Commands check: spawn + insert + checkpoint + spawn + rollback.
            let mut cmds = core::ptr::null_mut();
            // SAFETY: `sys_ctx` is valid.
            let cmds_res = unsafe { (api.sys_commands.unwrap())(sys_ctx, &mut cmds) };
            assert_eq!(cmds_res, FfiResult::OK);
            let mut ent = FfiEntity::NULL;
            // SAFETY: `cmds` is valid.
            let spawn1_res = unsafe {
                (api.cmd_spawn.unwrap())(cmds, core::ptr::null(), core::ptr::null(), 0, &mut ent)
            };
            assert_eq!(spawn1_res, FfiResult::OK);
            assert!(ent.is_valid());
            let mut cp = 0usize;
            // SAFETY: `cmds` is valid.
            let cp_res = unsafe { (api.cmd_checkpoint.unwrap())(cmds, &mut cp) };
            assert_eq!(cp_res, FfiResult::OK);
            let mut ent2 = FfiEntity::NULL;
            // SAFETY: `cmds` is valid.
            let spawn2_res = unsafe {
                (api.cmd_spawn.unwrap())(cmds, core::ptr::null(), core::ptr::null(), 0, &mut ent2)
            };
            assert_eq!(spawn2_res, FfiResult::OK);
            // SAFETY: `cmds` is valid.
            let rb_res = unsafe { (api.cmd_rollback.unwrap())(cmds, cp) };
            assert_eq!(rb_res, FfiResult::OK);

            FfiResult::OK
        }

        let mut world = HostWorld::new();
        world.setup_dense_entities(300, 128);
        let terms = [
            BamTermDesc::write(world.pos_handle),
            BamTermDesc::read(world.vel_handle),
            BamTermDesc::resource_read(world.sim_params_handle),
        ];
        let queries = [bamabi_sys::BamQueryDesc::new(terms.as_ptr(), terms.len())];
        let api_ptr = world.api_ptr().cast_mut().cast::<c_void>();
        let sys_desc = bamabi_sys::BamSystemDesc::new(
            c"test_sys".as_ptr(),
            queries.as_ptr(),
            queries.len(),
            test_sys,
            api_ptr,
        );
        let reg_sys = world.api.register_system.unwrap();
        // SAFETY: `load_ctx_ptr` and `sys_desc` are valid during setup.
        let reg_res = unsafe { reg_sys(world.load_ctx_ptr(), &sys_desc) };
        assert_eq!(reg_res, FfiResult::OK);
        let idx = world.find_system("test_sys").unwrap();
        assert_eq!(world.run_system(idx), FfiResult::OK);
        assert_eq!(world.intern_symbol_calls(), 1);
        assert_eq!(world.res_ticks_calls(), 1);
        assert_eq!(world.command_stats().committed_spawns, 1);
        assert_eq!(world.command_stats().rollbacks, 1);
    }

    #[test]
    fn stats_computation_and_cli_parsing_are_exact() {
        let stats = compute_stats(&[40.0, 10.0, 20.0, 30.0]);
        assert!((stats.min_ns - 10.0).abs() < 1e-9);
        assert!((stats.median_ns - 25.0).abs() < 1e-9);
        assert!((stats.mean_ns - 25.0).abs() < 1e-9);
        assert!((stats.p95_ns - 40.0).abs() < 1e-9);

        let odd = compute_stats(&[50.0, 10.0, 30.0, 20.0, 40.0]);
        assert!((odd.median_ns - 30.0).abs() < 1e-9);

        let parsed = parse_args([
            "--guest",
            "rust:release=/tmp/libguest.dylib",
            "--guest=c99:debug=C:\\mods\\guest.dll",
            "--workloads",
            "noop,dense_query_update",
            "--entities",
            "500,2000",
            "--chunks",
            "128",
            "--quick",
            "--json",
            "target/out.json",
            "--markdown=target/out.md",
        ])
        .unwrap();

        match parsed {
            CliAction::Run(cli) => {
                assert_eq!(cli.guests.len(), 2);
                assert_eq!(cli.guests[0].lang, "rust");
                assert_eq!(cli.guests[0].profile, "release");
                assert_eq!(cli.guests[0].path, PathBuf::from("/tmp/libguest.dylib"));
                assert_eq!(cli.guests[1].lang, "c99");
                assert_eq!(cli.guests[1].profile, "debug");
                assert_eq!(cli.guests[1].path, PathBuf::from("C:\\mods\\guest.dll"));
                assert_eq!(
                    cli.workloads,
                    vec![WorkloadKind::Noop, WorkloadKind::DenseQueryUpdate]
                );
                assert_eq!(cli.entities, vec![500, 2000]);
                assert_eq!(cli.chunks, vec![128]);
                assert!(cli.quick);
                assert_eq!(cli.json_path, Some(PathBuf::from("target/out.json")));
                assert_eq!(cli.markdown_path, Some(PathBuf::from("target/out.md")));
            }
            CliAction::Help => panic!("expected Run"),
        }

        assert!(parse_args(["--entities", "0"]).is_err());
        assert!(parse_args(["--workloads", "bad_workload"]).is_err());
        assert!(parse_args(["--guest", "missing_equals"]).is_err());
        assert!(parse_args(["--markdown", " "]).is_err());
    }

    #[test]
    fn native_baseline_runs_and_renders_markdown_and_json() {
        let config = BenchConfig {
            guests: Vec::new(),
            workloads: WorkloadKind::ALL.to_vec(),
            entities: vec![256],
            chunks: vec![64],
            quick: true,
            warmup_samples: 1,
            timed_samples: 3,
        };
        let report = run_benchmarks(&config).expect("native baseline must succeed");
        assert_eq!(report.rows.len(), 4);
        assert!(
            report
                .rows
                .iter()
                .all(|r| r.verified && r.subject == "native")
        );

        let md = render_markdown_report(&report);
        assert!(md.contains("Polyglot Benchmark Report"));
        assert!(md.contains("#### Full Permutation Results"));
        assert!(md.contains("| Subject"));
        assert!(md.contains("dense_query_update"));
        assert!(md.contains("1.00x"));

        // Synthetic report with a release guest verifies Summary Highlights rendering.
        let mut with_guest = report.clone();
        with_guest.guest_manifests.push((
            "rust:release".to_owned(),
            "bench_guest_rust".to_owned(),
            "0.3.0".to_owned(),
        ));
        let mut guest_row = with_guest.rows[0].clone();
        guest_row.subject = "rust:release".to_owned();
        guest_row.lang = "rust".to_owned();
        guest_row.profile = "release".to_owned();
        guest_row.ratio_vs_native = 2.0;
        with_guest.rows.push(guest_row);
        let guest_md = render_markdown_report(&with_guest);
        assert!(guest_md.contains("#### Verified Guests"));
        assert!(guest_md.contains("#### Summary Highlights (Release Guests vs Native)"));
        assert!(guest_md.contains("`rust:release`"));

        let json = render_json_report(&report);
        assert!(json.contains("\"abi_version\": \"0.3.0\""));
        assert!(json.contains("\"verified\": true"));

        let csum = position_checksum(&[Position {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            w: 4.0,
        }]);
        assert!((csum[0] - 1.0).abs() < 1e-9);
    }
}
