#![forbid(unsafe_code)]

//! Workspace build, verification, header generation, and QA automation (`cargo xtask`).
//!
//! Zero-external-dependency task runner (aside from workspace crates [`bamabi_macros`] and
//! [`bamabi_sdk_gen`]) covering formatting, safety/isolation lints, Rust + C99 test suites,
//! header byte-parity checks, static audits, Miri (Tree & Stacked Borrows + big-endian `s390x`),
//! Kani formal verification, and nightly `-Zrandomize-layout` runs.
//!
//! # Architecture Overview
//!
//! Execution flows through three small stages inside this binary:
//!
//! 1. **CLI parsing (`parse_args`)**: Reads `std::env::args().skip(1)`, handles top-level and
//!    per-subcommand `--help` / `-h` / `help <subcommand>`, and validates flags into a strongly
//!    typed `CommandSpec` (or returns a descriptive usage error before spawning any subprocess).
//! 2. **Dispatch (`dispatch`)**: Resolves the repository root from `CARGO_MANIFEST_DIR`
//!    (`workspace_root`) and routes each `CommandSpec` variant to its dedicated `cmd_*` runner
//!    (or into `component_headers::write_or_check_component_headers`).
//! 3. **Verification & codegen runners (`cmd_*`)**: Invoke `cargo`, `cc`/`clang`, `typos`,
//!    `cargo deny`, `cargo semver-checks`, `cargo +nightly miri`, and `cargo kani` with explicit
//!    environment hygiene (`clean_toolchain_cargo_cmd` clears `RUSTUP_TOOLCHAIN` and `RUSTC`
//!    overrides so `+nightly` and tool subcommands select the right toolchain).
//!
//! # Component Header Extraction (`component_headers`)
//!
//! The `component_headers` module defines canonical `#[derive(ModComponent)]` testbed structs
//! (mirroring `testbed::*` and `c_interop::*` fixtures) and feeds `bamabi_sdk_gen::section_for::<T>()`
//! (which reads `ModComponent::C_DECL`) into `bamabi_sdk_gen::render_component_header` to
//! produce or byte-check `include/generated/testbed_components.h`.
//!
//! # Running & Extending Subcommands
//!
//! Invoke via the `.cargo/config.toml` alias from the repository root:
//!
//! ```text
//! cargo xtask ci          # fmt, header --check, gen-component-headers --check, lint, test, c-test, bench --quick
//! cargo xtask ci --full   # adds audit, dual-model Miri, and Kani formal proofs
//! cargo xtask --help      # lists all 12 subcommands
//! ```
//!
//! To add a new subcommand:
//!
//! 1. Add variants to `SubcommandKind` and `CommandSpec` (plus an `*Args` struct if needed).
//! 2. Wire the string name in `parse_subcommand_kind`, its flag parser in `parse_args`, and its
//!    usage text in `print_help`.
//! 3. Implement `cmd_<name>(root: &Path, ...)` and route to it in `dispatch`.
//! 4. Add parser unit tests in the `tests` module verifying both valid flag combinations and
//!    rejected/conflicting flags.
//!
//! # Workspace & Crate Documentation
//!
//! - [`docs::architecture`]: Deep dive into `xtask` CLI parsing, toolchain isolation, and all 12
//!   subcommand runners (`ARCHITECTURE.md`).
//! - [`docs::tests`]: Unit test inventory covering CLI parsing and checked-in component-header
//!   parity (`TESTS.md`).
//! - [Workspace Documentation Hub (`bamabi_docs`)](../bamabi_docs/index.html)
//!   ([`overview`](../bamabi_docs/overview/index.html),
//!   [`architecture`](../bamabi_docs/architecture/index.html),
//!   [`development`](../bamabi_docs/development/index.html),
//!   [`testing`](../bamabi_docs/testing/index.html),
//!   [`benchmarks`](../bamabi_docs/benchmarks/index.html),
//!   [`wire_spec`](../bamabi_docs/wire_spec/index.html),
//!   [`crates`](../bamabi_docs/crates/index.html)):
//!   Root `README.md`, `ARCHITECTURE.md` (with the 6-crate end-to-end walkthrough),
//!   `DEVELOPMENT.md`, `TESTING.md`, `BENCHMARKS.md`, and `spec.md` rendered directly inside
//!   `cargo docs --open` (landing on `target/doc/bamabi_docs/index.html`).

#[cfg(doc)]
#[allow(unreachable_pub)]
pub mod docs {
    //! Embedded `xtask` architecture and verification documentation.

    /// Embedded `xtask/ARCHITECTURE.md`.
    pub mod architecture {
        #![doc = include_str!("../ARCHITECTURE.md")]
    }

    /// Embedded `xtask/TESTS.md`.
    pub mod tests {
        #![doc = include_str!("../TESTS.md")]
    }
}

mod component_headers;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

/// Top-level parsed CLI outcome before execution.
#[derive(Debug, PartialEq, Eq)]
enum ParsedArgs {
    /// Print general or per-subcommand usage and exit 0.
    Help(Option<SubcommandKind>),
    /// Execute a concrete subcommand.
    Run(CommandSpec),
}

/// Supported `cargo xtask` subcommand names (used for targeted `--help`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubcommandKind {
    Fmt,
    Lint,
    Test,
    CTest,
    Header,
    GenComponentHeaders,
    Audit,
    Miri,
    Kani,
    Nightly,
    Bench,
    Ci,
}

/// Concrete subcommand and validated options.
#[derive(Debug, PartialEq, Eq)]
enum CommandSpec {
    Fmt(FmtArgs),
    Lint,
    Test(TestArgs),
    CTest(CTestArgs),
    Header(CheckArgs),
    GenComponentHeaders(CheckArgs),
    Audit(AuditArgs),
    Miri(MiriArgs),
    Kani(KaniArgs),
    Nightly(NightlyArgs),
    Bench(BenchArgs),
    Ci(CiArgs),
}

/// Options for `cargo xtask fmt`.
#[derive(Debug, Default, PartialEq, Eq)]
struct FmtArgs {
    fix: bool,
}

/// Options for `cargo xtask test`.
#[derive(Debug, Default, PartialEq, Eq)]
struct TestArgs {
    cases: Option<usize>,
    no_default_features: bool,
    all_features: bool,
    doc: bool,
    proptest: bool,
    header_parity: bool,
}

impl TestArgs {
    /// Returns `true` when no explicit suite filter flags were requested.
    fn run_all_default(&self) -> bool {
        !self.no_default_features
            && !self.all_features
            && !self.doc
            && !self.proptest
            && !self.header_parity
    }
}

/// Options for `cargo xtask c-test`.
#[derive(Debug, Default, PartialEq, Eq)]
struct CTestArgs {
    no_asan: bool,
    asan_only: bool,
}

/// Options for `cargo xtask header` and `cargo xtask gen-component-headers`.
#[derive(Debug, Default, PartialEq, Eq)]
struct CheckArgs {
    check: bool,
}

/// Options for `cargo xtask audit`.
#[derive(Debug, Default, PartialEq, Eq)]
struct AuditArgs {
    baseline_rev: Option<String>,
    skip_targets: bool,
    skip_typos: bool,
}

/// Options for `cargo xtask miri`.
#[derive(Debug, Default, PartialEq, Eq)]
struct MiriArgs {
    tree_borrows: bool,
    stacked_borrows: bool,
    s390x: bool,
}

impl MiriArgs {
    /// Returns effective `(tree_borrows, stacked_borrows, s390x)` flags with default `--all`.
    fn resolved(&self) -> (bool, bool, bool) {
        if !self.tree_borrows && !self.stacked_borrows && !self.s390x {
            (true, true, false)
        } else {
            (self.tree_borrows, self.stacked_borrows, self.s390x)
        }
    }
}

/// Options for `cargo xtask kani`.
#[derive(Debug, Default, PartialEq, Eq)]
struct KaniArgs {
    harness: Option<String>,
    packages: Vec<String>,
}

/// Options for `cargo xtask nightly`.
#[derive(Debug, PartialEq, Eq)]
struct NightlyArgs {
    cases: usize,
    s390x: bool,
    randomize_layout: bool,
}

impl Default for NightlyArgs {
    fn default() -> Self {
        Self {
            cases: 2048,
            s390x: false,
            randomize_layout: false,
        }
    }
}

impl NightlyArgs {
    /// Returns effective `(s390x, randomize_layout)` flags, defaulting to both.
    fn resolved(&self) -> (bool, bool) {
        if !self.s390x && !self.randomize_layout {
            (true, true)
        } else {
            (self.s390x, self.randomize_layout)
        }
    }
}

/// Supported polyglot guest languages for `cargo xtask bench`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchLang {
    Rust,
    C99,
    Cpp17,
    Zig,
}

impl BenchLang {
    fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::C99 => "c99",
            Self::Cpp17 => "cpp17",
            Self::Zig => "zig",
        }
    }
}

/// Guest compilation profile for `cargo xtask bench`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuestProfile {
    Release,
    Debug,
}

impl GuestProfile {
    fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Debug => "debug",
        }
    }
}

/// Options for `cargo xtask bench`.
#[derive(Debug, Default, PartialEq, Eq)]
struct BenchArgs {
    langs: Option<Vec<BenchLang>>,
    guest_profiles: Option<Vec<GuestProfile>>,
    workloads: Option<String>,
    entities: Option<String>,
    chunks: Option<String>,
    quick: bool,
    strict_langs: bool,
    json: Option<PathBuf>,
    write_docs: bool,
    no_write_docs: bool,
    check_docs: bool,
}

impl BenchArgs {
    /// Returns `true` when no language, guest-profile, or workload subset filter was applied.
    fn is_full_matrix_request(&self) -> bool {
        self.langs.is_none() && self.guest_profiles.is_none() && self.workloads.is_none()
    }
    /// Returns the selected languages, defaulting to all 4 (`rust`, `c99`, `cpp17`, `zig`).
    fn resolved_langs(&self) -> Vec<BenchLang> {
        match &self.langs {
            Some(langs) if !langs.is_empty() => langs.clone(),
            _ => vec![
                BenchLang::Rust,
                BenchLang::C99,
                BenchLang::Cpp17,
                BenchLang::Zig,
            ],
        }
    }

    /// Returns `true` when `--langs` explicitly requested `lang`.
    fn explicit_lang_requested(&self, lang: BenchLang) -> bool {
        self.langs
            .as_ref()
            .is_some_and(|langs| langs.contains(&lang))
    }

