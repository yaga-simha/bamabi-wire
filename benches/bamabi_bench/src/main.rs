//! Layer-0 (zero-Bevy, zero-external-dependency) polyglot benchmark host for `bamabi-wire`.
//!
//! Dynamically loads compiled guest `.dylib` / `.so` / `.dll` modules targeting `ABI 0.4.0`
//! (`0x0004_0000`), verifies strict functional correctness against an in-process `native` Rust
//! baseline, measures latency and throughput, and prints an aligned Markdown table plus an
//! optional JSON report.

mod host;
mod loader;
mod workloads;

#[cfg(feature = "hotpath-alloc")]
pub(crate) mod alloc_probe {
    use core::{
        alloc::{GlobalAlloc, Layout},
        cell::Cell,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::{alloc::System, sync::Mutex};

    /// Lightweight allocation counter nested inside [`hotpath::CountingAllocator`] so both
    /// `hotpath` and deterministic steady-state zero-allocation assertions observe heap activity.
    pub(crate) struct BenchAllocCounter;

    static ALLOC_ACTIVE: AtomicBool = AtomicBool::new(false);
    static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
    static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

    thread_local! {
        static IN_MEASURED_THREAD: Cell<bool> = const { Cell::new(false) };
    }

    // SAFETY: Pure pass-through to `std::alloc::System`; only updates atomic counters when
    // `ALLOC_ACTIVE` and `IN_MEASURED_THREAD` are enabled on the calling thread.
    unsafe impl GlobalAlloc for BenchAllocCounter {
        #[inline]
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if ALLOC_ACTIVE.load(Ordering::Relaxed)
                && IN_MEASURED_THREAD.try_with(Cell::get).unwrap_or(false)
            {
                ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
                ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            }
            // SAFETY: Forwards caller's `GlobalAlloc::alloc` contract unchanged to `System`.
            unsafe { System.alloc(layout) }
        }

        #[inline]
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: `ptr` was allocated by `System` with `layout`.
            unsafe { System.dealloc(ptr, layout) }
        }

        #[inline]
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if ALLOC_ACTIVE.load(Ordering::Relaxed)
                && IN_MEASURED_THREAD.try_with(Cell::get).unwrap_or(false)
            {
                ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
                ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            }
            // SAFETY: Forwards caller's `GlobalAlloc::alloc_zeroed` contract unchanged to `System`.
            unsafe { System.alloc_zeroed(layout) }
        }

        #[inline]
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // SAFETY: `ptr` was allocated by `System` with `layout` and `new_size` is valid.
            let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
            if !new_ptr.is_null()
                && ALLOC_ACTIVE.load(Ordering::Relaxed)
                && IN_MEASURED_THREAD.try_with(Cell::get).unwrap_or(false)
            {
                ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
                ALLOC_BYTES.fetch_add(new_size, Ordering::Relaxed);
            }
            new_ptr
        }
    }

    #[global_allocator]
    static ALLOCATOR: hotpath::CountingAllocator<BenchAllocCounter> =
        hotpath::CountingAllocator::with(BenchAllocCounter);

    /// Measures `(alloc_count, alloc_bytes, result)` performed by `f` on the current thread.
    pub(crate) fn measure<R>(f: impl FnOnce() -> R) -> (usize, usize, R) {
        static PROBE_LOCK: Mutex<()> = Mutex::new(());
        struct ResetGuard;
        impl Drop for ResetGuard {
            fn drop(&mut self) {
                ALLOC_ACTIVE.store(false, Ordering::SeqCst);
                let _ = IN_MEASURED_THREAD.try_with(|flag| flag.set(false));
            }
        }

        let _lock = PROBE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        IN_MEASURED_THREAD.with(|flag| flag.set(true));
        ALLOC_COUNT.store(0, Ordering::SeqCst);
        ALLOC_BYTES.store(0, Ordering::SeqCst);
        ALLOC_ACTIVE.store(true, Ordering::SeqCst);
        let _guard = ResetGuard;
        let out = f();
        ALLOC_ACTIVE.store(false, Ordering::SeqCst);
        let count = ALLOC_COUNT.load(Ordering::SeqCst);
        let bytes = ALLOC_BYTES.load(Ordering::SeqCst);
        (count, bytes, out)
    }
}

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
bamabi_bench — Layer-0 C-ABI 0.4.0 polyglot benchmark host

USAGE:
    bamabi_bench [OPTIONS]

