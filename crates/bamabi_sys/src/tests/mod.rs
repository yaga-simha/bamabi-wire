//! Unit and regression tests for `bamabi_sys`.
//!
//! Covers ABI `0.2` / `0.3` layout parity (`size_of`, `align_of`, `offset_of!`), `ExtensibleStruct`
//! prefix reading and truncation, flag/reserved validation (`PO-W5`), `ABI_VERSION` packing and
//! SemVer compatibility, `FfiResult` taxonomy and `Display`, SipHash-2-4-128 test vectors,
//! `FfiSlice`/`StrView` Miri safety, `AbiPod`/`RawInsertable`/`ModComponent`, and ABI `0.3.0`
//! descriptors (`BamFieldDesc`, `BamRequiredComponent`, `BamRelationDesc`, `BamLayoutInfo`,
//! `BamScheduleSetDesc`).

mod layout;