    /// Returns the selected guest build profiles, defaulting to `[Release, Debug]`.
    fn resolved_profiles(&self) -> Vec<GuestProfile> {
        match &self.guest_profiles {
            Some(profiles) if !profiles.is_empty() => profiles.clone(),
            _ => vec![GuestProfile::Release, GuestProfile::Debug],
        }
    }
}

/// Options for `cargo xtask ci`.
#[derive(Debug, Default, PartialEq, Eq)]
struct CiArgs {
    audit: bool,
    miri: bool,
    kani: bool,
    no_bench: bool,
}

fn main() -> ExitCode {
    let raw_args: Vec<String> = env::args().skip(1).collect();
    match parse_args(&raw_args) {
        Ok(ParsedArgs::Help(topic)) => {
            print_help(topic);
            ExitCode::SUCCESS
        }
        Ok(ParsedArgs::Run(cmd)) => match dispatch(cmd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Parses CLI arguments after the binary name into a structured [`ParsedArgs`].
fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    if args.is_empty() {
        return Ok(ParsedArgs::Help(None));
    }

    let first = args[0].as_str();
    if is_help_flag(first) {
        return Ok(ParsedArgs::Help(None));
    }
    if first == "help" {
        return match args.get(1) {
            None => Ok(ParsedArgs::Help(None)),
            Some(sub) => {
                let kind = parse_subcommand_kind(sub).ok_or_else(|| {
                    format!("unknown help topic `{sub}`; run `cargo xtask --help`")
                })?;
                Ok(ParsedArgs::Help(Some(kind)))
            }
        };
    }

    let kind = parse_subcommand_kind(first)
        .ok_or_else(|| format!("unknown subcommand `{first}`; run `cargo xtask --help`"))?;
    let rest = &args[1..];
    if rest.iter().any(|arg| is_help_flag(arg)) {
        return Ok(ParsedArgs::Help(Some(kind)));
    }

    let spec = match kind {
        SubcommandKind::Fmt => CommandSpec::Fmt(parse_fmt_args(rest)?),
        SubcommandKind::Lint => {
            reject_extra_args("lint", rest)?;
            CommandSpec::Lint
        }
        SubcommandKind::Test => CommandSpec::Test(parse_test_args(rest)?),
        SubcommandKind::CTest => CommandSpec::CTest(parse_c_test_args(rest)?),
        SubcommandKind::Header => CommandSpec::Header(parse_check_args("header", rest)?),
        SubcommandKind::GenComponentHeaders => {
            CommandSpec::GenComponentHeaders(parse_check_args("gen-component-headers", rest)?)
        }
        SubcommandKind::Audit => CommandSpec::Audit(parse_audit_args(rest)?),
        SubcommandKind::Miri => CommandSpec::Miri(parse_miri_args(rest)?),
        SubcommandKind::Kani => CommandSpec::Kani(parse_kani_args(rest)?),
        SubcommandKind::Nightly => CommandSpec::Nightly(parse_nightly_args(rest)?),
        SubcommandKind::Bench => CommandSpec::Bench(parse_bench_args(rest)?),
        SubcommandKind::Ci => CommandSpec::Ci(parse_ci_args(rest)?),
    };
    Ok(ParsedArgs::Run(spec))
}

fn is_help_flag(arg: &str) -> bool {
    matches!(arg, "-h" | "--help")
}

fn parse_subcommand_kind(name: &str) -> Option<SubcommandKind> {
    match name {
        "fmt" => Some(SubcommandKind::Fmt),
        "lint" => Some(SubcommandKind::Lint),
        "test" => Some(SubcommandKind::Test),
        "c-test" => Some(SubcommandKind::CTest),
        "header" => Some(SubcommandKind::Header),
        "gen-component-headers" => Some(SubcommandKind::GenComponentHeaders),
        "audit" => Some(SubcommandKind::Audit),
        "miri" => Some(SubcommandKind::Miri),
        "kani" => Some(SubcommandKind::Kani),
        "nightly" => Some(SubcommandKind::Nightly),
        "bench" => Some(SubcommandKind::Bench),
        "ci" => Some(SubcommandKind::Ci),
        _ => None,
    }
}

fn reject_extra_args(cmd: &str, args: &[String]) -> Result<(), String> {
    if let Some(arg) = args.first() {
        return Err(format!(
            "unexpected argument `{arg}` for `{cmd}`; run `cargo xtask {cmd} --help`"
        ));
    }
    Ok(())
}

fn parse_fmt_args(args: &[String]) -> Result<FmtArgs, String> {
    let mut parsed = FmtArgs::default();
    for arg in args {
        match arg.as_str() {
            "--fix" => parsed.fix = true,
            other => {
                return Err(format!(
                    "unknown `fmt` flag `{other}`; run `cargo xtask fmt --help`"
                ));
            }
        }
    }
    Ok(parsed)
}

fn parse_positive_usize(flag: &str, value: &str) -> Result<usize, String> {
    let n: usize = value
        .parse()
        .map_err(|_| format!("invalid integer `{value}` for `{flag}`"))?;
    if n == 0 {
        return Err(format!("`{flag}` must be greater than 0"));
    }
    Ok(n)
}

fn next_flag_value<'a>(args: &'a [String], idx: &mut usize, flag: &str) -> Result<&'a str, String> {
    let next = *idx + 1;
    let val = args
        .get(next)
        .ok_or_else(|| format!("missing value after `{flag}`"))?;
    if val.is_empty() {
        return Err(format!("empty value for `{flag}`"));
    }
    *idx = next;
    Ok(val.as_str())
}

fn parse_test_args(args: &[String]) -> Result<TestArgs, String> {
    let mut parsed = TestArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(val) = arg.strip_prefix("--cases=") {
            parsed.cases = Some(parse_positive_usize("--cases", val)?);
        } else {
            match arg {
                "--cases" => {
                    let val = next_flag_value(args, &mut i, "--cases")?;
                    parsed.cases = Some(parse_positive_usize("--cases", val)?);
                }
                "--no-default-features" => parsed.no_default_features = true,
                "--all-features" => parsed.all_features = true,
                "--doc" => parsed.doc = true,
                "--proptest" => parsed.proptest = true,
                "--header-parity" => parsed.header_parity = true,
                other => {
                    return Err(format!(
                        "unknown `test` flag `{other}`; run `cargo xtask test --help`"
                    ));
                }
            }
        }
        i += 1;
    }
    Ok(parsed)
}

fn parse_c_test_args(args: &[String]) -> Result<CTestArgs, String> {
    let mut parsed = CTestArgs::default();
    for arg in args {
        match arg.as_str() {
            "--no-asan" => parsed.no_asan = true,
            "--asan-only" => parsed.asan_only = true,
            other => {
                return Err(format!(
                    "unknown `c-test` flag `{other}`; run `cargo xtask c-test --help`"
                ));
            }
        }
    }
    if parsed.no_asan && parsed.asan_only {
        return Err("cannot combine `--no-asan` and `--asan-only`".to_owned());
    }
    Ok(parsed)
}

fn parse_check_args(cmd: &str, args: &[String]) -> Result<CheckArgs, String> {
    let mut parsed = CheckArgs::default();
    for arg in args {
        match arg.as_str() {
            "--check" => parsed.check = true,
            other => {
                return Err(format!(
                    "unknown `{cmd}` flag `{other}`; run `cargo xtask {cmd} --help`"
                ));
            }
        }
    }
    Ok(parsed)
}

fn parse_audit_args(args: &[String]) -> Result<AuditArgs, String> {
    let mut parsed = AuditArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(rev) = arg.strip_prefix("--baseline-rev=") {
            if rev.is_empty() {
                return Err("empty value for `--baseline-rev`".to_owned());
            }
            parsed.baseline_rev = Some(rev.to_owned());
        } else {
            match arg {
                "--baseline-rev" => {
                    let rev = next_flag_value(args, &mut i, "--baseline-rev")?;
                    parsed.baseline_rev = Some(rev.to_owned());
                }
                "--skip-targets" => parsed.skip_targets = true,
                "--skip-typos" => parsed.skip_typos = true,
                other => {
                    return Err(format!(
                        "unknown `audit` flag `{other}`; run `cargo xtask audit --help`"
                    ));
                }
            }
        }
        i += 1;
    }
    Ok(parsed)
}

fn parse_miri_args(args: &[String]) -> Result<MiriArgs, String> {
    let mut parsed = MiriArgs::default();
    for arg in args {
        match arg.as_str() {
            "--tree-borrows" => parsed.tree_borrows = true,
            "--stacked-borrows" => parsed.stacked_borrows = true,
            "--all" => {
                parsed.tree_borrows = true;
                parsed.stacked_borrows = true;
            }
            "--s390x" => parsed.s390x = true,
            other => {
                return Err(format!(
                    "unknown `miri` flag `{other}`; run `cargo xtask miri --help`"
                ));
            }
        }
    }
    Ok(parsed)
}

fn parse_kani_args(args: &[String]) -> Result<KaniArgs, String> {
    let mut parsed = KaniArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(pat) = arg.strip_prefix("--harness=") {
            if pat.is_empty() {
                return Err("empty value for `--harness`".to_owned());
            }
            parsed.harness = Some(pat.to_owned());
        } else if let Some(pkg) = arg.strip_prefix("--package=") {
            if pkg.is_empty() {
                return Err("empty value for `--package`".to_owned());
            }
            parsed.packages.push(pkg.to_owned());
        } else {
            match arg {
                "--harness" => {
                    let pat = next_flag_value(args, &mut i, "--harness")?;
                    parsed.harness = Some(pat.to_owned());
                }
                "-p" | "--package" => {
                    let pkg = next_flag_value(args, &mut i, arg)?;
                    parsed.packages.push(pkg.to_owned());
                }
                other => {
                    return Err(format!(
                        "unknown `kani` flag `{other}`; run `cargo xtask kani --help`"
                    ));
                }
            }
        }
        i += 1;
    }
    Ok(parsed)
}

fn parse_nightly_args(args: &[String]) -> Result<NightlyArgs, String> {
    let mut parsed = NightlyArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(val) = arg.strip_prefix("--cases=") {
            parsed.cases = parse_positive_usize("--cases", val)?;
        } else {
            match arg {
                "--cases" => {
                    let val = next_flag_value(args, &mut i, "--cases")?;
                    parsed.cases = parse_positive_usize("--cases", val)?;
                }
                "--s390x" => parsed.s390x = true,
                "--randomize-layout" => parsed.randomize_layout = true,
                other => {
                    return Err(format!(
                        "unknown `nightly` flag `{other}`; run `cargo xtask nightly --help`"
                    ));
                }
            }
        }
        i += 1;
    }
    Ok(parsed)
}

