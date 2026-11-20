# `bamabi_guest_sys`

Low-level, **zero-Bevy-dependency** guest FFI runtime for Rust `.cdylib` mods targeting `bamabi` (`ABI 0.3.0`, `v0.3.0`, Layer 0 in [`bamabi-wire`](../../README.md)).

Isolates all guest-side `unsafe` code (`#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks, clippy::missing_safety_doc)]`) so [`bamabi_guest`](../bamabi_guest) enforces `#![forbid(unsafe_code)]`:

- **`Host` & `ValidatedLoadCtx`**: Bounded `BamHostApi` copy, `ABI 0.3.0` SemVer verification, checked `covers_slot` + `UNSUPPORTED`-fallback wrappers for all **68/68** host slots (including all 18 ABI `0.3.0` slots), `"bam.v0.*"` custom symbol lookup, and `VTableHeader` checks.
- **`GuestInstance`**: Per-World heap state owner with deterministic LIFO closure teardown and hot-reload `save_state` / `restore_state` trampolines.
- **`panic` & `ReentrantCell`**: `extern "C"` (`nounwind`) panic guards (`catch_guest_result`, `catch_guest_bool`, `catch_guest_unit`) paired with lock-free `UnsafeCell` + `AtomicBool` re-entrancy guards so unwind payloads are dropped strictly on the guest heap.
- **Validated Iterators, Commands & Migrations**: Layout-, alignment-, UUID-, and length-checked wrappers for dense columns, sparse rows, `iter_count`, `iter_up_cascade`, `par_for_each_chunk_readonly`, `GuestRes` change ticks, deferred commands & checkpoints (`cmd_checkpoint` / `cmd_rollback`, `cmd_insert_relation` / `cmd_remove_relation`), panic-guarded `migrate_component_column` (`BamRowMigrateFn`), dynamic component drop/hook trampolines, and observers.

Mod authors should depend on [`bamabi_guest`](../bamabi_guest) rather than calling this `-sys` crate directly.

## Standalone Usage

```toml
[dependencies]
bamabi_guest_sys = { git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main" }
```

## Verification

```bash
cargo clippy -p bamabi_guest_sys --all-targets --all-features -- -D warnings
cargo test -p bamabi_guest_sys --all-features
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_guest_sys --no-deps --all-features
MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest_sys
```

## License

Dual-licensed under `MIT OR Apache-2.0` ([`LICENSE-MIT`](../../LICENSE-MIT) and [`LICENSE-APACHE`](../../LICENSE-APACHE)).