OPTIONS:
    --guest <lang>:<profile>=<path>   Guest shared library to verify & benchmark (repeatable)
    --workloads, --workload <csv>     Comma-separated workloads:
                                      noop,dense_query_update,command_burst,host_api_roundtrip,
                                      message_idle_poll,message_burst_256,message_burst_4096
                                      (aliases: message, message_burst)
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
        match trimmed {
            "message" | "message_burst" => {
                for kind in WorkloadKind::ALL_MESSAGES {
                    if !out.contains(&kind) {
                        out.push(kind);
                    }
                }
            }
            other => {
                let kind = WorkloadKind::parse(other)?;
                if !out.contains(&kind) {
                    out.push(kind);
                }
            }
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
            "--workloads" | "--workload" => {
                let val = take_val(inline_val, &mut iter, flag.as_str())?;
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
        "### bamabi-wire ABI 0.4.0 Polyglot Benchmark Report ({mode}, warmup={w}, samples={s})\n\n",
        mode = if report.quick { "quick" } else { "full" },
        w = report.warmup_samples,
        s = report.timed_samples,
    ));

    if !report.guest_manifests.is_empty() {
        out.push_str("#### Verified Guests\n\n");
        for (subject, name, ver) in &report.guest_manifests {
            out.push_str(&format!("- `{subject}`: `{name}` v{ver} (ABI 0.4.0)\n"));
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
    out.push_str("  \"abi_version\": \"0.4.0\",\n");
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

    #[cfg(feature = "hotpath")]
    let _hp_guard = hotpath::HotpathGuardBuilder::new("bamabi_bench")
        .functions_limit(30)
        .build();

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
        BamLayoutInfo, BamSymbol, BamSystem, BamSystemSet, BamTermDesc, ComponentDesc,
        ComponentUuid, FfiBool, FfiEntity, FfiResult, ScheduleId, StrView,
    };

    use super::*;
    use crate::host::{
        BENCH_DT, BENCH_MESSAGE_NAME, BENCH_MESSAGE_UUID, BenchMessage, CmdHeader, HostWorld,
        OwnedHostWorld, POSITION_NAME, POSITION_UUID, Position, SIM_PARAMS_NAME, SIM_PARAMS_UUID,
        SimParams, SystemFrame, VELOCITY_NAME, VELOCITY_UUID, Velocity, initial_position,
        initial_velocity,
    };
    use crate::workloads::{
        GuestBenchMsg, InProcessGuest, compute_stats, native_command_burst, native_dense_update,
        position_checksum,
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

        for (c_name, uuid, expected_handle, expected_align) in [
            (c"bench::Position", POSITION_UUID, world.pos_handle, 4),
            (c"bench::Velocity", VELOCITY_UUID, world.vel_handle, 4),
            (
                c"bench::SimParams",
                SIM_PARAMS_UUID,
                world.sim_params_handle,
                4,
            ),
            (
                c"bench::BenchMessage",
                BENCH_MESSAGE_UUID,
                world.bench_msg_handle,
                8,
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
            assert_eq!(info.align, expected_align);
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
        let _ = (
            POSITION_NAME,
            VELOCITY_NAME,
            SIM_PARAMS_NAME,
            BENCH_MESSAGE_NAME,
        );
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
            let sym_name = StrView::from_str("bench.sym");
            // SAFETY: `host_ctx` is valid and `"bench.sym"` is `'static`.
            let intern_res =
                unsafe { (api.intern_symbol.unwrap())(host_ctx, &raw const sym_name, &mut sym) };
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

        // Default workloads keep the original 4 workloads for xtask compatibility.
        match parse_args(["--quick"]).unwrap() {
            CliAction::Run(cli) => assert_eq!(cli.workloads, WorkloadKind::ALL.to_vec()),
            CliAction::Help => panic!("expected Run"),
        }

        // `--workload` alias and `message` / `message_burst` expansion.
        match parse_args(["--workload", "message"]).unwrap() {
            CliAction::Run(cli) => {
                assert_eq!(
                    cli.workloads,
                    vec![
                        WorkloadKind::MessageIdlePoll,
                        WorkloadKind::MessageBurst256,
                        WorkloadKind::MessageBurst4096,
                    ]
                );
            }
            CliAction::Help => panic!("expected Run"),
        }
        match parse_args(["--workload=noop,message_burst,noop,message_burst_256"]).unwrap() {
            CliAction::Run(cli) => {
                assert_eq!(
                    cli.workloads,
                    vec![
                        WorkloadKind::Noop,
                        WorkloadKind::MessageIdlePoll,
                        WorkloadKind::MessageBurst256,
                        WorkloadKind::MessageBurst4096,
                    ]
                );
            }
            CliAction::Help => panic!("expected Run"),
        }
        match parse_args(["--workloads", "message_idle_poll,message_burst_4096"]).unwrap() {
            CliAction::Run(cli) => {
                assert_eq!(
                    cli.workloads,
                    vec![
                        WorkloadKind::MessageIdlePoll,
                        WorkloadKind::MessageBurst4096,
                    ]
                );
            }
            CliAction::Help => panic!("expected Run"),
        }
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
            "0.4.0".to_owned(),
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
        assert!(json.contains("\"abi_version\": \"0.4.0\""));
        assert!(json.contains("\"verified\": true"));

        let csum = position_checksum(&[Position {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            w: 4.0,
        }]);
        assert!((csum[0] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn command_queue_split_buffer_layout_payloads_and_edge_rollbacks() {
        #[repr(C)]
        struct LargePod {
            bytes: [u8; 64],
        }

        let mut world = HostWorld::new();
        let zst_comp = world.expose_component::<()>(
            "bench::ZstTag",
            ComponentUuid::from_parts("bench::ZstTag", 0, 1, 0),
            0,
            true,
        );
        let large_comp = world.expose_component::<LargePod>(
            "bench::LargePod",
            ComponentUuid::from_parts("bench::LargePod", 64, 1, 0),
            0,
            false,
        );
        let pos_comp = world.pos_handle;

        assert_eq!(core::mem::size_of::<CmdHeader>(), 16);
        assert_eq!(core::mem::align_of::<CmdHeader>(), 4);

        let api = world.api;
        let spawn_empty = api.cmd_spawn_empty.unwrap();
        let spawn = api.cmd_spawn.unwrap();
        let spawn_batch = api.cmd_spawn_batch.unwrap();
        let despawn = api.cmd_despawn.unwrap();
        let insert = api.cmd_insert.unwrap();
        let insert_if_new = api.cmd_insert_if_new.unwrap();
        let insert_default = api.cmd_insert_default.unwrap();
        let remove = api.cmd_remove.unwrap();
        let checkpoint = api.cmd_checkpoint.unwrap();
        let rollback = api.cmd_rollback.unwrap();

        world.reserve_command_capacity(100);
        let reserved_ops_cap = world.commands_mut().ops.capacity();
        let reserved_payload_cap = world.commands_mut().payload_bytes.capacity();
        assert!(reserved_ops_cap >= 100);
        assert!(reserved_payload_cap >= 1600);

        let cmds_of = |w: &mut OwnedHostWorld| {
            core::ptr::from_mut(w.commands_mut()).cast::<bamabi_sys::BamCommands>()
        };
        let mut e1 = FfiEntity::NULL;
        // SAFETY: `cmds` and `e1` are valid outside guest execution.
        let r_spawn_empty = unsafe { spawn_empty(cmds_of(&mut world), &mut e1) };
        assert_eq!(r_spawn_empty, FfiResult::OK);
        assert!(e1.is_valid());

        // 0-byte ZST insert must succeed even with a dangling non-null pointer under Miri.
        let dangling_zst = core::ptr::NonNull::<u8>::dangling().as_ptr();
        // SAFETY: `cmds` is valid and `data_len == 0` must not dereference `dangling_zst`.
        let r_zst = unsafe { insert(cmds_of(&mut world), e1, zst_comp, dangling_zst, 0) };
        assert_eq!(r_zst, FfiResult::OK);
        // SAFETY: `cmds` is valid.
        let r_def = unsafe { insert_default(cmds_of(&mut world), e1, zst_comp) };
        assert_eq!(r_def, FfiResult::OK);

        // 16B POD insert.
        let pos = Position {
            x: 1.5,
            y: 2.5,
            z: 3.5,
            w: 4.5,
        };
        // SAFETY: `cmds` and `pos` are valid for 16 readable bytes.
        let r_pos = unsafe {
            insert(
                cmds_of(&mut world),
                e1,
                pos_comp,
                core::ptr::from_ref(&pos).cast::<u8>(),
                16,
            )
        };
        assert_eq!(r_pos, FfiResult::OK);
        assert_eq!(world.commands_mut().payload_bytes.len(), 16);

        // 0-op no-op rollback at `token == ops.len()` must succeed without reading past `ops`.
        let mut cp_noop = 0usize;
        // SAFETY: `cmds` and `cp_noop` are valid.
        let r_cp_noop = unsafe { checkpoint(cmds_of(&mut world), &mut cp_noop) };
        assert_eq!(r_cp_noop, FfiResult::OK);
        // SAFETY: `cmds` is valid.
        let r_rb_noop = unsafe { rollback(cmds_of(&mut world), cp_noop) };
        assert_eq!(r_rb_noop, FfiResult::OK);
        let noop_stats = world.command_stats();
        assert_eq!(noop_stats.checkpoints, 1);
        assert_eq!(noop_stats.rollbacks, 1);
        assert_eq!(noop_stats.rolled_back_ops, 0);
        assert_eq!(world.commands_mut().payload_bytes.len(), 16);

        // Nested checkpoint + >32B insert + empty-payload ops + rollback.
        let mut cp1 = 0usize;
        // SAFETY: `cmds` and `cp1` are valid.
        let r_cp1 = unsafe { checkpoint(cmds_of(&mut world), &mut cp1) };
        assert_eq!(r_cp1, FfiResult::OK);
        let large = LargePod { bytes: [0xAB; 64] };
        // SAFETY: `cmds` and `large` are valid for 64 readable bytes.
        let r_large = unsafe {
            insert_if_new(
                cmds_of(&mut world),
                e1,
                large_comp,
                core::ptr::from_ref(&large).cast::<u8>(),
                64,
            )
        };
        assert_eq!(r_large, FfiResult::OK);
        assert_eq!(world.commands_mut().payload_bytes.len(), 80);
        assert_eq!(&world.commands_mut().payload_bytes[16..80], &[0xAB; 64]);

        let mut cp2 = 0usize;
        // SAFETY: `cmds` and `cp2` are valid.
        let r_cp2 = unsafe { checkpoint(cmds_of(&mut world), &mut cp2) };
        assert_eq!(r_cp2, FfiResult::OK);
        let mut e2 = FfiEntity::NULL;
        // SAFETY: `cmds` and `e2` are valid.
        let r_e2 = unsafe { spawn_empty(cmds_of(&mut world), &mut e2) };
        assert_eq!(r_e2, FfiResult::OK);
        // SAFETY: `cmds` is valid.
        let r_rem = unsafe { remove(cmds_of(&mut world), e2, pos_comp) };
        assert_eq!(r_rem, FfiResult::OK);
        // SAFETY: `cmds` is valid.
        let r_des = unsafe { despawn(cmds_of(&mut world), e2) };
        assert_eq!(r_des, FfiResult::OK);

        // Rollback to `cp2` drops 3 zero-payload ops, keeping the 64B insert.
        // SAFETY: `cmds` is valid.
        let r_rb2 = unsafe { rollback(cmds_of(&mut world), cp2) };
        assert_eq!(r_rb2, FfiResult::OK);
        assert_eq!(world.commands_mut().payload_bytes.len(), 80);
        let after_cp2 = world.command_stats();
        assert_eq!(after_cp2.last_insert_component, large_comp);
        assert_eq!(after_cp2.last_insert_len, 64);
        assert_eq!(after_cp2.rolled_back_ops, 3);

        // Rollback to `cp1` drops the 64B insert and truncates `payload_bytes` back to 16.
        // SAFETY: `cmds` is valid.
        let r_rb1 = unsafe { rollback(cmds_of(&mut world), cp1) };
        assert_eq!(r_rb1, FfiResult::OK);
        assert_eq!(world.commands_mut().payload_bytes.len(), 16);
        let after_cp1 = world.command_stats();
        assert_eq!(after_cp1.last_insert_component, pos_comp);
        assert_eq!(after_cp1.last_insert_len, 16);
        assert_eq!(after_cp1.rolled_back_ops, 4);

        // Rollback to 0 clears everything.
        // SAFETY: `cmds` is valid.
        let r_rb0 = unsafe { rollback(cmds_of(&mut world), 0) };
        assert_eq!(r_rb0, FfiResult::OK);
        assert!(world.commands_mut().ops.is_empty());
        assert!(world.commands_mut().payload_bytes.is_empty());

        // `clear()` preserves reserved capacities across repeated bursts.
        world.reset_commands();
        assert_eq!(world.commands_mut().ops.capacity(), reserved_ops_cap);
        assert_eq!(
            world.commands_mut().payload_bytes.capacity(),
            reserved_payload_cap
        );

        // SAFETY: `cmds` and `pos` are valid.
        let r_ins = unsafe {
            insert(
                cmds_of(&mut world),
                e1,
                pos_comp,
                core::ptr::from_ref(&pos).cast::<u8>(),
                16,
            )
        };
        assert_eq!(r_ins, FfiResult::OK);
        let inline_comps = [pos_comp, zst_comp];
        let inline_ptrs = [core::ptr::from_ref(&pos).cast::<u8>(), dangling_zst];
        let mut e3 = FfiEntity::NULL;
        // SAFETY: `cmds`, `inline_comps`, `inline_ptrs`, and `e3` are valid.
        let r_spawn = unsafe {
            spawn(
                cmds_of(&mut world),
                inline_comps.as_ptr(),
                inline_ptrs.as_ptr(),
                2,
                &mut e3,
            )
        };
        assert_eq!(r_spawn, FfiResult::OK);
        // SAFETY: `cmds`, `inline_comps`, and `inline_ptrs` are valid for 3 entities.
        let r_batch = unsafe {
            spawn_batch(
                cmds_of(&mut world),
                inline_comps.as_ptr(),
                inline_ptrs.as_ptr(),
                2,
                3,
                core::ptr::null_mut(),
            )
        };
        assert_eq!(r_batch, FfiResult::OK);
        let batch_stats = world.command_stats();
        assert_eq!(batch_stats.committed_spawns, 4);
        assert_eq!(batch_stats.committed_inserts, 1 + 2 + 6);
        assert_eq!(batch_stats.last_insert_component, pos_comp);
        assert_eq!(batch_stats.last_insert_len, 16);

        // FFI validation error paths.
        let cmds = cmds_of(&mut world);
        // SAFETY: Testing explicit null/invalid inputs against FFI guards.
        unsafe {
            assert_eq!(
                insert(core::ptr::null_mut(), e1, pos_comp, core::ptr::null(), 16),
                FfiResult::NULL_POINTER
            );
            assert_eq!(
                insert(cmds, FfiEntity::NULL, pos_comp, core::ptr::null(), 16),
                FfiResult::INVALID_ENTITY
            );
            assert_eq!(
                insert(cmds, e1, BamComponent::INVALID, core::ptr::null(), 0),
                FfiResult::INVALID_COMPONENT
            );
            assert_eq!(
                insert(cmds, e1, pos_comp, core::ptr::null(), 8),
                FfiResult::TYPE_MISMATCH
            );
            assert_eq!(
                insert_if_new(cmds, e1, pos_comp, core::ptr::null(), 16),
                FfiResult::NULL_POINTER
            );
        }
        assert_eq!(
            world.commands_mut().ops.capacity(),
            reserved_ops_cap,
            "operations within reserved capacity must not reallocate ops"
        );
        assert_eq!(
            world.commands_mut().payload_bytes.capacity(),
            reserved_payload_cap,
            "payloads within reserved capacity must not reallocate payload_bytes"
        );
    }

    #[test]
    fn system_frame_layout_arity_and_reentrancy() {
        use core::cell::Cell;
        use core::sync::atomic::{AtomicUsize, Ordering};

        assert_eq!(
            core::mem::size_of::<SystemFrame>(),
            6 * core::mem::size_of::<usize>()
        );
        assert_eq!(
            core::mem::align_of::<SystemFrame>(),
            core::mem::align_of::<usize>()
        );

        struct ReentrantCtx {
            world: Cell<*const OwnedHostWorld>,
            inner_idx: Cell<usize>,
            outer_calls: AtomicUsize,
            inner_calls: AtomicUsize,
        }

        unsafe extern "C" fn zero_query_cb(
            sys_ctx: *mut bamabi_sys::BamSystemCtx,
            user_data: *mut c_void,
        ) -> FfiResult {
            // SAFETY: `user_data` points to `BamHostApi` alive for the test.
            let api = unsafe { &*user_data.cast::<BamHostApi>() };
            // SAFETY: `sys_ctx` is valid for this callback.
            let host_ctx = unsafe { (api.sys_host_ctx.unwrap())(sys_ctx) };
            assert!(!host_ctx.is_null());

            let mut cmds = core::ptr::null_mut();
            // SAFETY: `sys_ctx` and `cmds` are valid.
            let cmds_res = unsafe { (api.sys_commands.unwrap())(sys_ctx, &mut cmds) };
            assert_eq!(cmds_res, FfiResult::OK);
            assert!(!cmds.is_null());

            let mut iter = core::ptr::null_mut();
            // SAFETY: `sys_ctx` and `iter` are valid; query 0 is out of range for 0-query system.
            let q_res = unsafe { (api.sys_query.unwrap())(sys_ctx, 0, &mut iter) };
            assert_eq!(q_res, FfiResult::OUT_OF_RANGE);
            assert!(iter.is_null());

            let bogus_ent = FfiEntity::from_bits(1);
            // SAFETY: Out-of-range query index returns null without dereferencing `iters`.
            assert!(unsafe { (api.sys_entity_get.unwrap())(sys_ctx, 0, bogus_ent, 0) }.is_null());
            // SAFETY: Out-of-range query index returns null without dereferencing `iters`.
            let get_mut_ptr =
                unsafe { (api.sys_entity_get_mut.unwrap())(sys_ctx, 0, bogus_ent, 0) };
            assert!(get_mut_ptr.is_null());
            FfiResult::OK
        }

        unsafe extern "C" fn return_false(_user_data: *mut c_void) -> FfiBool {
            FfiBool::FALSE
        }

        unsafe extern "C" fn panic_if_called(
            _sys_ctx: *mut bamabi_sys::BamSystemCtx,
            _user_data: *mut c_void,
        ) -> FfiResult {
            panic!("run_if returned FALSE; system callback must not run");
        }

        unsafe extern "C" fn dummy_cb(
            _sys_ctx: *mut bamabi_sys::BamSystemCtx,
            _user_data: *mut c_void,
        ) -> FfiResult {
            FfiResult::OK
        }

        unsafe extern "C" fn inner_2q_cb(
            sys_ctx: *mut bamabi_sys::BamSystemCtx,
            user_data: *mut c_void,
        ) -> FfiResult {
            // SAFETY: `user_data` points to `ReentrantCtx` alive for the test.
            let ctx = unsafe { &*user_data.cast::<ReentrantCtx>() };
            // SAFETY: `ctx.world` is valid for the test.
            let world = unsafe { &*ctx.world.get() };
            ctx.inner_calls.fetch_add(1, Ordering::Relaxed);

            let mut iter0 = core::ptr::null_mut();
            let mut iter1 = core::ptr::null_mut();
            let mut iter2 = core::ptr::null_mut();
            // SAFETY: `sys_ctx` has 2 queries.
            unsafe {
                assert_eq!(
                    (world.api.sys_query.unwrap())(sys_ctx, 0, &mut iter0),
                    FfiResult::OK
                );
                assert_eq!(
                    (world.api.sys_query.unwrap())(sys_ctx, 1, &mut iter1),
                    FfiResult::OK
                );
                assert_ne!(iter0, iter1);
                assert_eq!(
                    (world.api.sys_query.unwrap())(sys_ctx, 2, &mut iter2),
                    FfiResult::OUT_OF_RANGE
                );
            }

            let mut count0 = 0usize;
            let mut count1 = 0usize;
            // SAFETY: `iter0` and `iter1` are valid query iterators.
            unsafe {
                assert_eq!(
                    (world.api.iter_count.unwrap())(iter0, &mut count0),
                    FfiResult::OK
                );
                assert_eq!(
                    (world.api.iter_count.unwrap())(iter1, &mut count1),
                    FfiResult::OK
                );
            }
            assert_eq!(count0, 300);
            assert_eq!(count1, 300);

            let mut len = 0usize;
            let mut seen_entities = 0usize;
            let mut first_entity = FfiEntity::NULL;
            // SAFETY: `iter1` is a valid query iterator.
            while unsafe { (world.api.iter_next_chunk.unwrap())(iter1, &mut len) } == FfiBool::TRUE
            {
                // SAFETY: Active chunk has valid entities slice of length `len`.
                let ents = unsafe { (world.api.iter_entities.unwrap())(iter1) };
                assert!(!ents.is_null());
                if first_entity.is_null() && len > 0 {
                    // SAFETY: `ents` points to `len` readable entities.
                    first_entity = unsafe { *ents };
                }
                seen_entities += len;
            }
            assert_eq!(seen_entities, 300);

            // SAFETY: `first_entity` is a valid entity in query 1 (`Read(Position)`).
            let pos_ptr =
                unsafe { (world.api.sys_entity_get.unwrap())(sys_ctx, 1, first_entity, 0) };
            assert!(!pos_ptr.is_null());
            FfiResult::OK
        }

        unsafe extern "C" fn outer_reentrant_cb(
            sys_ctx: *mut bamabi_sys::BamSystemCtx,
            user_data: *mut c_void,
        ) -> FfiResult {
            // SAFETY: `user_data` points to `ReentrantCtx` alive for the test.
            let ctx = unsafe { &*user_data.cast::<ReentrantCtx>() };
            // SAFETY: `ctx.world` is valid for the test.
            let world = unsafe { &*ctx.world.get() };
            ctx.outer_calls.fetch_add(1, Ordering::Relaxed);

            let mut iter0 = core::ptr::null_mut();
            let mut iter1 = core::ptr::null_mut();
            // SAFETY: `sys_ctx` has 2 queries.
            unsafe {
                assert_eq!(
                    (world.api.sys_query.unwrap())(sys_ctx, 0, &mut iter0),
                    FfiResult::OK
                );
                assert_eq!(
                    (world.api.sys_query.unwrap())(sys_ctx, 1, &mut iter1),
                    FfiResult::OK
                );
                assert_eq!(
                    (world.api.sys_query.unwrap())(sys_ctx, 2, &mut iter1),
                    FfiResult::OUT_OF_RANGE
                );
            }

            // Advance outer query 0 by 1 chunk, test `sys_entity_get_mut` on query 0, then call `inner_2q_cb` reentrantly.
            let mut len0 = 0usize;
            // SAFETY: `iter0` is valid.
            let next_ok = unsafe { (world.api.iter_next_chunk.unwrap())(iter0, &mut len0) };
            assert_eq!(next_ok, FfiBool::TRUE);
            // SAFETY: Active chunk has valid entities slice.
            let ents = unsafe { (world.api.iter_entities.unwrap())(iter0) };
            // SAFETY: `ents` has `len0 >= 1` entities.
            let ent0 = unsafe { *ents };
            // SAFETY: `ent0` has mutable `Position` at term 0 of query 0.
            let mut_pos_ptr =
                unsafe { (world.api.sys_entity_get_mut.unwrap())(sys_ctx, 0, ent0, 0) };
            assert!(!mut_pos_ptr.is_null());

            // Reentrant `run_system` on inner 2-query system while outer `SystemFrame` is active.
            assert_eq!(world.run_system(ctx.inner_idx.get()), FfiResult::OK);

            // Outer iterator state must remain intact across reentrant call.
            let mut rem_chunks = 0usize;
            let mut rem_entities = 0usize;
            // SAFETY: `iter0` is still valid after inner `run_system`.
            while unsafe { (world.api.iter_next_chunk.unwrap())(iter0, &mut len0) } == FfiBool::TRUE
            {
                rem_chunks += 1;
                rem_entities += len0;
            }
            assert_eq!(rem_chunks, 2);
            assert_eq!(rem_entities, 300 - 128);
            FfiResult::OK
        }

        let mut world = HostWorld::new();
        world.setup_dense_entities(300, 128);
        let api_ptr = world.api_ptr().cast_mut().cast::<c_void>();
        let reg_sys = world.api.register_system.unwrap();
        let reg_sys_handle = world.api.register_system_handle.unwrap();
        let mount_sys = world.api.mount_system_in_schedule.unwrap();

        // 0-query system.
        let zero_desc = bamabi_sys::BamSystemDesc::new(
            c"zero_q".as_ptr(),
            core::ptr::null(),
            0,
            zero_query_cb,
            api_ptr,
        );
        // SAFETY: `load_ctx_ptr` and `zero_desc` are valid during init.
        let r_zero = unsafe { reg_sys(world.load_ctx_ptr(), &zero_desc) };
        assert_eq!(r_zero, FfiResult::OK);
        let zero_idx = world.find_system("zero_q").unwrap();
        assert_eq!(world.run_system(zero_idx), FfiResult::OK);

        // `run_if` returning FALSE skips callback.
        let q_pos_terms = [BamTermDesc::read(world.pos_handle)];
        let q_pos = [bamabi_sys::BamQueryDesc::new(
            q_pos_terms.as_ptr(),
            q_pos_terms.len(),
        )];
        let mut skipped_desc = bamabi_sys::BamSystemDesc::new(
            c"skipped_sys".as_ptr(),
            q_pos.as_ptr(),
            q_pos.len(),
            panic_if_called,
            core::ptr::null_mut(),
        );
        skipped_desc.run_if = Some(return_false);
        // SAFETY: `load_ctx_ptr` and `skipped_desc` are valid during init.
        let r_skip = unsafe { reg_sys(world.load_ctx_ptr(), &skipped_desc) };
        assert_eq!(r_skip, FfiResult::OK);
        let skipped_idx = world.find_system("skipped_sys").unwrap();
        assert_eq!(world.run_system(skipped_idx), FfiResult::OK);

        // Inner 2-query system registered via `register_system_handle` and mounted via `mount_system_in_schedule`.
        let q0_terms = [
            BamTermDesc::write(world.pos_handle),
            BamTermDesc::read(world.vel_handle),
        ];
        let q1_terms = [BamTermDesc::read(world.pos_handle)];
        let two_queries = [
            bamabi_sys::BamQueryDesc::new(q0_terms.as_ptr(), q0_terms.len()),
            bamabi_sys::BamQueryDesc::new(q1_terms.as_ptr(), q1_terms.len()),
        ];

        let ctx = ReentrantCtx {
            world: Cell::new(core::ptr::null()),
            inner_idx: Cell::new(0),
            outer_calls: AtomicUsize::new(0),
            inner_calls: AtomicUsize::new(0),
        };
        let ctx_ptr = core::ptr::from_ref(&ctx).cast_mut().cast::<c_void>();

        let inner_desc = bamabi_sys::BamSystemDesc::new(
            c"inner_2q".as_ptr(),
            two_queries.as_ptr(),
            two_queries.len(),
            inner_2q_cb,
            ctx_ptr,
        );
        let mut inner_handle = BamSystem::INVALID;
        // SAFETY: `load_ctx_ptr`, `inner_desc`, and `inner_handle` are valid during init.
        let r_inner =
            unsafe { reg_sys_handle(world.load_ctx_ptr(), &inner_desc, &mut inner_handle) };
        assert_eq!(r_inner, FfiResult::OK);
        // SAFETY: `load_ctx_ptr` is valid during init; mutating `&mut RegisteredSystem` must remain Miri-sound.
        let r_mount = unsafe {
            mount_sys(
                world.load_ctx_ptr(),
                inner_handle,
                ScheduleId::FIXED_UPDATE,
                BamSystemSet::INVALID,
            )
        };
        assert_eq!(r_mount, FfiResult::OK);
        let inner_idx = world.find_system("inner_2q").unwrap();
        ctx.inner_idx.set(inner_idx);

        // Register 8 more systems to force `HostWorld::systems` `Vec` reallocation after mounting `inner_2q`.
        for i in 0..8 {
            let name = format!("dummy_sys_{i}\0");
            let dummy_desc = bamabi_sys::BamSystemDesc::new(
                name.as_ptr().cast(),
                core::ptr::null(),
                0,
                dummy_cb,
                core::ptr::null_mut(),
            );
            // SAFETY: `load_ctx_ptr` and `dummy_desc` are valid during init.
            let r_dummy = unsafe { reg_sys(world.load_ctx_ptr(), &dummy_desc) };
            assert_eq!(r_dummy, FfiResult::OK);
        }

        let outer_desc = bamabi_sys::BamSystemDesc::new(
            c"outer_2q".as_ptr(),
            two_queries.as_ptr(),
            two_queries.len(),
            outer_reentrant_cb,
            ctx_ptr,
        );
        // SAFETY: `load_ctx_ptr` and `outer_desc` are valid during init.
        let r_outer = unsafe { reg_sys(world.load_ctx_ptr(), &outer_desc) };
        assert_eq!(r_outer, FfiResult::OK);
        let outer_idx = world.find_system("outer_2q").unwrap();
        ctx.world.set(core::ptr::from_ref(&world));

        assert_eq!(world.run_system(outer_idx), FfiResult::OK);
        assert_eq!(world.run_system(outer_idx), FfiResult::OK);
        assert_eq!(ctx.outer_calls.load(Ordering::Relaxed), 2);
        assert_eq!(ctx.inner_calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn symbol_table_intern_dedup_concurrency_and_utf8_validation() {
        let world = HostWorld::new();
        let s0 = world.shared.intern_symbol_str("a");
        let s1 = world.shared.intern_symbol_str("b");
        let s2 = world.shared.intern_symbol_str("bench.sym");
        let s3 = world.shared.intern_symbol_str("αβγ::λ");
        assert_eq!(s0.to_raw(), 0);
        assert_eq!(s1.to_raw(), 1);
        assert_eq!(s2.to_raw(), 2);
        assert_eq!(s3.to_raw(), 3);
        assert_eq!(world.shared.intern_symbol_str("bench.sym"), s2);
        assert_eq!(world.shared.intern_symbol_str("αβγ::λ"), s3);
        assert_eq!(world.intern_symbol_calls(), 6);

        world.reset_intern_symbol_calls();
        let shared = &world.shared;
        let thread_results = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for tid in 0..4usize {
                handles.push(scope.spawn(move || {
                    let mut out = Vec::new();
                    for round in 0..8usize {
                        let a = shared.intern_symbol_str("bench.sym");
                        let b = shared.intern_symbol_str("αβγ::λ");
                        let shared_new = shared.intern_symbol_str("concurrent.shared");
                        let per_thread = shared.intern_symbol_str(&format!("worker_{tid}_{round}"));
                        out.push((a, b, shared_new, per_thread));
                    }
                    out
                }));
            }
            handles
                .into_iter()
                .map(|h| h.join().expect("thread must not panic"))
                .collect::<Vec<_>>()
        });
        assert_eq!(world.intern_symbol_calls(), 4 * 8 * 4);
        let expected_shared_new = thread_results[0][0].2;
        for per_t in &thread_results {
            for &(a, b, shared_new, per_thread) in per_t {
                assert_eq!(a, s2);
                assert_eq!(b, s3);
                assert_eq!(shared_new, expected_shared_new);
                assert!(per_thread.is_valid());
            }
        }

        // FFI error validation checks must not increment `intern_symbol_calls`.
        world.reset_intern_symbol_calls();
        let intern_fn = world.api.intern_symbol.unwrap();
        let host_ctx = world.host_ctx_ptr();
        let mut sym_out = BamSymbol::INVALID;
        let invalid_utf8 = [0xffu8, 0xfeu8];
        // SAFETY: Testing explicit invalid arguments and pointers against FFI guards.
        unsafe {
            let x_str = StrView::from_str("x");
            let empty_str = StrView::from_str("");
            let null_empty_str = StrView {
                ptr: core::ptr::null(),
                len: 0,
            };
            let null_nonempty_str = StrView {
                ptr: core::ptr::null(),
                len: 4,
            };
            let bad_utf8_str = StrView {
                ptr: invalid_utf8.as_ptr(),
                len: invalid_utf8.len(),
            };
            assert_eq!(
                intern_fn(core::ptr::null(), &raw const x_str, &mut sym_out),
                FfiResult::NULL_POINTER
            );
            assert_eq!(
                intern_fn(host_ctx, &raw const x_str, core::ptr::null_mut()),
                FfiResult::NULL_POINTER
            );
            assert_eq!(
                intern_fn(host_ctx, &raw const empty_str, &mut sym_out),
                FfiResult::INVALID_ARGUMENT
            );
            assert_eq!(
                intern_fn(host_ctx, &raw const null_empty_str, &mut sym_out),
                FfiResult::INVALID_ARGUMENT
            );
            assert_eq!(
                intern_fn(host_ctx, &raw const null_nonempty_str, &mut sym_out),
                FfiResult::NULL_POINTER
            );
            assert_eq!(
                intern_fn(host_ctx, &raw const bad_utf8_str, &mut sym_out),
                FfiResult::INVALID_UTF8
            );
        }
        assert_eq!(world.intern_symbol_calls(), 0);
    }

    #[test]
    fn stateful_slice_cursor_conservation_across_sizes_and_batches() {
        struct DrainProbeCtx {
            api: *const BamHostApi,
            max_count: usize,
            observed_seqs: Vec<u64>,
            zero_read_confirmed: bool,
        }

        unsafe extern "C" fn drain_probe_cb(
            sys_ctx: *mut bamabi_sys::BamSystemCtx,
            user_data: *mut c_void,
        ) -> FfiResult {
            // SAFETY: `user_data` points to `DrainProbeCtx` alive for this test.
            let probe = unsafe { &mut *user_data.cast::<DrainProbeCtx>() };
            // SAFETY: `probe.api` points to a validated `BamHostApi`.
            let api = unsafe { &*probe.api };
            let mut iter = core::ptr::null_mut();
            // SAFETY: `sys_ctx` has query 0 (`MESSAGE_READ` of `BenchMessage`).
            let q_res = unsafe { (api.sys_query.unwrap())(sys_ctx, 0, &mut iter) };
            assert_eq!(q_res, FfiResult::OK);

            // `max_count == 0` must read 0 items and must not advance cursor.
            let mut zero_read = 99usize;
            // SAFETY: `iter` and `zero_read` are valid; `max_count == 0` never reads `out_buf`.
            let z_res = unsafe {
                (api.msg_read_batch.unwrap())(iter, 0, core::ptr::null_mut(), 0, &mut zero_read)
            };
            assert_eq!(z_res, FfiResult::OK);
            assert_eq!(zero_read, 0);

            probe.observed_seqs.clear();
            let mut buf = vec![BenchMessage::default(); probe.max_count];
            loop {
                let mut read = 0usize;
                // SAFETY: `iter` is query 0, `buf` has `probe.max_count * 16` writable bytes, and `read` is valid.
                let r_res = unsafe {
                    (api.msg_read_batch.unwrap())(
                        iter,
                        0,
                        buf.as_mut_ptr().cast::<u8>(),
                        probe.max_count,
                        &mut read,
                    )
                };
                assert_eq!(r_res, FfiResult::OK);
                for msg in &buf[..read] {
                    assert_eq!(msg.payload, (msg.seq as u32).wrapping_mul(3));
                    assert_eq!(msg.flags, 1);
                    probe.observed_seqs.push(msg.seq);
                }
                if read == 0 {
                    probe.zero_read_confirmed = true;
                    break;
                }
            }
            FfiResult::OK
        }

        const COUNTS: [usize; 9] = [0, 1, 63, 64, 65, 128, 129, 256, 4096];
        const BATCHES: [usize; 6] = [1, 7, 32, 64, 65, 256];

        let mut world = HostWorld::new();
        let mut probe = DrainProbeCtx {
            api: world.api_ptr(),
            max_count: 1,
            observed_seqs: Vec::new(),
            zero_read_confirmed: false,
        };
        let terms = [BamTermDesc::message_read(world.bench_msg_handle)];
        let queries = [bamabi_sys::BamQueryDesc::new(terms.as_ptr(), terms.len())];
        let desc = bamabi_sys::BamSystemDesc::new(
            c"cursor_probe".as_ptr(),
            queries.as_ptr(),
            queries.len(),
            drain_probe_cb,
            core::ptr::from_mut(&mut probe).cast::<c_void>(),
        );
        // SAFETY: `load_ctx_ptr()` and `desc` are valid during setup.
        let reg_res = unsafe { (world.api.register_system.unwrap())(world.load_ctx_ptr(), &desc) };
        assert_eq!(reg_res, FfiResult::OK);
        let sys_idx = world.find_system("cursor_probe").unwrap();

        for &n in &COUNTS {
            world.enqueue_bench_messages(n);

            // Direct `StatefulSliceCursor` conservation check (take + trailing None).
            for &max_count in &BATCHES {
                let mut cursor = 0usize;
                let mut direct_seqs = Vec::with_capacity(n);
                let queue = world.bench_message_queue().unwrap();
                loop {
                    let mut batch_count = 0usize;
                    for raw in queue.cursor(&mut cursor).take(max_count) {
                        assert_eq!(raw.len(), 16);
                        let mut msg = core::mem::MaybeUninit::<BenchMessage>::uninit();
                        // SAFETY: `msg` has 16 writable bytes and `raw` has 16 initialized bytes.
                        let parsed = unsafe {
                            core::ptr::copy_nonoverlapping(
                                raw.as_ptr(),
                                (&raw mut msg).cast::<core::mem::MaybeUninit<u8>>(),
                                16,
                            );
                            msg.assume_init()
                        };
                        direct_seqs.push(parsed.seq);
                        batch_count += 1;
                    }
                    if batch_count == 0 {
                        break;
                    }
                }
                assert_eq!(cursor, n);
                assert_eq!(direct_seqs.len(), n);
                assert!(direct_seqs.iter().enumerate().all(|(i, &s)| s == i as u64));

                // C-ABI `host_msg_read_batch` conservation check + rewind.
                world.rewind_msg_cursors();
                probe.max_count = max_count;
                probe.zero_read_confirmed = false;
                assert_eq!(world.run_system(sys_idx), FfiResult::OK);
                assert!(probe.zero_read_confirmed);
                assert_eq!(probe.observed_seqs.len(), n);
                assert!(
                    probe
                        .observed_seqs
                        .iter()
                        .enumerate()
                        .all(|(i, &s)| s == i as u64)
                );
            }
        }

        // `clear_bench_messages` keeps data capacity and resets cursors.
        let cap_before = world.bench_message_queue().unwrap().data.capacity();
        assert!(cap_before >= 4096 * 16);
        world.clear_bench_messages();
        let q_after = world.bench_message_queue().unwrap();
        assert_eq!(q_after.len, 0);
        assert_eq!(q_after.data.capacity(), cap_before);
    }

    #[test]
    fn run_if_ctx_cursor_isolation_and_gating() {
        use core::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;

        use bamabi_guest::prelude::conditions;

        // 1. Safe Rust SDK: `on_message` (`query_idx = usize::MAX`) must not consume body cursor (`query_idx = 0`).
        {
            let mut world = HostWorld::new();
            let body_runs = Arc::new(AtomicU64::new(0));
            let body_sum = Arc::new(AtomicU64::new(0));
            let runs_clone = Arc::clone(&body_runs);
            let sum_clone = Arc::clone(&body_sum);

            let guest = InProcessGuest::load(c"test_run_if_isolation", &mut world, |app| {
                app.system(c"gated_reader")
                    .schedule(ScheduleId::UPDATE)
                    .run_if_condition(conditions::on_message::<GuestBenchMsg>())
                    .query(|q| q.message_read::<GuestBenchMsg>())
                    .build(move |ctx| {
                        runs_clone.fetch_add(1, Ordering::Relaxed);
                        let msgs = ctx.read_messages::<GuestBenchMsg>(0)?;
                        let s: u64 = msgs.iter().map(|m| m.seq).sum();
                        sum_clone.store(s, Ordering::Relaxed);
                        Ok(())
                    })
            })
            .unwrap();
            let sys_idx = world.find_system("gated_reader").unwrap();

            // Empty queue -> `on_message` skips body.
            assert_eq!(world.run_system(sys_idx), FfiResult::OK);
            assert_eq!(body_runs.load(Ordering::Relaxed), 0);

            // Queue 10 messages (sum 0..10 == 45) -> `on_message` passes, body reads all 10.
            world.enqueue_bench_messages(10);
            assert_eq!(world.run_system(sys_idx), FfiResult::OK);
            assert_eq!(body_runs.load(Ordering::Relaxed), 1);
            assert_eq!(body_sum.load(Ordering::Relaxed), 45);
            // SAFETY: Reading `msg_cursors` outside guest callbacks.
            let cursors = unsafe { &*world.msg_cursors.get() };
            assert_eq!(cursors.get(&(sys_idx, usize::MAX, 0)), Some(&10));
            assert_eq!(cursors.get(&(sys_idx, 0, 0)), Some(&10));

            // Second tick without rewind -> `on_message` sees 0 unread and skips body.
            assert_eq!(world.run_system(sys_idx), FfiResult::OK);
            assert_eq!(body_runs.load(Ordering::Relaxed), 1);

            // Rewind resets both `usize::MAX` and `0` cursors -> third tick runs and reads 45 again.
            world.rewind_msg_cursors();
            body_sum.store(0, Ordering::Relaxed);
            assert_eq!(world.run_system(sys_idx), FfiResult::OK);
            assert_eq!(body_runs.load(Ordering::Relaxed), 2);
            assert_eq!(body_sum.load(Ordering::Relaxed), 45);

            guest.shutdown(&mut world).unwrap();
        }

        // 2. Raw C-ABI `run_if_ctx` returning non-OK skips body, OK runs body with separate cursor.
        {
            struct RawCondCtx {
                api: *const BamHostApi,
                cond_read: usize,
                body_read: usize,
                allow_run: bool,
            }

            unsafe extern "C" fn raw_cond_cb(
                sys_ctx: *mut bamabi_sys::BamSystemCtx,
                user_data: *mut c_void,
            ) -> FfiResult {
                // SAFETY: `user_data` points to `RawCondCtx` alive for this test.
                let ctx = unsafe { &mut *user_data.cast::<RawCondCtx>() };
                if !ctx.allow_run {
                    return FfiResult::NOT_FOUND;
                }
                // SAFETY: `ctx.api` points to a valid `BamHostApi`.
                let api = unsafe { &*ctx.api };
                let mut iter = core::ptr::null_mut();
                // SAFETY: `run_if_query` provides query slot 0.
                let q_res = unsafe { (api.sys_query.unwrap())(sys_ctx, 0, &mut iter) };
                assert_eq!(q_res, FfiResult::OK);
                let mut buf = [BenchMessage::default(); 3];
                let mut read = 0usize;
                // SAFETY: Reading up to 3 messages in `run_if_ctx` advances only `query_idx = usize::MAX`.
                let r_res = unsafe {
                    (api.msg_read_batch.unwrap())(
                        iter,
                        0,
                        buf.as_mut_ptr().cast::<u8>(),
                        3,
                        &mut read,
                    )
                };
                assert_eq!(r_res, FfiResult::OK);
                ctx.cond_read = read;
                FfiResult::OK
            }

            unsafe extern "C" fn raw_body_cb(
                sys_ctx: *mut bamabi_sys::BamSystemCtx,
                user_data: *mut c_void,
            ) -> FfiResult {
                // SAFETY: `user_data` points to `RawCondCtx` alive for this test.
                let ctx = unsafe { &mut *user_data.cast::<RawCondCtx>() };
                // SAFETY: `ctx.api` points to a valid `BamHostApi`.
                let api = unsafe { &*ctx.api };
                let mut iter = core::ptr::null_mut();
                // SAFETY: `sys_ctx` has query 0.
                let q_res = unsafe { (api.sys_query.unwrap())(sys_ctx, 0, &mut iter) };
                assert_eq!(q_res, FfiResult::OK);
                let mut buf = [BenchMessage::default(); 16];
                let mut read = 0usize;
                // SAFETY: Body reads all queued messages independently of `run_if_ctx` cursor.
                let r_res = unsafe {
                    (api.msg_read_batch.unwrap())(
                        iter,
                        0,
                        buf.as_mut_ptr().cast::<u8>(),
                        16,
                        &mut read,
                    )
                };
                assert_eq!(r_res, FfiResult::OK);
                ctx.body_read = read;
                FfiResult::OK
            }

            let mut world = HostWorld::new();
            world.enqueue_bench_messages(7);
            let mut state = RawCondCtx {
                api: world.api_ptr(),
                cond_read: 0,
                body_read: 0,
                allow_run: false,
            };
            let state_ptr = core::ptr::from_mut(&mut state).cast::<c_void>();
            let terms = [BamTermDesc::message_read(world.bench_msg_handle)];
            let body_q = [bamabi_sys::BamQueryDesc::new(terms.as_ptr(), terms.len())];
            let cond_q = bamabi_sys::BamQueryDesc::new(terms.as_ptr(), terms.len());
            let desc = bamabi_sys::BamSystemDesc::new(
                c"raw_cond_sys".as_ptr(),
                body_q.as_ptr(),
                body_q.len(),
                raw_body_cb,
                state_ptr,
            )
            .with_run_if_ctx(raw_cond_cb, &raw const cond_q, state_ptr);

            // SAFETY: `load_ctx_ptr()` and `desc` are valid during registration.
            let reg_res =
                unsafe { (world.api.register_system.unwrap())(world.load_ctx_ptr(), &desc) };
            assert_eq!(reg_res, FfiResult::OK);
            let idx = world.find_system("raw_cond_sys").unwrap();

            assert_eq!(world.run_system(idx), FfiResult::OK);
            assert_eq!(state.body_read, 0);

            state.allow_run = true;
            assert_eq!(world.run_system(idx), FfiResult::OK);
            assert_eq!(state.cond_read, 3);
            assert_eq!(state.body_read, 7);
            // SAFETY: Reading `msg_cursors` outside guest callbacks.
            let cursors = unsafe { &*world.msg_cursors.get() };
            assert_eq!(cursors.get(&(idx, usize::MAX, 0)), Some(&3));
            assert_eq!(cursors.get(&(idx, 0, 0)), Some(&7));
        }
    }

    #[test]
    fn message_workloads_run_verify_and_render_cleanly() {
        let config = BenchConfig {
            guests: Vec::new(),
            workloads: WorkloadKind::ALL_MESSAGES.to_vec(),
            entities: vec![256],
            chunks: vec![64],
            quick: true,
            warmup_samples: 1,
            timed_samples: 3,
        };
        let report = run_benchmarks(&config).expect("message workloads must succeed");
        assert_eq!(report.rows.len(), 12);
        assert!(report.rows.iter().all(|r| r.verified));
        for workload in WorkloadKind::ALL_MESSAGES {
            for subj in [
                "native",
                "rust_sdk:drain_messages",
                "rust_sdk:reused_vec",
                "c_abi:raw_batch",
            ] {
                assert!(
                    report
                        .rows
                        .iter()
                        .any(|r| r.workload == workload && r.subject == subj),
                    "missing row for {} / {}",
                    workload.as_str(),
                    subj
                );
            }
        }
        let md = render_markdown_report(&report);
        assert!(md.contains("message_idle_poll"));
        assert!(md.contains("message_burst_256"));
        assert!(md.contains("message_burst_4096"));
        assert!(md.contains("rust_sdk:reused_vec"));
        let json = render_json_report(&report);
        assert!(json.contains("\"workload\": \"message_burst_4096\""));
    }
}