fn parse_bench_lang(item: &str) -> Option<BenchLang> {
    match item {
        "rust" => Some(BenchLang::Rust),
        "c99" => Some(BenchLang::C99),
        "cpp17" => Some(BenchLang::Cpp17),
        "zig" => Some(BenchLang::Zig),
        _ => None,
    }
}

fn parse_guest_profile(item: &str) -> Option<GuestProfile> {
    match item {
        "release" => Some(GuestProfile::Release),
        "debug" => Some(GuestProfile::Debug),
        _ => None,
    }
}

fn parse_csv_enum<T: Copy + PartialEq>(
    flag: &str,
    value: &str,
    parse_item: fn(&str) -> Option<T>,
    valid: &str,
) -> Result<Vec<T>, String> {
    let mut out = Vec::new();
    for raw in value.split(',') {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(format!("empty item in `{flag}` list `{value}`"));
        }
        let parsed = parse_item(trimmed).ok_or_else(|| {
            format!("unknown `{flag}` value `{trimmed}`; expected subset of {valid}")
        })?;
        if !out.contains(&parsed) {
            out.push(parsed);
        }
    }
    if out.is_empty() {
        return Err(format!("empty value for `{flag}`"));
    }
    Ok(out)
}

fn parse_bench_args(args: &[String]) -> Result<BenchArgs, String> {
    let mut parsed = BenchArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let (flag, inline_val) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v)),
            None => (arg, None),
        };
        let take_val = |idx: &mut usize| -> Result<String, String> {
            let val = if let Some(v) = inline_val {
                v.to_owned()
            } else {
                next_flag_value(args, idx, flag)?.to_owned()
            };
            if val.trim().is_empty() {
                return Err(format!("empty value for `{flag}`"));
            }
            Ok(val)
        };
        match flag {
            "--langs" => {
                let v = take_val(&mut i)?;
                parsed.langs = Some(parse_csv_enum(
                    "--langs",
                    &v,
                    parse_bench_lang,
                    "rust, c99, cpp17, zig",
                )?);
            }
            "--guest-profiles" => {
                let v = take_val(&mut i)?;
                parsed.guest_profiles = Some(parse_csv_enum(
                    "--guest-profiles",
                    &v,
                    parse_guest_profile,
                    "release, debug",
                )?);
            }
            "--workloads" => parsed.workloads = Some(take_val(&mut i)?),
            "--entities" => parsed.entities = Some(take_val(&mut i)?),
            "--chunks" => parsed.chunks = Some(take_val(&mut i)?),
            "--json" => parsed.json = Some(PathBuf::from(take_val(&mut i)?)),
            "--quick" if inline_val.is_none() => parsed.quick = true,
            "--strict-langs" if inline_val.is_none() => parsed.strict_langs = true,
            "--write-docs" if inline_val.is_none() => parsed.write_docs = true,
            "--no-write-docs" if inline_val.is_none() => parsed.no_write_docs = true,
            "--check-docs" if inline_val.is_none() => parsed.check_docs = true,
            _ => {
                return Err(format!(
                    "unknown `bench` flag `{arg}`; run `cargo xtask bench --help`"
                ));
            }
        }
        i += 1;
    }
    if parsed.write_docs && parsed.check_docs {
        return Err("cannot combine `--write-docs` and `--check-docs`".to_owned());
    }
    if parsed.write_docs && parsed.no_write_docs {
        return Err("cannot combine `--write-docs` and `--no-write-docs`".to_owned());
    }
    Ok(parsed)
}

fn parse_ci_args(args: &[String]) -> Result<CiArgs, String> {
    let mut parsed = CiArgs::default();
    for arg in args {
        match arg.as_str() {
            "--full" => {
                parsed.audit = true;
                parsed.miri = true;
                parsed.kani = true;
            }
            "--audit" => parsed.audit = true,
            "--miri" => parsed.miri = true,
            "--kani" => parsed.kani = true,
            "--no-bench" => parsed.no_bench = true,
            other => {
                return Err(format!(
                    "unknown `ci` flag `{other}`; run `cargo xtask ci --help`"
                ));
            }
        }
    }
    Ok(parsed)
}

fn print_help(topic: Option<SubcommandKind>) {
    match topic {
        None => println!(
            "\
bamabi-wire task runner (`cargo xtask`)

USAGE:
    cargo xtask <SUBCOMMAND> [OPTIONS]

SUBCOMMANDS:
    fmt                    Check or fix workspace Rust formatting
    lint                   Verify #![forbid(unsafe_code)], zero-Bevy, clippy, and rustdoc
    test                   Run workspace tests (--no-default-features, --all-features, doctests)
    c-test                 Run strict C99 header checks and standalone_c_test (+ ASan/UBSan)
    header                 Regenerate or check include/bamabi.h byte parity
    gen-component-headers  Regenerate or check include/generated/testbed_components.h parity
    audit                  Run cross-target checks, typos, cargo-deny, and optional semver-checks
    miri                   Run Miri (Tree Borrows, Stacked Borrows, and/or s390x)
    kani                   Run Kani formal verification proofs
    nightly                Run s390x Miri and -Zrandomize-layout extended suites
    bench                  Build and run the polyglot host-guest benchmark suite (Rust, C99, C++17, Zig)
    ci                     Run the local CI gate (fmt, headers, lint, test, c-test, bench --quick)

Run `cargo xtask <SUBCOMMAND> --help` for subcommand options."
        ),
        Some(SubcommandKind::Fmt) => println!(
            "\
USAGE:
    cargo xtask fmt [--fix]

OPTIONS:
    --fix    Apply formatting changes instead of checking"
        ),
        Some(SubcommandKind::Lint) => println!(
            "\
USAGE:
    cargo xtask lint

CHECKS:
    - #![forbid(unsafe_code)] on bamabi_docs, bamabi_guest, bamabi_macros, bamabi_sdk_gen, and xtask
    - Zero non-bamabi Bevy dependencies in `cargo tree --workspace`
    - `cargo clippy --workspace --all-targets -- -D warnings` (default & --all-features)
    - `RUSTDOCFLAGS=\"-D warnings\" cargo doc --workspace --no-deps` (default & --all-features)"
        ),
        Some(SubcommandKind::Test) => println!(
            "\
USAGE:
    cargo xtask test [--cases N] [--no-default-features] [--all-features] [--doc] [--proptest] [--header-parity]

OPTIONS:
    --cases N                Set PROPTEST_CASES=N
    --no-default-features    Run `cargo test --workspace --no-default-features`
    --all-features           Run `cargo test --workspace --all-features`
    --doc                    Run `cargo test --doc --all-features`
    --proptest               Run the proptest integration suites
    --header-parity          Run `cargo test -p bamabi_sdk_gen --test header_parity`

Defaults to running --no-default-features, --all-features, and --doc."
        ),
        Some(SubcommandKind::CTest) => println!(
            "\
USAGE:
    cargo xtask c-test [--no-asan | --asan-only]

OPTIONS:
    --no-asan      Skip the ASan + UBSan clang build and run of tests/standalone_c_test.c
    --asan-only    Run only the ASan + UBSan clang build and run of tests/standalone_c_test.c"
        ),
        Some(SubcommandKind::Header) => println!(
            "\
USAGE:
    cargo xtask header [--check]

OPTIONS:
    --check    Verify include/bamabi.h matches bamabi_sdk_gen::render_c99_header() without modifying disk"
        ),
        Some(SubcommandKind::GenComponentHeaders) => println!(
            "\
USAGE:
    cargo xtask gen-component-headers [--check]

OPTIONS:
    --check    Verify include/generated/testbed_components.h matches ModComponent::C_DECL byte-for-byte"
        ),
        Some(SubcommandKind::Audit) => println!(
            "\
USAGE:
    cargo xtask audit [--baseline-rev <rev>] [--skip-targets] [--skip-typos]

OPTIONS:
    --baseline-rev <rev>    Run `cargo semver-checks check-release -p bamabi_sys --baseline-rev <rev>`
    --skip-targets          Skip i686-unknown-linux-gnu and wasm32-unknown-unknown checks
    --skip-typos            Skip running the `typos` spellchecker"
        ),
        Some(SubcommandKind::Miri) => println!(
            "\
USAGE:
    cargo xtask miri [--tree-borrows | --stacked-borrows | --all | --s390x]

OPTIONS:
    --tree-borrows       Run Miri with -Zmiri-tree-borrows -Zmiri-strict-provenance
    --stacked-borrows    Run Miri with -Zmiri-strict-provenance
    --all                Run both Tree Borrows and Stacked Borrows (default)
    --s390x              Run big-endian s390x-unknown-linux-gnu Miri tests"
        ),
        Some(SubcommandKind::Kani) => println!(
            "\
USAGE:
    cargo xtask kani [--harness <pat>] [-p <pkg>]

OPTIONS:
    --harness <pat>    Filter Kani proofs by harness name pattern
    -p, --package <pkg>  Restrict Kani to specific package(s) (repeatable; defaults to bamabi_sys + bamabi_guest_sys)"
        ),
        Some(SubcommandKind::Nightly) => println!(
            "\
USAGE:
    cargo xtask nightly [--cases N] [--s390x] [--randomize-layout]

OPTIONS:
    --cases N             Set PROPTEST_CASES=N for -Zrandomize-layout runs (default: 2048)
    --s390x               Run only the big-endian s390x-unknown-linux-gnu Miri suite
    --randomize-layout    Run only the -Zrandomize-layout + extended proptest suites

Defaults to running both --s390x and --randomize-layout."
        ),
        Some(SubcommandKind::Bench) => println!(
            "\
USAGE:
    cargo xtask bench [OPTIONS]

OPTIONS:
    --langs <csv>             Comma-separated subset of: rust, c99, cpp17, zig (default: all)
    --guest-profiles <csv>    Comma-separated subset of: release, debug (default: release,debug)
    --workloads <csv>         Filter workloads (noop,dense_query_update,command_burst,host_api_roundtrip)
    --entities <csv>          Entity counts for dense_query_update (default: 1000,100000)
    --chunks <csv>            Chunk sizes for dense_query_update (default: 256,4096; quick: 256)
    --quick                   Use fast warmup/sample counts (also run by `cargo xtask ci`)
    --strict-langs            Fail instead of skipping if an optional toolchain (e.g. zig) is missing
    --json <path>             Write JSON benchmark results to <path>
    --write-docs              Force refreshing BENCHMARKS.md (automatic on full 8-guest runs)
    --no-write-docs           Do not update BENCHMARKS.md after running benchmarks
    --check-docs              Verify BENCHMARKS.md contains valid markers and complete results"
        ),
        Some(SubcommandKind::Ci) => println!(
            "\
USAGE:
    cargo xtask ci [--full] [--audit] [--miri] [--kani] [--no-bench]

OPTIONS:
    --audit       Also run `cargo xtask audit`
    --miri        Also run `cargo xtask miri`
    --kani        Also run `cargo xtask kani`
    --no-bench    Skip `cargo xtask bench --quick`
    --full        Enable --audit, --miri, and --kani

Default runs fmt, header --check, gen-component-headers --check, lint, test, c-test, and bench --quick."
        ),
    }
}

fn workspace_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "failed to resolve workspace root from CARGO_MANIFEST_DIR".to_owned())
}

fn dispatch(cmd: CommandSpec) -> Result<(), String> {
    let root = workspace_root()?;
    match cmd {
        CommandSpec::Fmt(args) => cmd_fmt(&root, &args),
        CommandSpec::Lint => cmd_lint(&root),
        CommandSpec::Test(args) => cmd_test(&root, &args),
        CommandSpec::CTest(args) => cmd_c_test(&root, &args),
        CommandSpec::Header(args) => cmd_header(&root, &args),
        CommandSpec::GenComponentHeaders(args) => {
            component_headers::write_or_check_component_headers(&root, args.check)
        }
        CommandSpec::Audit(args) => cmd_audit(&root, &args),
        CommandSpec::Miri(args) => cmd_miri(&root, &args),
        CommandSpec::Kani(args) => cmd_kani(&root, &args),
        CommandSpec::Nightly(args) => cmd_nightly(&root, &args),
        CommandSpec::Bench(args) => cmd_bench(&root, &args),
        CommandSpec::Ci(args) => cmd_ci(&root, &args),
    }
}

/// Runs a command inheriting stdio, returning a descriptive error on non-zero exit.
fn run_cmd(cmd: &mut Command, label: &str) -> Result<(), String> {
    println!("==> {label}");
    let status = cmd
        .status()
        .map_err(|e| format!("failed to spawn `{label}`: {e}"))?;
    if !status.success() {
        return Err(format!("`{label}` failed with exit status {status}"));
    }
    Ok(())
}

/// Builds a `cargo` command anchored at `root`.
///
/// Uses `"cargo"` (the rustup proxy) rather than `env!("CARGO")` so `+nightly` and
/// subcommands like `cargo kani` or `cargo deny` resolve their proper toolchains.
fn cargo_cmd(root: &Path) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root);
    cmd
}

/// Builds a `cargo` command cleared of active `RUSTUP_TOOLCHAIN` / `RUSTC` overrides.
fn clean_toolchain_cargo_cmd(root: &Path) -> Command {
    let mut cmd = cargo_cmd(root);
    cmd.env_remove("RUSTUP_TOOLCHAIN");
    cmd.env_remove("RUSTC");
    cmd
}

/// Probes whether a tool command succeeds silently.
fn check_tool_available(program: &str, args: &[&str], install_hint: &str) -> Result<(), String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTC")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cmd.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!(
            "`{program} {}` exited with {status}; {install_hint}",
            args.join(" ")
        )),
        Err(err) => Err(format!("`{program}` not available ({err}); {install_hint}")),
    }
}

fn cmd_fmt(root: &Path, args: &FmtArgs) -> Result<(), String> {
    let mut cmd = cargo_cmd(root);
    cmd.args(["fmt", "--all"]);
    if args.fix {
        run_cmd(&mut cmd, "cargo fmt --all")
    } else {
        cmd.args(["--", "--check"]);
        run_cmd(&mut cmd, "cargo fmt --all -- --check")
    }
}

fn cmd_lint(root: &Path) -> Result<(), String> {
    verify_forbid_unsafe(root)?;
    verify_zero_bevy(root)?;

    run_cmd(
        cargo_cmd(root).args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]),
        "cargo clippy --workspace --all-targets -- -D warnings",
    )?;
    run_cmd(
        cargo_cmd(root).args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ]),
        "cargo clippy --workspace --all-targets --all-features -- -D warnings",
    )?;
    run_cmd(
        cargo_cmd(root).env("RUSTDOCFLAGS", "-D warnings").args([
            "doc",
            "--workspace",
            "--no-deps",
        ]),
        "RUSTDOCFLAGS=\"-D warnings\" cargo doc --workspace --no-deps",
    )?;
    run_cmd(
        cargo_cmd(root).env("RUSTDOCFLAGS", "-D warnings").args([
            "doc",
            "--workspace",
            "--no-deps",
            "--all-features",
        ]),
        "RUSTDOCFLAGS=\"-D warnings\" cargo doc --workspace --no-deps --all-features",
    )?;
    Ok(())
}

fn verify_forbid_unsafe(root: &Path) -> Result<(), String> {
    println!("==> Verifying #![forbid(unsafe_code)] on safe crates");
    let required = [
        "crates/bamabi_docs/src/lib.rs",
        "crates/bamabi_guest/src/lib.rs",
        "crates/bamabi_macros/src/lib.rs",
        "crates/bamabi_sdk_gen/src/lib.rs",
        "xtask/src/main.rs",
    ];
    for rel in required {
        let path = root.join(rel);
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let has_forbid = content
            .lines()
            .any(|line| line.trim() == "#![forbid(unsafe_code)]");
        if !has_forbid {
            return Err(format!(
                "{} is missing required `#![forbid(unsafe_code)]` declaration",
                path.display()
            ));
        }
    }
    Ok(())
}

fn verify_zero_bevy(root: &Path) -> Result<(), String> {
    println!("==> Verifying zero-Bevy isolation (`cargo tree --workspace`)");
    let output = cargo_cmd(root)
        .args(["tree", "--workspace"])
        .output()
        .map_err(|e| format!("failed to run `cargo tree --workspace`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`cargo tree --workspace` failed: {stderr}"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut offenders = Vec::new();
    for line in stdout.lines() {
        let stripped = line.split(" (").next().unwrap_or(line);
        let lower = stripped.to_ascii_lowercase();
        if lower.contains("bevy") && !lower.contains("bamabi") {
            offenders.push(stripped.trim().to_owned());
        }
    }
    if !offenders.is_empty() {
        return Err(format!(
            "unexpected Bevy dependency found in bamabi-wire:\n{}",
            offenders.join("\n")
        ));
    }
    Ok(())
}

fn cmd_test(root: &Path, args: &TestArgs) -> Result<(), String> {
    let apply_cases = |cmd: &mut Command| {
        if let Some(n) = args.cases {
            cmd.env("PROPTEST_CASES", n.to_string());
        }
    };

    if args.run_all_default() {
        let mut c1 = cargo_cmd(root);
        c1.args(["test", "--workspace", "--no-default-features"]);
        apply_cases(&mut c1);
        run_cmd(&mut c1, "cargo test --workspace --no-default-features")?;

        let mut c2 = cargo_cmd(root);
        c2.args(["test", "--workspace", "--all-features"]);
        apply_cases(&mut c2);
        run_cmd(&mut c2, "cargo test --workspace --all-features")?;

        let mut c3 = cargo_cmd(root);
        c3.args(["test", "--doc", "--all-features"]);
        apply_cases(&mut c3);
        run_cmd(&mut c3, "cargo test --doc --all-features")?;
        return Ok(());
    }

    if args.no_default_features {
        let mut cmd = cargo_cmd(root);
        cmd.args(["test", "--workspace", "--no-default-features"]);
        apply_cases(&mut cmd);
        run_cmd(&mut cmd, "cargo test --workspace --no-default-features")?;
    }
    if args.all_features {
        let mut cmd = cargo_cmd(root);
        cmd.args(["test", "--workspace", "--all-features"]);
        apply_cases(&mut cmd);
        run_cmd(&mut cmd, "cargo test --workspace --all-features")?;
    }
    if args.doc {
        let mut cmd = cargo_cmd(root);
        cmd.args(["test", "--doc", "--all-features"]);
        apply_cases(&mut cmd);
        run_cmd(&mut cmd, "cargo test --doc --all-features")?;
    }
    if args.proptest {
        let proptests = [
            ("bamabi_sys", "proptest_wire"),
            ("bamabi_guest_sys", "proptest_guest_sys"),
            ("bamabi_macros", "proptest_witness"),
        ];
        for (pkg, target) in proptests {
            let mut cmd = cargo_cmd(root);
            cmd.args(["test", "-p", pkg, "--test", target]);
            apply_cases(&mut cmd);
            run_cmd(&mut cmd, &format!("cargo test -p {pkg} --test {target}"))?;
        }
    }
    if args.header_parity {
        let mut cmd = cargo_cmd(root);
        cmd.args(["test", "-p", "bamabi_sdk_gen", "--test", "header_parity"]);
        apply_cases(&mut cmd);
        run_cmd(
            &mut cmd,
            "cargo test -p bamabi_sdk_gen --test header_parity",
        )?;
    }

    Ok(())
}

fn c_compiler() -> String {
    env::var("CC").unwrap_or_else(|_| "cc".to_owned())
}

fn cpp_compiler() -> String {
    env::var("CXX").unwrap_or_else(|_| "c++".to_owned())
}

fn cargo_target_dir(root: &Path) -> PathBuf {
    match env::var_os("CARGO_TARGET_DIR") {
        Some(val) if !val.is_empty() => {
            let path = PathBuf::from(val);
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        }
        _ => root.join("target"),
    }
}

fn sanitizer_clang() -> String {
    if let Ok(cc) = env::var("CLANG") {
        return cc;
    }
    // On macOS, a Swift toolchain (`swiftly`) shim for `clang` on PATH can stall on ASan runs;
    // prefer the system Apple Xcode `/usr/bin/clang` when present.
    if cfg!(target_os = "macos") && Path::new("/usr/bin/clang").is_file() {
        return "/usr/bin/clang".to_owned();
    }
    "clang".to_owned()
}

fn portable_asan_options() -> &'static str {
    // Apple Clang ASan on macOS ARM64 aborts if `detect_leaks=1` is passed.
    if cfg!(target_os = "linux") {
        "detect_leaks=1:strict_string_checks=1:halt_on_error=1"
    } else {
        "strict_string_checks=1:halt_on_error=1"
    }
}

fn cmd_c_test(root: &Path, args: &CTestArgs) -> Result<(), String> {
    let target_dir = root.join("target");
    fs::create_dir_all(&target_dir)
        .map_err(|e| format!("failed to create {}: {e}", target_dir.display()))?;

    let c_test_src = root.join("tests").join("standalone_c_test.c");
    let include_dir = root.join("include");
    let exe_suffix = env::consts::EXE_SUFFIX;

    if !args.asan_only {
        let cc = c_compiler();
        let bamabi_h = root.join("include").join("bamabi.h");
        let testbed_h = root
            .join("include")
            .join("generated")
            .join("testbed_components.h");

        run_cmd(
            Command::new(&cc)
                .current_dir(root)
                .args([
                    "-std=c99",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-pedantic",
                    "-fsyntax-only",
                    "-x",
                    "c",
                ])
                .arg(&bamabi_h),
            &format!(
                "{cc} -std=c99 -Wall -Wextra -Werror -pedantic -fsyntax-only include/bamabi.h"
            ),
        )?;

        run_cmd(
            Command::new(&cc)
                .current_dir(root)
                .args([
                    "-std=c99",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-pedantic",
                    "-fsyntax-only",
                    "-x",
                    "c",
                ])
                .arg(&testbed_h),
            &format!(
                "{cc} -std=c99 -Wall -Wextra -Werror -pedantic -fsyntax-only include/generated/testbed_components.h"
            ),
        )?;

        let bin_path = target_dir.join(format!("standalone_c_test{exe_suffix}"));
        run_cmd(
            Command::new(&cc)
                .current_dir(root)
                .args(["-std=c99", "-Wall", "-Wextra", "-Werror", "-pedantic", "-I"])
                .arg(&include_dir)
                .arg(&c_test_src)
                .arg("-o")
                .arg(&bin_path),
            &format!("{cc} tests/standalone_c_test.c -> target/standalone_c_test{exe_suffix}"),
        )?;
        run_cmd(
            Command::new(&bin_path).current_dir(root),
            &format!("target/standalone_c_test{exe_suffix}"),
        )?;
    }

    if !args.no_asan {
        let clang = sanitizer_clang();
        let san_path = target_dir.join(format!("standalone_c_test_san{exe_suffix}"));
        run_cmd(
            Command::new(&clang)
                .current_dir(root)
                .args([
                    "-std=c99",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-pedantic",
                    "-O1",
                    "-g",
                    "-fno-omit-frame-pointer",
                    "-fsanitize=address,undefined",
                    "-fno-sanitize-recover=all",
                    "-I",
                ])
                .arg(&include_dir)
                .arg(&c_test_src)
                .arg("-o")
                .arg(&san_path),
            &format!(
                "{clang} -fsanitize=address,undefined tests/standalone_c_test.c -> target/standalone_c_test_san{exe_suffix}"
            ),
        )?;
        run_cmd(
            Command::new(&san_path)
                .current_dir(root)
                .env("ASAN_OPTIONS", portable_asan_options())
                .env("UBSAN_OPTIONS", "print_stacktrace=1:halt_on_error=1"),
            &format!("target/standalone_c_test_san{exe_suffix} (ASan + UBSan)"),
        )?;
    }

    Ok(())
}

fn cmd_header(root: &Path, args: &CheckArgs) -> Result<(), String> {
    if args.check {
        let rendered = bamabi_sdk_gen::render_c99_header()?;
        let header_path = root.join("include").join("bamabi.h");
        let existing = fs::read_to_string(&header_path)
            .map_err(|e| format!("failed to read {}: {e}", header_path.display()))?
            .replace("\r\n", "\n");
        if existing != rendered {
            let diff = component_headers::first_line_diff(&rendered, &existing);
            return Err(format!(
                "{} is out of date with crates/bamabi_sys ({diff}); run `cargo xtask header`",
                header_path.display()
            ));
        }
        println!(
            "==> Verified {} matches render_c99_header() byte-for-byte",
            header_path.display()
        );
        return Ok(());
    }
    bamabi_sdk_gen::write_or_check_header(root, false)
}

fn cmd_audit(root: &Path, args: &AuditArgs) -> Result<(), String> {
    if !args.skip_targets {
        let targets = ["i686-unknown-linux-gnu", "wasm32-unknown-unknown"];
        verify_rustup_targets(&targets)?;
        for target in targets {
            run_cmd(
                cargo_cmd(root).args(["check", "-p", "bamabi_sys", "--target", target]),
                &format!("cargo check -p bamabi_sys --target {target}"),
            )?;
        }
    }

    if !args.skip_typos {
        check_tool_available(
            "typos",
            &["--version"],
            "install with `cargo install typos-cli`",
        )?;
        run_cmd(Command::new("typos").current_dir(root), "typos")?;
    }

    check_tool_available(
        "cargo",
        &["deny", "--version"],
        "install with `cargo install cargo-deny`",
    )?;
    run_cmd(
        clean_toolchain_cargo_cmd(root).args([
            "deny",
            "--all-features",
            "check",
            "advisories",
            "licenses",
            "bans",
            "sources",
        ]),
        "cargo deny --all-features check advisories licenses bans sources",
    )?;

    if let Some(rev) = &args.baseline_rev {
        check_tool_available(
            "cargo",
            &["semver-checks", "--version"],
            "install with `cargo install cargo-semver-checks`",
        )?;
        run_cmd(
            clean_toolchain_cargo_cmd(root).args([
                "semver-checks",
                "check-release",
                "-p",
                "bamabi_sys",
                "--baseline-rev",
                rev,
            ]),
            &format!("cargo semver-checks check-release -p bamabi_sys --baseline-rev {rev}"),
        )?;
    }

    Ok(())
}

fn verify_rustup_targets(required: &[&str]) -> Result<(), String> {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .env_remove("RUSTUP_TOOLCHAIN")
        .output();
    let Ok(output) = output else {
        // If `rustup` is not present (e.g. distro-packaged toolchain), let `cargo check` decide.
        return Ok(());
    };
    if !output.status.success() {
        return Ok(());
    }
    let installed = String::from_utf8_lossy(&output.stdout);
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|target| !installed.lines().any(|line| line.trim() == *target))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "missing Rust target(s): {}; install with `rustup target add {}` (or pass `--skip-targets`)",
            missing.join(", "),
            missing.join(" ")
        ));
    }
    Ok(())
}

fn cmd_miri(root: &Path, args: &MiriArgs) -> Result<(), String> {
    check_tool_available(
        "cargo",
        &["+nightly", "miri", "--version"],
        "install with `rustup toolchain install nightly --component miri`",
    )?;
    let (tree_borrows, stacked_borrows, s390x) = args.resolved();

    if tree_borrows {
        let flags = "-Zmiri-tree-borrows -Zmiri-strict-provenance";
        run_miri_suite(root, flags, "Tree Borrows")?;
    }
    if stacked_borrows {
        let flags = "-Zmiri-strict-provenance";
        run_miri_suite(root, flags, "Stacked Borrows")?;
    }
    if s390x {
        let flags = "-Zmiri-tree-borrows -Zmiri-strict-provenance";
        run_cmd(
            clean_toolchain_cargo_cmd(root)
                .env("MIRIFLAGS", flags)
                .args([
                    "+nightly",
                    "miri",
                    "test",
                    "--target",
                    "s390x-unknown-linux-gnu",
                    "-p",
                    "bamabi_sys",
                    "-p",
                    "bamabi_guest_sys",
                    "-p",
                    "bamabi_guest",
                    "--all-features",
                ]),
            "cargo +nightly miri test --target s390x-unknown-linux-gnu (Tree Borrows + strict provenance)",
        )?;
    }

    Ok(())
}

fn run_miri_suite(root: &Path, miriflags: &str, label: &str) -> Result<(), String> {
    run_cmd(
        clean_toolchain_cargo_cmd(root)
            .env("MIRIFLAGS", miriflags)
            .args([
                "+nightly",
                "miri",
                "test",
                "-p",
                "bamabi_sys",
                "-p",
                "bamabi_guest_sys",
                "-p",
                "bamabi_guest",
                "--all-features",
            ]),
        &format!(
            "cargo +nightly miri test ({label}) -p bamabi_sys -p bamabi_guest_sys -p bamabi_guest --all-features"
        ),
    )?;
    run_cmd(
        clean_toolchain_cargo_cmd(root)
            .env("MIRIFLAGS", miriflags)
            .args([
                "+nightly",
                "miri",
                "test",
                "-p",
                "bamabi_macros",
                "--test",
                "mod_component_vtable",
            ]),
        &format!("cargo +nightly miri test ({label}) -p bamabi_macros --test mod_component_vtable"),
    )?;
    Ok(())
}

fn cmd_kani(root: &Path, args: &KaniArgs) -> Result<(), String> {
    check_tool_available(
        "cargo",
        &["kani", "--version"],
        "install Kani (`cargo install --locked kani-verifier && cargo kani setup`)",
    )?;
    let mut cmd = clean_toolchain_cargo_cmd(root);
    cmd.arg("kani");
    if args.packages.is_empty() {
        cmd.args(["-p", "bamabi_sys", "-p", "bamabi_guest_sys"]);
    } else {
        for pkg in &args.packages {
            cmd.args(["-p", pkg]);
        }
    }
    if let Some(pat) = &args.harness {
        cmd.args(["--harness", pat]);
    }
    cmd.args([
        "-Z",
        "unstable-options",
        "--harness-timeout",
        "15m",
        "--output-format",
        "terse",
        "-j",
    ]);
    run_cmd(&mut cmd, "cargo kani")
}

fn cmd_nightly(root: &Path, args: &NightlyArgs) -> Result<(), String> {
    let (run_s390x, run_randomize_layout) = args.resolved();

    if run_s390x {
        cmd_miri(
            root,
            &MiriArgs {
                tree_borrows: false,
                stacked_borrows: false,
                s390x: true,
            },
        )?;
    }

    if run_randomize_layout {
        check_tool_available(
            "cargo",
            &["+nightly", "--version"],
            "install with `rustup toolchain install nightly`",
        )?;
        let cases = args.cases.to_string();

        run_cmd(
            clean_toolchain_cargo_cmd(root)
                .env("RUSTFLAGS", "-Zrandomize-layout")
                .env("PROPTEST_CASES", &cases)
                .args([
                    "+nightly",
                    "test",
                    "-p",
                    "bamabi_sys",
                    "-p",
                    "bamabi_guest_sys",
                    "-p",
                    "bamabi_guest",
                    "--all-features",
                ]),
            &format!(
                "RUSTFLAGS=\"-Zrandomize-layout\" PROPTEST_CASES={cases} cargo +nightly test -p bamabi_sys -p bamabi_guest_sys -p bamabi_guest --all-features"
            ),
        )?;
        run_cmd(
            clean_toolchain_cargo_cmd(root)
                .env("RUSTFLAGS", "-Zrandomize-layout")
                .env("PROPTEST_CASES", &cases)
                .args([
                    "+nightly",
                    "test",
                    "-p",
                    "bamabi_macros",
                    "--lib",
                    "--test",
                    "mod_component_vtable",
                ]),
            &format!(
                "RUSTFLAGS=\"-Zrandomize-layout\" PROPTEST_CASES={cases} cargo +nightly test -p bamabi_macros --lib --test mod_component_vtable"
            ),
        )?;
        run_cmd(
            clean_toolchain_cargo_cmd(root)
                .env("RUSTFLAGS", "-Zrandomize-layout")
                .env("PROPTEST_CASES", &cases)
                .args(["+nightly", "test", "-p", "bamabi_sdk_gen"]),
            &format!(
                "RUSTFLAGS=\"-Zrandomize-layout\" PROPTEST_CASES={cases} cargo +nightly test -p bamabi_sdk_gen"
            ),
        )?;
    }

    Ok(())
}

fn guest_lib_filename(lang: BenchLang, profile: GuestProfile) -> String {
    format!(
        "libbench_{}_{}{}",
        lang.as_str(),
        profile.as_str(),
        env::consts::DLL_SUFFIX
    )
}

fn shared_compile_flags() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["-fPIC", "-dynamiclib"]
    } else if cfg!(target_os = "windows") {
        &["-shared"]
    } else {
        &["-fPIC", "-shared"]
    }
}

fn ensure_optional_bench_tool(
    program: &str,
    version_args: &[&str],
    install_hint: &str,
    lang: BenchLang,
    args: &BenchArgs,
) -> Result<bool, String> {
    match check_tool_available(program, version_args, install_hint) {
        Ok(()) => Ok(true),
        Err(err) => {
            if args.strict_langs || args.explicit_lang_requested(lang) {
                Err(err)
            } else {
                println!("==> Skipping `{}` benchmark guests ({err})", lang.as_str());
                Ok(false)
            }
        }
    }
}

fn build_rust_guest(
    root: &Path,
    target_dir: &Path,
    guests_dir: &Path,
    profile: GuestProfile,
) -> Result<PathBuf, String> {
    let mut cmd = cargo_cmd(root);
    cmd.args(["build", "-p", "bamabi_bench_guest_rust"]);
    let label = match profile {
        GuestProfile::Release => {
            cmd.arg("--release");
            "cargo build --release -p bamabi_bench_guest_rust"
        }
        GuestProfile::Debug => "cargo build -p bamabi_bench_guest_rust",
    };
    run_cmd(&mut cmd, label)?;

    let built_name = format!(
        "{}bamabi_bench_guest_rust{}",
        env::consts::DLL_PREFIX,
        env::consts::DLL_SUFFIX
    );
    let built_path = target_dir.join(profile.as_str()).join(&built_name);
    if !built_path.is_file() {
        return Err(format!(
            "expected compiled Rust guest library at {}",
            built_path.display()
        ));
    }
    let out_path = guests_dir.join(guest_lib_filename(BenchLang::Rust, profile));
    fs::copy(&built_path, &out_path).map_err(|e| {
        format!(
            "failed to copy {} to {}: {e}",
            built_path.display(),
            out_path.display()
        )
    })?;
    Ok(out_path)
}

#[allow(clippy::too_many_arguments)]
fn build_c_or_cpp_guest(
    root: &Path,
    compiler: &str,
    lang: BenchLang,
    std_flag: &str,
    pedantic_flag: &str,
    src_rel: &str,
    sdk_dir: &Path,
    guests_dir: &Path,
    profile: GuestProfile,
) -> Result<PathBuf, String> {
    let src_path = root.join(src_rel);
    let out_path = guests_dir.join(guest_lib_filename(lang, profile));
    let opt_flags: &[&str] = match profile {
        GuestProfile::Release => &["-O3"],
        GuestProfile::Debug => &["-O0", "-g"],
    };

    let mut cmd = Command::new(compiler);
    cmd.current_dir(root)
        .args([std_flag, "-Wall", "-Wextra", "-Werror", pedantic_flag])
        .args(opt_flags)
        .args(shared_compile_flags())
        .arg("-I")
        .arg(sdk_dir)
        .arg(&src_path)
        .arg("-o")
        .arg(&out_path);

    let label = format!(
        "{compiler} ({lang}:{profile}) {src_rel} -> {}",
        out_path.display(),
        lang = lang.as_str(),
        profile = profile.as_str()
    );
    run_cmd(&mut cmd, &label)?;
    Ok(out_path)
}

fn build_zig_guest(
    root: &Path,
    sdk_dir: &Path,
    guests_dir: &Path,
    profile: GuestProfile,
) -> Result<PathBuf, String> {
    let zig_dir = root.join("benches").join("guests").join("zig_guest");
    let opt_arg = match profile {
        GuestProfile::Release => "-Doptimize=ReleaseFast",
        GuestProfile::Debug => "-Doptimize=Debug",
    };
    let sdk_arg = format!("-Dsdk-dir={}", sdk_dir.display());
    let cache_root = guests_dir.join(format!("zig_cache_{}", profile.as_str()));
    let local_cache = cache_root.join("local");
    let global_cache = cache_root.join("global");
    let out_prefix = guests_dir.join(format!("zig_out_{}", profile.as_str()));
    for dir in [&local_cache, &global_cache, &out_prefix] {
        fs::create_dir_all(dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    }

    let mut cmd = Command::new("zig");
    cmd.current_dir(&zig_dir)
        .env("ZIG_GLOBAL_CACHE_DIR", &global_cache)
        .args(["build", opt_arg, &sdk_arg, "--prefix"])
        .arg(&out_prefix)
        .arg("--cache-dir")
        .arg(&local_cache)
        .arg("--global-cache-dir")
        .arg(&global_cache);
    run_cmd(
        &mut cmd,
        &format!("zig build ({}, {opt_arg})", profile.as_str()),
    )?;

    let built_name = format!(
        "{}bench_zig{}",
        env::consts::DLL_PREFIX,
        env::consts::DLL_SUFFIX
    );
    let lib_candidate = out_prefix.join("lib").join(&built_name);
    let bin_candidate = out_prefix.join("bin").join(&built_name);
    let built_path = if lib_candidate.is_file() {
        lib_candidate
    } else if bin_candidate.is_file() {
        bin_candidate
    } else {
        return Err(format!(
            "expected compiled Zig guest library at {} or {}",
            lib_candidate.display(),
            bin_candidate.display()
        ));
    };

    let out_path = guests_dir.join(guest_lib_filename(BenchLang::Zig, profile));
    fs::copy(&built_path, &out_path).map_err(|e| {
        format!(
            "failed to copy {} to {}: {e}",
            built_path.display(),
            out_path.display()
        )
    })?;
    Ok(out_path)
}

const BENCH_RESULTS_START: &str = "<!-- BENCHMARKS_RESULTS_START -->";
const BENCH_RESULTS_END: &str = "<!-- BENCHMARKS_RESULTS_END -->";

fn extract_bench_results_section(content: &str) -> Result<&str, String> {
    let start_pos = content
        .find(BENCH_RESULTS_START)
        .ok_or_else(|| format!("BENCHMARKS.md is missing `{BENCH_RESULTS_START}` marker"))?;
    let body_start = start_pos + BENCH_RESULTS_START.len();
    let end_rel = content[body_start..]
        .find(BENCH_RESULTS_END)
        .ok_or_else(|| format!("BENCHMARKS.md is missing `{BENCH_RESULTS_END}` marker"))?;
    Ok(&content[body_start..body_start + end_rel])
}

fn update_benchmarks_doc_section(existing: &str, generated_block: &str) -> Result<String, String> {
    let normalized = existing.replace("\r\n", "\n");
    let start_pos = normalized
        .find(BENCH_RESULTS_START)
        .ok_or_else(|| format!("BENCHMARKS.md is missing `{BENCH_RESULTS_START}` marker"))?;
    let body_start = start_pos + BENCH_RESULTS_START.len();
    let end_rel = normalized[body_start..]
        .find(BENCH_RESULTS_END)
        .ok_or_else(|| format!("BENCHMARKS.md is missing `{BENCH_RESULTS_END}` marker"))?;
    let end_pos = body_start + end_rel;

    let mut out = String::with_capacity(normalized.len() + generated_block.len());
    out.push_str(&normalized[..body_start]);
    out.push('\n');
    out.push_str(generated_block.trim());
    out.push('\n');
    out.push_str(&normalized[end_pos..]);
    Ok(out)
}

fn validate_benchmarks_doc(content: &str) -> Result<(), String> {
    let normalized = content.replace("\r\n", "\n");
    let section = extract_bench_results_section(&normalized)?;
    if !section.contains("| Subject") || !section.contains("| ---") {
        return Err(
            "BENCHMARKS.md results section does not contain a formatted Markdown results table"
                .to_owned(),
        );
    }
    let required_subjects = [
        "native",
        "rust:release",
        "rust:debug",
        "c99:release",
        "c99:debug",
        "cpp17:release",
        "cpp17:debug",
        "zig:release",
        "zig:debug",
    ];
    for subject in required_subjects {
        let row_prefix = format!("| {subject}");
        if !section.contains(&row_prefix) {
            return Err(format!(
                "BENCHMARKS.md results section is missing subject `{subject}`"
            ));
        }
    }
    let required_workloads = [
        "noop",
        "dense_query_update",
        "command_burst",
        "host_api_roundtrip",
    ];
    for workload in required_workloads {
        if !section.contains(workload) {
            return Err(format!(
                "BENCHMARKS.md results section is missing workload `{workload}`"
            ));
        }
    }
    Ok(())
}

fn check_benchmarks_doc(root: &Path) -> Result<(), String> {
    let path = root.join("BENCHMARKS.md");
    let content =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    validate_benchmarks_doc(&content)?;
    println!(
        "==> Verified {} contains valid polyglot benchmark results",
        path.display()
    );
    Ok(())
}

fn probe_tool_first_line(program: &str, args: &[&str]) -> String {
    let Ok(output) = Command::new(program)
        .args(args)
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTC")
        .output()
    else {
        return "unavailable".to_owned();
    };
    if !output.status.success() {
        return "unavailable".to_owned();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stdout.trim().is_empty() {
        stderr.as_ref()
    } else {
        stdout.as_ref()
    };
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown")
        .replace('`', "'")
}

fn probe_git_short_sha(root: &Path) -> String {
    let Ok(output) = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    else {
        return "unknown".to_owned();
    };
    if !output.status.success() {
        return "unknown".to_owned();
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if sha.is_empty() {
        "unknown".to_owned()
    } else {
        sha
    }
}

fn format_bench_env_metadata(root: &Path) -> String {
    let commit = probe_git_short_sha(root);
    let cc = c_compiler();
    let cxx = cpp_compiler();
    let rustc_ver = probe_tool_first_line("rustc", &["--version"]);
    let cc_ver = probe_tool_first_line(&cc, &["--version"]);
    let cxx_ver = probe_tool_first_line(&cxx, &["--version"]);
    let zig_ver = probe_tool_first_line("zig", &["version"]);
    format!(
        "- **Commit:** `{commit}`\n\
         - **Host target:** `{os}/{arch}` (`bamabi_bench` built with `--release`)\n\
         - **Rust:** `{rustc_ver}`\n\
         - **C99 (`{cc}`):** `{cc_ver}`\n\
         - **C++17 (`{cxx}`):** `{cxx_ver}`\n\
         - **Zig:** `{zig_ver}`\n",
        os = env::consts::OS,
        arch = env::consts::ARCH,
    )
}

fn cmd_bench(root: &Path, args: &BenchArgs) -> Result<(), String> {
    if args.check_docs {
        return check_benchmarks_doc(root);
    }

    let target_dir = cargo_target_dir(root);
    let sdk_dir = target_dir.join("bench_sdk");
    let guests_dir = target_dir.join("bench_guests");
    fs::create_dir_all(&guests_dir)
        .map_err(|e| format!("failed to create {}: {e}", guests_dir.display()))?;

    println!(
        "==> Exporting polyglot SDK headers to {}",
        sdk_dir.display()
    );
    bamabi_sdk_gen::export_sdk(&sdk_dir)
        .map_err(|e| format!("failed to export SDK to {}: {e}", sdk_dir.display()))?;

    run_cmd(
        cargo_cmd(root).args(["build", "--release", "-p", "bamabi_bench"]),
        "cargo build --release -p bamabi_bench",
    )?;
    let host_bin = target_dir
        .join("release")
        .join(format!("bamabi_bench{}", env::consts::EXE_SUFFIX));
    if !host_bin.is_file() {
        return Err(format!(
            "expected benchmark host binary at {}, but it was not found",
            host_bin.display()
        ));
    }

    let langs = args.resolved_langs();
    let profiles = args.resolved_profiles();
    let mut built_guests: Vec<(BenchLang, GuestProfile, PathBuf)> = Vec::new();

    for &lang in &langs {
        match lang {
            BenchLang::Rust => {
                for &profile in &profiles {
                    let path = build_rust_guest(root, &target_dir, &guests_dir, profile)?;
                    built_guests.push((lang, profile, path));
                }
            }
            BenchLang::C99 => {
                let cc = c_compiler();
                for &profile in &profiles {
                    let path = build_c_or_cpp_guest(
                        root,
                        &cc,
                        BenchLang::C99,
                        "-std=c99",
                        "-pedantic",
                        "benches/guests/c99_guest/bench_guest.c",
                        &sdk_dir,
                        &guests_dir,
                        profile,
                    )?;
                    built_guests.push((lang, profile, path));
                }
            }
            BenchLang::Cpp17 => {
                let cxx = cpp_compiler();
                if ensure_optional_bench_tool(
                    &cxx,
                    &["--version"],
                    "install a C++17 compiler (e.g. g++/clang++) or set $CXX",
                    BenchLang::Cpp17,
                    args,
                )? {
                    for &profile in &profiles {
                        let path = build_c_or_cpp_guest(
                            root,
                            &cxx,
                            BenchLang::Cpp17,
                            "-std=c++17",
                            "-Wpedantic",
                            "benches/guests/cpp17_guest/bench_guest.cpp",
                            &sdk_dir,
                            &guests_dir,
                            profile,
                        )?;
                        built_guests.push((lang, profile, path));
                    }
                }
            }
            BenchLang::Zig => {
                if ensure_optional_bench_tool(
                    "zig",
                    &["version"],
                    "install Zig 0.16.0 (https://ziglang.org/download/)",
                    BenchLang::Zig,
                    args,
                )? {
                    for &profile in &profiles {
                        let path = build_zig_guest(root, &sdk_dir, &guests_dir, profile)?;
                        built_guests.push((lang, profile, path));
                    }
                }
            }
        }
    }

    let mut bench_cmd = Command::new(&host_bin);
    bench_cmd.current_dir(root);
    for (lang, profile, path) in &built_guests {
        bench_cmd.arg("--guest").arg(format!(
            "{}:{}={}",
            lang.as_str(),
            profile.as_str(),
            path.display()
        ));
    }
    if let Some(workloads) = &args.workloads {
        bench_cmd.args(["--workloads", workloads]);
    }
    if let Some(entities) = &args.entities {
        bench_cmd.args(["--entities", entities]);
    }
    if let Some(chunks) = &args.chunks {
        bench_cmd.args(["--chunks", chunks]);
    }
    if args.quick {
        bench_cmd.arg("--quick");
    }
    if let Some(json_path) = &args.json {
        let resolved_json = if json_path.is_absolute() {
            json_path.clone()
        } else {
            root.join(json_path)
        };
        if let Some(parent) = resolved_json.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
        bench_cmd.arg("--json").arg(&resolved_json);
    }

    let markdown_path = target_dir.join("bench-summary.md");
    bench_cmd.arg("--markdown").arg(&markdown_path);

    run_cmd(&mut bench_cmd, "bamabi_bench")?;

    let built_all_default_langs = built_guests.len() == 8;
    let should_write_docs = if args.write_docs {
        true
    } else if args.no_write_docs {
        false
    } else {
        args.is_full_matrix_request() && built_all_default_langs
    };

    if should_write_docs {
        let generated_md = fs::read_to_string(&markdown_path).map_err(|e| {
            format!(
                "failed to read generated Markdown report {}: {e}",
                markdown_path.display()
            )
        })?;
        let doc_path = root.join("BENCHMARKS.md");
        let existing_doc = fs::read_to_string(&doc_path)
            .map_err(|e| format!("failed to read {}: {e}", doc_path.display()))?;
        let block = format!(
            "{}\n{}",
            format_bench_env_metadata(root),
            generated_md.trim()
        );
        let updated_doc = update_benchmarks_doc_section(&existing_doc, &block)?;
        validate_benchmarks_doc(&updated_doc)?;
        fs::write(&doc_path, updated_doc.as_bytes())
            .map_err(|e| format!("failed to write {}: {e}", doc_path.display()))?;
        println!("==> Updated {}", doc_path.display());
    } else if !args.no_write_docs {
        println!(
            "==> Skipped updating BENCHMARKS.md (filtered run or partial language coverage: {}/8 guests)",
            built_guests.len()
        );
    }

    Ok(())
}

fn cmd_ci(root: &Path, args: &CiArgs) -> Result<(), String> {
    cmd_fmt(root, &FmtArgs { fix: false })?;
    cmd_header(root, &CheckArgs { check: true })?;
    component_headers::write_or_check_component_headers(root, true)?;
    check_benchmarks_doc(root)?;
    cmd_lint(root)?;
    cmd_test(root, &TestArgs::default())?;
    cmd_c_test(root, &CTestArgs::default())?;
    if !args.no_bench {
        cmd_bench(
            root,
            &BenchArgs {
                quick: true,
                no_write_docs: true,
                ..Default::default()
            },
        )?;
    }

    if args.audit {
        cmd_audit(root, &AuditArgs::default())?;
    }
    if args.miri {
        cmd_miri(root, &MiriArgs::default())?;
    }
    if args.kani {
        cmd_kani(root, &KaniArgs::default())?;
    }
    println!("==> All CI checks passed!");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    /// Proves that empty args, `-h`/`--help`, `help <topic>`, and `<subcommand> -h`/`--help`
    /// resolve to `ParsedArgs::Help` with the exact target `SubcommandKind` (or `None`).
    #[test]
    fn parses_help_variants() {
        assert_eq!(parse_args(&s(&[])).unwrap(), ParsedArgs::Help(None));
        assert_eq!(parse_args(&s(&["--help"])).unwrap(), ParsedArgs::Help(None));
        assert_eq!(parse_args(&s(&["-h"])).unwrap(), ParsedArgs::Help(None));
        assert_eq!(
            parse_args(&s(&["help", "miri"])).unwrap(),
            ParsedArgs::Help(Some(SubcommandKind::Miri))
        );
        assert_eq!(
            parse_args(&s(&["gen-component-headers", "--help"])).unwrap(),
            ParsedArgs::Help(Some(SubcommandKind::GenComponentHeaders))
        );
        assert_eq!(
            parse_args(&s(&["c-test", "-h"])).unwrap(),
            ParsedArgs::Help(Some(SubcommandKind::CTest))
        );
    }

    /// Proves that `fmt`, `lint`, `header`, `gen-component-headers`, and `c-test` parse
    /// their boolean mode switches (`--fix`, `--check`, `--no-asan`, `--asan-only`) accurately.
    #[test]
    fn parses_fmt_lint_header_and_c_test() {
        assert_eq!(
            parse_args(&s(&["fmt"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Fmt(FmtArgs { fix: false }))
        );
        assert_eq!(
            parse_args(&s(&["fmt", "--fix"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Fmt(FmtArgs { fix: true }))
        );
        assert_eq!(
            parse_args(&s(&["lint"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Lint)
        );
        assert_eq!(
            parse_args(&s(&["header", "--check"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Header(CheckArgs { check: true }))
        );
        assert_eq!(
            parse_args(&s(&["gen-component-headers"])).unwrap(),
            ParsedArgs::Run(CommandSpec::GenComponentHeaders(CheckArgs { check: false }))
        );
        assert_eq!(
            parse_args(&s(&["c-test", "--no-asan"])).unwrap(),
            ParsedArgs::Run(CommandSpec::CTest(CTestArgs {
                no_asan: true,
                asan_only: false,
            }))
        );
        assert_eq!(
            parse_args(&s(&["c-test", "--asan-only"])).unwrap(),
            ParsedArgs::Run(CommandSpec::CTest(CTestArgs {
                no_asan: false,
                asan_only: true,
            }))
        );
    }

    /// Proves that `cargo xtask test` supports both bare default execution and explicit suite
    /// filters alongside `--cases N` and `--cases=N` `PROPTEST_CASES` overrides.
    #[test]
    fn parses_test_filters_and_cases() {
        assert_eq!(
            parse_args(&s(&["test"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Test(TestArgs::default()))
        );
        assert_eq!(
            parse_args(&s(&[
                "test",
                "--cases",
                "64",
                "--proptest",
                "--header-parity"
            ]))
            .unwrap(),
            ParsedArgs::Run(CommandSpec::Test(TestArgs {
                cases: Some(64),
                no_default_features: false,
                all_features: false,
                doc: false,
                proptest: true,
                header_parity: true,
            }))
        );
        assert_eq!(
            parse_args(&s(&[
                "test",
                "--cases=128",
                "--no-default-features",
                "--doc"
            ]))
            .unwrap(),
            ParsedArgs::Run(CommandSpec::Test(TestArgs {
                cases: Some(128),
                no_default_features: true,
                all_features: false,
                doc: true,
                proptest: false,
                header_parity: false,
            }))
        );
    }

    /// Proves that `audit`, `miri`, `kani`, `nightly`, and `ci` flags parse correctly and that
    /// `MiriArgs::resolved` / `NightlyArgs::resolved` expand default flags as documented.
    #[test]
    fn parses_audit_miri_kani_nightly_and_ci() {
        assert_eq!(
            parse_args(&s(&[
                "audit",
                "--baseline-rev",
                "v0.3.0",
                "--skip-targets",
                "--skip-typos"
            ]))
            .unwrap(),
            ParsedArgs::Run(CommandSpec::Audit(AuditArgs {
                baseline_rev: Some("v0.3.0".to_owned()),
                skip_targets: true,
                skip_typos: true,
            }))
        );
        assert_eq!(
            parse_args(&s(&["miri"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Miri(MiriArgs::default()))
        );
        assert_eq!(MiriArgs::default().resolved(), (true, true, false));
        assert_eq!(
            parse_args(&s(&["miri", "--s390x"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Miri(MiriArgs {
                tree_borrows: false,
                stacked_borrows: false,
                s390x: true,
            }))
        );
        assert_eq!(
            parse_args(&s(&[
                "kani",
                "-p",
                "bamabi_sys",
                "--harness=po_w1_abi_constants"
            ]))
            .unwrap(),
            ParsedArgs::Run(CommandSpec::Kani(KaniArgs {
                harness: Some("po_w1_abi_constants".to_owned()),
                packages: vec!["bamabi_sys".to_owned()],
            }))
        );
        assert_eq!(
            parse_args(&s(&["nightly", "--cases", "512"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Nightly(NightlyArgs {
                cases: 512,
                s390x: false,
                randomize_layout: false,
            }))
        );
        assert_eq!(NightlyArgs::default().resolved(), (true, true));
        assert_eq!(
            parse_args(&s(&["nightly", "--randomize-layout"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Nightly(NightlyArgs {
                cases: 2048,
                s390x: false,
                randomize_layout: true,
            }))
        );
        assert_eq!(
            NightlyArgs {
                cases: 2048,
                s390x: false,
                randomize_layout: true,
            }
            .resolved(),
            (false, true)
        );
        assert_eq!(
            parse_args(&s(&["ci", "--full"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Ci(CiArgs {
                audit: true,
                miri: true,
                kani: true,
                no_bench: false,
            }))
        );
        assert_eq!(
            parse_args(&s(&["ci", "--no-bench"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Ci(CiArgs {
                audit: false,
                miri: false,
                kani: false,
                no_bench: true,
            }))
        );
        assert_eq!(
            parse_args(&s(&["ci", "--full", "--no-bench"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Ci(CiArgs {
                audit: true,
                miri: true,
                kani: true,
                no_bench: true,
            }))
        );
    }

    /// Proves that `bench` parses default options, `--flag val` and `--flag=val` forms,
    /// CSV deduplication, help routing, and invalid `bench` flag rejections.
    #[test]
    fn parses_bench_flags_and_rejects_invalid_inputs() {
        assert_eq!(
            parse_args(&s(&["help", "bench"])).unwrap(),
            ParsedArgs::Help(Some(SubcommandKind::Bench))
        );
        assert_eq!(
            parse_args(&s(&["bench", "-h"])).unwrap(),
            ParsedArgs::Help(Some(SubcommandKind::Bench))
        );

        let default_bench = BenchArgs::default();
        assert_eq!(
            parse_args(&s(&["bench"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Bench(BenchArgs::default()))
        );
        assert!(default_bench.is_full_matrix_request());
        assert_eq!(
            default_bench.resolved_langs(),
            vec![
                BenchLang::Rust,
                BenchLang::C99,
                BenchLang::Cpp17,
                BenchLang::Zig
            ]
        );
        assert_eq!(
            default_bench.resolved_profiles(),
            vec![GuestProfile::Release, GuestProfile::Debug]
        );

        let parsed = parse_args(&s(&[
            "bench",
            "--langs",
            "rust,zig,rust",
            "--guest-profiles=release",
            "--workloads=noop,dense_query_update",
            "--entities",
            "1000",
            "--chunks=256",
            "--quick",
            "--strict-langs",
            "--json",
            "target/bench.json",
            "--write-docs",
        ]))
        .unwrap();
        assert_eq!(
            parsed,
            ParsedArgs::Run(CommandSpec::Bench(BenchArgs {
                langs: Some(vec![BenchLang::Rust, BenchLang::Zig]),
                guest_profiles: Some(vec![GuestProfile::Release]),
                workloads: Some("noop,dense_query_update".to_owned()),
                entities: Some("1000".to_owned()),
                chunks: Some("256".to_owned()),
                quick: true,
                strict_langs: true,
                json: Some(PathBuf::from("target/bench.json")),
                write_docs: true,
                no_write_docs: false,
                check_docs: false,
            }))
        );
        assert_eq!(
            parse_args(&s(&["bench", "--check-docs"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Bench(BenchArgs {
                check_docs: true,
                ..Default::default()
            }))
        );
        assert_eq!(
            parse_args(&s(&["bench", "--no-write-docs"])).unwrap(),
            ParsedArgs::Run(CommandSpec::Bench(BenchArgs {
                no_write_docs: true,
                ..Default::default()
            }))
        );
        if let ParsedArgs::Run(CommandSpec::Bench(ref bench_args)) = parsed {
            assert!(bench_args.explicit_lang_requested(BenchLang::Zig));
            assert!(!bench_args.explicit_lang_requested(BenchLang::C99));
            assert!(!bench_args.is_full_matrix_request());
        } else {
            panic!("expected CommandSpec::Bench");
        }

        assert!(parse_args(&s(&["bench", "--langs=rust,go"])).is_err());
        assert!(parse_args(&s(&["bench", "--langs="])).is_err());
        assert!(parse_args(&s(&["bench", "--langs", "rust,"])).is_err());
        assert!(parse_args(&s(&["bench", "--guest-profiles=fast"])).is_err());
        assert!(parse_args(&s(&["bench", "--workloads"])).is_err());
        assert!(parse_args(&s(&["bench", "--entities="])).is_err());
        assert!(parse_args(&s(&["bench", "--chunks="])).is_err());
        assert!(parse_args(&s(&["bench", "--json", ""])).is_err());
        assert!(parse_args(&s(&["bench", "--quick=true"])).is_err());
        assert!(parse_args(&s(&["bench", "--write-docs", "--check-docs"])).is_err());
        assert!(parse_args(&s(&["bench", "--write-docs", "--no-write-docs"])).is_err());
        assert!(parse_args(&s(&["bench", "--write-docs=true"])).is_err());
        assert!(parse_args(&s(&["bench", "--no-write-docs=true"])).is_err());
        assert!(parse_args(&s(&["bench", "--check-docs=true"])).is_err());
        assert!(parse_args(&s(&["bench", "--unknown"])).is_err());
    }

    /// Proves that `BENCHMARKS.md` marker splicing, structural validation, and the checked-in
    /// `BENCHMARKS.md` document stay consistent.
    #[test]
    fn validates_and_updates_benchmarks_doc_markers() {
        let template = "\
# Benchmarks
Intro
<!-- BENCHMARKS_RESULTS_START -->
old content
<!-- BENCHMARKS_RESULTS_END -->
Footer
";
        let valid_block = "\
### bamabi-wire ABI 0.3.0 Polyglot Benchmark (quick, warmup=5, samples=20)

| Subject | Workload | Entities | Chunk | Min (ns) | Median (ns) | P95 (ns) | Mean (ns) | ns/entity | M ent/s | vs Native |
| ------- | -------- | -------: | ----: | -------: | ----------: | -------: | --------: | --------: | ------: | --------: |
| native | noop | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |
| native | dense_query_update | 1000 | 256 | 1.0 | 1.0 | 1.0 | 1.0 | 0.1 | 1000.0 | 1.00x |
| native | command_burst | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |
| native | host_api_roundtrip | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |
| rust:release | noop | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |
| rust:debug | noop | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |
| c99:release | noop | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |
| c99:debug | noop | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |
| cpp17:release | noop | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |
| cpp17:debug | noop | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |
| zig:release | noop | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |
| zig:debug | noop | - | - | 1.0 | 1.0 | 1.0 | 1.0 | - | - | 1.00x |";
        let updated = update_benchmarks_doc_section(template, valid_block).unwrap();
        assert!(updated.contains("Intro"));
        assert!(updated.contains("Footer"));
        assert!(!updated.contains("old content"));
        validate_benchmarks_doc(&updated).unwrap();

        assert!(update_benchmarks_doc_section("no markers", valid_block).is_err());
        assert!(validate_benchmarks_doc("no markers").is_err());
        let missing_zig = updated.replace("| zig:debug", "| omitted:debug");
        assert!(validate_benchmarks_doc(&missing_zig).is_err());

        let checked_in = workspace_root().unwrap().join("BENCHMARKS.md");
        let content = fs::read_to_string(&checked_in)
            .expect("checked-in BENCHMARKS.md must exist at workspace root");
        validate_benchmarks_doc(&content).expect("checked-in BENCHMARKS.md must pass validation");
    }

    /// Proves that unknown subcommands, unexpected extra arguments, conflicting `--no-asan` /
    /// `--asan-only` flags, zero `--cases`, missing flag values, and empty `--flag=` values fail
    /// parsing cleanly before spawning subprocesses.
    #[test]
    fn rejects_invalid_subcommands_and_flags() {
        assert!(parse_args(&s(&["unknown"])).is_err());
        assert!(parse_args(&s(&["lint", "--extra"])).is_err());
        assert!(parse_args(&s(&["c-test", "--no-asan", "--asan-only"])).is_err());
        assert!(parse_args(&s(&["test", "--cases", "0"])).is_err());
        assert!(parse_args(&s(&["test", "--cases"])).is_err());
        assert!(parse_args(&s(&["audit", "--baseline-rev="])).is_err());
        assert!(parse_args(&s(&["kani", "-p"])).is_err());
        assert!(parse_args(&s(&["ci", "--bogus"])).is_err());
    }
}
