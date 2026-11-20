//! Property-based tests for `bamabi_guest_sys` (`ChunkColumnDispenser`,
//! `ValidatedIter::read_messages_batch_checked`, `SparseRows`, and `SparseRowsMut`).

mod common;

use bamabi_guest_sys::{Host, SparseRows, SparseRowsMut, UuidFilter, ValidatedIter};
use bamabi_sys::{
    ABI_VERSION, AbiPod, AccessMode, BAM_STORAGE_FLAG_MESSAGE, BAM_STORAGE_FLAG_RESOURCE,
    BamColumnInfo, BamComponent, BamHostApi, BamIter, ComponentUuid, FfiBool, FfiEntity, FfiResult,
    MAX_TERMS_PER_QUERY, RawInsertable,
};
use common::make_mock_api;
use proptest::prelude::*;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ZstPod;
// SAFETY: Zero-sized marker type with no drop glue or uninitialized bytes.
unsafe impl RawInsertable for ZstPod {}
// SAFETY: Zero-sized marker type with deterministic C ABI layout.
unsafe impl AbiPod for ZstPod {}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MsgPod {
    seq: u64,
    tag: u32,
    pad: u32,
}
// SAFETY: `#[repr(C)]` POD struct with explicit initialized padding and no drop glue.
unsafe impl RawInsertable for MsgPod {}
// SAFETY: All fields are `AbiPod` with deterministic C layout.
unsafe impl AbiPod for MsgPod {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ElemKind {
    U8,
    U32,
    U64,
    Zst,
}

impl ElemKind {
    const fn size(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U32 => 4,
            Self::U64 => 8,
            Self::Zst => 0,
        }
    }

    const fn align(self) -> usize {
        match self {
            Self::U8 | Self::Zst => 1,
            Self::U32 => 4,
            Self::U64 => 8,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum BorrowOp {
    Read {
        comp: u32,
        byte_offset: usize,
        kind: ElemKind,
    },
    Write {
        comp: u32,
        byte_offset: usize,
        kind: ElemKind,
    },
    OptionalRead {
        comp: u32,
        byte_offset: usize,
        kind: ElemKind,
        is_null: bool,
    },
    OptionalWrite {
        comp: u32,
        byte_offset: usize,
        kind: ElemKind,
        is_null: bool,
    },
    Entities {
        byte_offset: usize,
    },
}

#[derive(Clone, Copy, Debug)]
struct PlannedTerm {
    comp: BamComponent,
    mode: AccessMode,
    uuid: ComponentUuid,
    size: usize,
    align: usize,
    ptr: *mut u8,
}

struct DispenserMockState {
    chunk_len: usize,
    yielded: bool,
    terms: Vec<PlannedTerm>,
    entity_ptrs: Vec<*const FfiEntity>,
    entity_call_idx: usize,
}

/// Mock `iter_next_chunk` returning a single chunk of `chunk_len` rows.
unsafe extern "C" fn disp_next_chunk(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
    // SAFETY: Caller passes pointer to `DispenserMockState`.
    let st = unsafe { &mut *iter.cast::<DispenserMockState>() };
    if st.yielded {
        return FfiBool::FALSE;
    }
    st.yielded = true;
    // SAFETY: `out_len` is writable stack storage provided by `ValidatedIter::next_chunk`.
    unsafe { *out_len = st.chunk_len };
    FfiBool::TRUE
}

/// Mock `iter_entities` returning the pre-planned entity pointer for each `entities()` call.
unsafe extern "C" fn disp_entities(iter: *const BamIter) -> *const FfiEntity {
    // SAFETY: Test binds `iter` to `DispenserMockState`; mutating only `entity_call_idx` avoids
    // aliasing the separate heap buffer backing entity/column payloads.
    let st = unsafe { &mut *iter.cast_mut().cast::<DispenserMockState>() };
    let idx = st.entity_call_idx;
    st.entity_call_idx += 1;
    st.entity_ptrs
        .get(idx)
        .copied()
        .unwrap_or_else(core::ptr::null)
}

/// Mock `iter_column_info` returning the descriptor for `term`.
unsafe extern "C" fn disp_col_info(
    iter: *const BamIter,
    term: usize,
    out: *mut BamColumnInfo,
) -> FfiResult {
    // SAFETY: `DispenserMockState.terms` is immutable during chunk iteration.
    let st = unsafe { &*iter.cast::<DispenserMockState>() };
    let Some(plan) = st.terms.get(term) else {
        return FfiResult::OUT_OF_RANGE;
    };
    // SAFETY: `out` is writable stack storage.
    unsafe {
        *out = BamColumnInfo {
            component: plan.comp,
            mode: plan.mode,
            uuid: plan.uuid,
            size: plan.size,
            align: plan.align,
            storage_flags: 0,
            reserved0: 0,
        };
    }
    FfiResult::OK
}

/// Mock `iter_column_readonly` returning the pre-planned pointer without retagging the pool.
unsafe extern "C" fn disp_col_ro(iter: *const BamIter, term: usize) -> *const u8 {
    // SAFETY: `DispenserMockState.terms` is immutable during chunk iteration.
    let st = unsafe { &*iter.cast::<DispenserMockState>() };
    st.terms
        .get(term)
        .map_or(core::ptr::null(), |p| p.ptr.cast_const())
}

/// Mock `iter_column_mutable` returning the pre-planned pointer without retagging the pool.
unsafe extern "C" fn disp_col_mut(iter: *mut BamIter, term: usize) -> *mut u8 {
    // SAFETY: `DispenserMockState.terms` is immutable during chunk iteration.
    let st = unsafe { &*iter.cast::<DispenserMockState>() };
    st.terms.get(term).map_or(core::ptr::null_mut(), |p| p.ptr)
}

#[derive(Clone, Copy, Debug)]
struct OracleBorrow {
    comp: u32,
    start: usize,
    end: usize,
    is_mut: bool,
}

fn intervals_overlap(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> bool {
    a_end > a_start && b_end > b_start && a_start < b_end && b_start < a_end
}

fn borrow_op_strategy() -> impl Strategy<Value = BorrowOp> {
    let kind_strat = prop_oneof![
        Just(ElemKind::U8),
        Just(ElemKind::U32),
        Just(ElemKind::U64),
        Just(ElemKind::Zst),
    ];
    prop_oneof![
        3 => (1u32..=12, 0usize..=16, kind_strat.clone()).prop_map(|(comp, slot, kind)| {
            BorrowOp::Read {
                comp,
                byte_offset: slot * 8,
                kind,
            }
        }),
        3 => (1u32..=12, 0usize..=16, kind_strat.clone()).prop_map(|(comp, slot, kind)| {
            BorrowOp::Write {
                comp,
                byte_offset: slot * 8,
                kind,
            }
        }),
        2 => (1u32..=12, 0usize..=16, kind_strat.clone(), any::<bool>()).prop_map(
            |(comp, slot, kind, is_null)| BorrowOp::OptionalRead {
                comp,
                byte_offset: slot * 8,
                kind,
                is_null,
            }
        ),
        2 => (1u32..=12, 0usize..=16, kind_strat, any::<bool>()).prop_map(
            |(comp, slot, kind, is_null)| BorrowOp::OptionalWrite {
                comp,
                byte_offset: slot * 8,
                kind,
                is_null,
            }
        ),
        2 => (0usize..=16).prop_map(|slot| BorrowOp::Entities {
            byte_offset: slot * 8,
        }),
    ]
}

fn check_oracle_borrow(
    oracle: &[OracleBorrow],
    entities_range: Option<(usize, usize)>,
    comp: u32,
    start: usize,
    byte_len: usize,
    is_mut: bool,
) -> Result<(), FfiResult> {
    let end = start.checked_add(byte_len).ok_or(FfiResult::OUT_OF_RANGE)?;
    if is_mut
        && byte_len > 0
        && let Some((ent_start, ent_end)) = entities_range
        && intervals_overlap(start, end, ent_start, ent_end)
    {
        return Err(FfiResult::ACCESS_CONFLICT);
    }
    for prev in oracle {
        if is_mut || prev.is_mut {
            if prev.comp == comp {
                return Err(FfiResult::ACCESS_CONFLICT);
            }
            if intervals_overlap(start, end, prev.start, prev.end) {
                return Err(FfiResult::ACCESS_CONFLICT);
            }
        }
    }
    if oracle.len() >= MAX_TERMS_PER_QUERY {
        return Err(FfiResult::CAPACITY_EXCEEDED);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct HeldRecord {
    comp: u32,
    start: usize,
    end: usize,
    is_mut: bool,
}

enum HeldRead<'a> {
    U8(&'a [u8]),
    U32(&'a [u32]),
    U64(&'a [u64]),
    Zst(&'a [ZstPod]),
    Entities(&'a [FfiEntity]),
}

fn proptest_config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig {
            cases: 4,
            failure_persistence: None,
            ..ProptestConfig::default()
        }
    } else {
        ProptestConfig::with_cases(512)
    }
}

enum HeldWrite<'a> {
    U8(&'a mut [u8]),
    U32(&'a mut [u32]),
    U64(&'a mut [u64]),
    Zst(&'a mut [ZstPod]),
}

proptest! {
    #![proptest_config(proptest_config())]

    #[test]
    fn chunk_column_dispenser_random_sequences_never_alias_mutably(
        chunk_len in 0usize..=8,
        ops in prop::collection::vec(borrow_op_strategy(), 1..=40),
    ) {
        // Backing pool of 256 u64s (2048 bytes), stored in an independent raw heap allocation so
        // mock FFI callbacks never create `&mut DispenserMockState` aliases over active slices.
        const POOL_U64S: usize = 256;
        let pool_box: Box<[u64]> = vec![0u64; POOL_U64S].into_boxed_slice();
        let pool_raw: *mut [u64] = Box::into_raw(pool_box);
        let pool_base: *mut u8 = pool_raw.cast::<u8>();

        let mut terms = Vec::new();
        let mut entity_ptrs = Vec::new();

        for op in &ops {
            match *op {
                BorrowOp::Read { comp, byte_offset, kind } => {
                    // SAFETY: `byte_offset + 8 * 8 <= 2048` for all generated offsets/chunk_len.
                    let ptr = unsafe { pool_base.add(byte_offset) };
                    terms.push(PlannedTerm {
                        comp: BamComponent::from_raw(comp),
                        mode: AccessMode::READ,
                        uuid: ComponentUuid::from_parts("prop::Read", kind.size(), kind.align(), 0),
                        size: kind.size(),
                        align: kind.align(),
                        ptr,
                    });
                }
                BorrowOp::Write { comp, byte_offset, kind } => {
                    // SAFETY: Within the 2048-byte pool allocation.
                    let ptr = unsafe { pool_base.add(byte_offset) };
                    terms.push(PlannedTerm {
                        comp: BamComponent::from_raw(comp),
                        mode: AccessMode::WRITE,
                        uuid: ComponentUuid::from_parts("prop::Write", kind.size(), kind.align(), 0),
                        size: kind.size(),
                        align: kind.align(),
                        ptr,
                    });
                }
                BorrowOp::OptionalRead { comp, byte_offset, kind, is_null } => {
                    let ptr = if is_null {
                        core::ptr::null_mut()
                    } else {
                        // SAFETY: Within the 2048-byte pool allocation.
                        unsafe { pool_base.add(byte_offset) }
                    };
                    terms.push(PlannedTerm {
                        comp: BamComponent::from_raw(comp),
                        mode: AccessMode::OPTIONAL_READ,
                        uuid: ComponentUuid::from_parts("prop::OptRead", kind.size(), kind.align(), 0),
                        size: kind.size(),
                        align: kind.align(),
                        ptr,
                    });
                }
                BorrowOp::OptionalWrite { comp, byte_offset, kind, is_null } => {
                    let ptr = if is_null {
                        core::ptr::null_mut()
                    } else {
                        // SAFETY: Within the 2048-byte pool allocation.
                        unsafe { pool_base.add(byte_offset) }
                    };
                    terms.push(PlannedTerm {
                        comp: BamComponent::from_raw(comp),
                        mode: AccessMode::OPTIONAL_WRITE,
                        uuid: ComponentUuid::from_parts("prop::OptWrite", kind.size(), kind.align(), 0),
                        size: kind.size(),
                        align: kind.align(),
                        ptr,
                    });
                }
                BorrowOp::Entities { byte_offset } => {
                    // SAFETY: `byte_offset` is a multiple of 8 within the 256-u64 pool allocation.
                    let ptr = unsafe { pool_raw.cast::<u64>().add(byte_offset / 8) }
                        .cast::<FfiEntity>()
                        .cast_const();
                    entity_ptrs.push(ptr);
                }
            }
        }

        let mut api: BamHostApi = make_mock_api();
        api.iter_next_chunk = Some(disp_next_chunk);
        api.iter_entities = Some(disp_entities);
        api.iter_column_info = Some(disp_col_info);
        api.iter_column_readonly = Some(disp_col_ro);
        api.iter_column_mutable = Some(disp_col_mut);

        let mut state = DispenserMockState {
            chunk_len,
            yielded: false,
            terms,
            entity_ptrs,
            entity_call_idx: 0,
        };

        {
            // SAFETY: `api` and `state` stay alive until the end of this block.
            let host = unsafe { Host::from_raw(&api, (&raw const state).cast()) }.unwrap();
            // SAFETY: `state` implements the mock `BamIter` callbacks in `api`.
            let mut iter = unsafe { ValidatedIter::from_raw((&raw mut state).cast(), &host) };
            prop_assert_eq!(iter.next_chunk(), Some(chunk_len));

            let mut disp = bamabi_guest_sys::ChunkColumnDispenser::new(&mut iter);
            let mut oracle: Vec<OracleBorrow> = Vec::new();
            let mut oracle_entities: Option<(usize, usize)> = None;
            let mut held_records: Vec<HeldRecord> = Vec::new();
            let mut held_reads: Vec<HeldRead<'_>> = Vec::new();
            let mut held_writes: Vec<HeldWrite<'_>> = Vec::new();
            let mut ent_cursor = 0usize;

            for op in &ops {
                match *op {
                    BorrowOp::Entities { byte_offset } => {
                        if chunk_len == 0 {
                            let ents = disp.entities().unwrap();
                            prop_assert!(ents.is_empty());
                            held_reads.push(HeldRead::Entities(ents));
                            continue;
                        }
                        let byte_len = chunk_len * core::mem::size_of::<FfiEntity>();
                        let start = pool_base.addr() + byte_offset;
                        let end = start + byte_len;
                        let mut expected_err = None;
                        if let Some(prev) = oracle_entities
                            && prev != (start, end)
                        {
                            expected_err = Some(FfiResult::ACCESS_CONFLICT);
                        }
                        if expected_err.is_none() {
                            for prev in &oracle {
                                if prev.is_mut && intervals_overlap(start, end, prev.start, prev.end) {
                                    expected_err = Some(FfiResult::ACCESS_CONFLICT);
                                    break;
                                }
                            }
                        }
                        let res = disp.entities();
                        if let Some(err) = expected_err {
                            prop_assert_eq!(res, Err(err));
                        } else {
                            let ents = res.unwrap();
                            prop_assert_eq!(ents.len(), chunk_len);
                            oracle_entities = Some((start, end));
                            held_reads.push(HeldRead::Entities(ents));
                        }
                        ent_cursor += 1;
                    }
                    BorrowOp::Read { comp, byte_offset, kind } => {
                        let byte_len = chunk_len * kind.size();
                        let start = if byte_len == 0 { 0 } else { pool_base.addr() + byte_offset };
                        let expected = check_oracle_borrow(
                            &oracle,
                            oracle_entities,
                            comp,
                            start,
                            byte_len,
                            false,
                        );
                        match kind {
                            ElemKind::U8 => {
                                let res = disp.next_read::<u8>(AccessMode::READ, None);
                                prop_assert_eq!(res.as_ref().map(|s| s.len()).map_err(|e| *e), expected.map(|()| chunk_len));
                                if let Ok(slice) = res {
                                    held_reads.push(HeldRead::U8(slice));
                                }
                            }
                            ElemKind::U32 => {
                                let res = disp.next_read::<u32>(AccessMode::READ, None);
                                prop_assert_eq!(res.as_ref().map(|s| s.len()).map_err(|e| *e), expected.map(|()| chunk_len));
                                if let Ok(slice) = res {
                                    held_reads.push(HeldRead::U32(slice));
                                }
                            }
                            ElemKind::U64 => {
                                let res = disp.next_read::<u64>(AccessMode::READ, None);
                                prop_assert_eq!(res.as_ref().map(|s| s.len()).map_err(|e| *e), expected.map(|()| chunk_len));
                                if let Ok(slice) = res {
                                    held_reads.push(HeldRead::U64(slice));
                                }
                            }
                            ElemKind::Zst => {
                                let res = disp.next_read::<ZstPod>(AccessMode::READ, None);
                                prop_assert_eq!(res.as_ref().map(|s| s.len()).map_err(|e| *e), expected.map(|()| chunk_len));
                                if let Ok(slice) = res {
                                    held_reads.push(HeldRead::Zst(slice));
                                }
                            }
                        }
                        if expected.is_ok() {
                            let rec = OracleBorrow {
                                comp,
                                start,
                                end: start + byte_len,
                                is_mut: false,
                            };
                            oracle.push(rec);
                            held_records.push(HeldRecord {
                                comp,
                                start: rec.start,
                                end: rec.end,
                                is_mut: false,
                            });
                        }
                    }
                    BorrowOp::Write { comp, byte_offset, kind } => {
                        let byte_len = chunk_len * kind.size();
                        let start = if byte_len == 0 { 0 } else { pool_base.addr() + byte_offset };
                        let expected = check_oracle_borrow(
                            &oracle,
                            oracle_entities,
                            comp,
                            start,
                            byte_len,
                            true,
                        );
                        match kind {
                            ElemKind::U8 => {
                                let res = disp.next_write::<u8>(None);
                                prop_assert_eq!(res.as_ref().map(|s| s.len()).map_err(|e| *e), expected.map(|()| chunk_len));
                                if let Ok(slice) = res {
                                    held_writes.push(HeldWrite::U8(slice));
                                }
                            }
                            ElemKind::U32 => {
                                let res = disp.next_write::<u32>(None);
                                prop_assert_eq!(res.as_ref().map(|s| s.len()).map_err(|e| *e), expected.map(|()| chunk_len));
                                if let Ok(slice) = res {
                                    held_writes.push(HeldWrite::U32(slice));
                                }
                            }
                            ElemKind::U64 => {
                                let res = disp.next_write::<u64>(None);
                                prop_assert_eq!(res.as_ref().map(|s| s.len()).map_err(|e| *e), expected.map(|()| chunk_len));
                                if let Ok(slice) = res {
                                    held_writes.push(HeldWrite::U64(slice));
                                }
                            }
                            ElemKind::Zst => {
                                let res = disp.next_write::<ZstPod>(None);
                                prop_assert_eq!(res.as_ref().map(|s| s.len()).map_err(|e| *e), expected.map(|()| chunk_len));
                                if let Ok(slice) = res {
                                    held_writes.push(HeldWrite::Zst(slice));
                                }
                            }
                        }
                        if expected.is_ok() {
                            let rec = OracleBorrow {
                                comp,
                                start,
                                end: start + byte_len,
                                is_mut: true,
                            };
                            oracle.push(rec);
                            held_records.push(HeldRecord {
                                comp,
                                start: rec.start,
                                end: rec.end,
                                is_mut: true,
                            });
                        }
                    }
                    BorrowOp::OptionalRead { comp, byte_offset, kind, is_null } => {
                        if is_null {
                            match kind {
                                ElemKind::U8 => prop_assert!(disp.next_optional_read::<u8>(None).unwrap().is_none()),
                                ElemKind::U32 => prop_assert!(disp.next_optional_read::<u32>(None).unwrap().is_none()),
                                ElemKind::U64 => prop_assert!(disp.next_optional_read::<u64>(None).unwrap().is_none()),
                                ElemKind::Zst => prop_assert!(disp.next_optional_read::<ZstPod>(None).unwrap().is_none()),
                            }
                            continue;
                        }
                        let byte_len = chunk_len * kind.size();
                        let start = if byte_len == 0 { 0 } else { pool_base.addr() + byte_offset };
                        let expected = check_oracle_borrow(
                            &oracle,
                            oracle_entities,
                            comp,
                            start,
                            byte_len,
                            false,
                        );
                        match kind {
                            ElemKind::U8 => {
                                let res = disp.next_optional_read::<u8>(None);
                                prop_assert_eq!(res.as_ref().map(|o| o.as_ref().map(|s| s.len())).map_err(|e| *e), expected.map(|()| Some(chunk_len)));
                                if let Ok(Some(slice)) = res {
                                    held_reads.push(HeldRead::U8(slice));
                                }
                            }
                            ElemKind::U32 => {
                                let res = disp.next_optional_read::<u32>(None);
                                prop_assert_eq!(res.as_ref().map(|o| o.as_ref().map(|s| s.len())).map_err(|e| *e), expected.map(|()| Some(chunk_len)));
                                if let Ok(Some(slice)) = res {
                                    held_reads.push(HeldRead::U32(slice));
                                }
                            }
                            ElemKind::U64 => {
                                let res = disp.next_optional_read::<u64>(None);
                                prop_assert_eq!(res.as_ref().map(|o| o.as_ref().map(|s| s.len())).map_err(|e| *e), expected.map(|()| Some(chunk_len)));
                                if let Ok(Some(slice)) = res {
                                    held_reads.push(HeldRead::U64(slice));
                                }
                            }
                            ElemKind::Zst => {
                                let res = disp.next_optional_read::<ZstPod>(None);
                                prop_assert_eq!(res.as_ref().map(|o| o.as_ref().map(|s| s.len())).map_err(|e| *e), expected.map(|()| Some(chunk_len)));
                                if let Ok(Some(slice)) = res {
                                    held_reads.push(HeldRead::Zst(slice));
                                }
                            }
                        }
                        if expected.is_ok() {
                            let rec = OracleBorrow {
                                comp,
                                start,
                                end: start + byte_len,
                                is_mut: false,
                            };
                            oracle.push(rec);
                            held_records.push(HeldRecord {
                                comp,
                                start: rec.start,
                                end: rec.end,
                                is_mut: false,
                            });
                        }
                    }
                    BorrowOp::OptionalWrite { comp, byte_offset, kind, is_null } => {
                        if is_null {
                            match kind {
                                ElemKind::U8 => prop_assert!(disp.next_optional_write::<u8>(None).unwrap().is_none()),
                                ElemKind::U32 => prop_assert!(disp.next_optional_write::<u32>(None).unwrap().is_none()),
                                ElemKind::U64 => prop_assert!(disp.next_optional_write::<u64>(None).unwrap().is_none()),
                                ElemKind::Zst => prop_assert!(disp.next_optional_write::<ZstPod>(None).unwrap().is_none()),
                            }
                            continue;
                        }
                        let byte_len = chunk_len * kind.size();
                        let start = if byte_len == 0 { 0 } else { pool_base.addr() + byte_offset };
                        let expected = check_oracle_borrow(
                            &oracle,
                            oracle_entities,
                            comp,
                            start,
                            byte_len,
                            true,
                        );
                        match kind {
                            ElemKind::U8 => {
                                let res = disp.next_optional_write::<u8>(None);
                                prop_assert_eq!(res.as_ref().map(|o| o.as_ref().map(|s| s.len())).map_err(|e| *e), expected.map(|()| Some(chunk_len)));
                                if let Ok(Some(slice)) = res {
                                    held_writes.push(HeldWrite::U8(slice));
                                }
                            }
                            ElemKind::U32 => {
                                let res = disp.next_optional_write::<u32>(None);
                                prop_assert_eq!(res.as_ref().map(|o| o.as_ref().map(|s| s.len())).map_err(|e| *e), expected.map(|()| Some(chunk_len)));
                                if let Ok(Some(slice)) = res {
                                    held_writes.push(HeldWrite::U32(slice));
                                }
                            }
                            ElemKind::U64 => {
                                let res = disp.next_optional_write::<u64>(None);
                                prop_assert_eq!(res.as_ref().map(|o| o.as_ref().map(|s| s.len())).map_err(|e| *e), expected.map(|()| Some(chunk_len)));
                                if let Ok(Some(slice)) = res {
                                    held_writes.push(HeldWrite::U64(slice));
                                }
                            }
                            ElemKind::Zst => {
                                let res = disp.next_optional_write::<ZstPod>(None);
                                prop_assert_eq!(res.as_ref().map(|o| o.as_ref().map(|s| s.len())).map_err(|e| *e), expected.map(|()| Some(chunk_len)));
                                if let Ok(Some(slice)) = res {
                                    held_writes.push(HeldWrite::Zst(slice));
                                }
                            }
                        }
                        if expected.is_ok() {
                            let rec = OracleBorrow {
                                comp,
                                start,
                                end: start + byte_len,
                                is_mut: true,
                            };
                            oracle.push(rec);
                            held_records.push(HeldRecord {
                                comp,
                                start: rec.start,
                                end: rec.end,
                                is_mut: true,
                            });
                        }
                    }
                }
            }
            let _ = ent_cursor;

            // Verify zero aliasing mutable borrows across ALL dispensed slices and entity ranges.
            prop_assert!(held_records.len() <= MAX_TERMS_PER_QUERY);
            for (i, a) in held_records.iter().enumerate() {
                if a.is_mut {
                    if let Some((ent_start, ent_end)) = oracle_entities {
                        prop_assert!(!intervals_overlap(a.start, a.end, ent_start, ent_end));
                    }
                    for (j, b) in held_records.iter().enumerate() {
                        if i != j {
                            prop_assert_ne!(a.comp, b.comp);
                            prop_assert!(!intervals_overlap(a.start, a.end, b.start, b.end));
                        }
                    }
                }
            }

            // Exercise all simultaneously live mutable and shared borrows to confirm zero aliasing.
            for (idx, w) in held_writes.iter_mut().enumerate() {
                match w {
                    HeldWrite::U8(s) => {
                        for item in s.iter_mut() {
                            *item = (idx as u8).wrapping_add(1);
                        }
                    }
                    HeldWrite::U32(s) => {
                        for item in s.iter_mut() {
                            *item = (idx as u32).wrapping_add(1);
                        }
                    }
                    HeldWrite::U64(s) => {
                        for item in s.iter_mut() {
                            *item = (idx as u64).wrapping_add(1);
                        }
                    }
                    HeldWrite::Zst(s) => {
                        for item in s.iter_mut() {
                            *item = ZstPod;
                        }
                    }
                }
            }
            for r in &held_reads {
                match r {
                    HeldRead::U8(s) => prop_assert_eq!(s.len(), chunk_len),
                    HeldRead::U32(s) => prop_assert_eq!(s.len(), chunk_len),
                    HeldRead::U64(s) => prop_assert_eq!(s.len(), chunk_len),
                    HeldRead::Zst(s) => prop_assert_eq!(s.len(), chunk_len),
                    HeldRead::Entities(s) => prop_assert_eq!(s.len(), chunk_len),
                }
            }
        }

        // SAFETY: `pool_raw` was created via `Box::into_raw` and all borrows tied to `iter` ended above.
        drop(unsafe { Box::from_raw(pool_raw) });
    }
}

struct MsgBatchHostState {
    mode: AccessMode,
    uuid: ComponentUuid,
    elem_size: usize,
    elem_align: usize,
    storage_flags: u32,
    hostile_read_count: usize,
    status: FfiResult,
    payload: Vec<MsgPod>,
    read_calls: usize,
}

unsafe extern "C" fn msg_col_info(
    iter: *const BamIter,
    term: usize,
    out: *mut BamColumnInfo,
) -> FfiResult {
    if term != 0 {
        return FfiResult::OUT_OF_RANGE;
    }
    // SAFETY: Caller passes pointer to `MsgBatchHostState`.
    let st = unsafe { &*iter.cast::<MsgBatchHostState>() };
    // SAFETY: `out` is writable stack storage.
    unsafe {
        *out = BamColumnInfo {
            component: BamComponent::from_raw(1),
            mode: st.mode,
            uuid: st.uuid,
            size: st.elem_size,
            align: st.elem_align,
            storage_flags: st.storage_flags,
            reserved0: 0,
        };
    }
    FfiResult::OK
}

unsafe extern "C" fn msg_read_batch(
    iter: *mut BamIter,
    term: usize,
    out_buf: *mut u8,
    max_count: usize,
    out_read_count: *mut usize,
) -> FfiResult {
    if term != 0 {
        return FfiResult::OUT_OF_RANGE;
    }
    // SAFETY: Caller passes pointer to `MsgBatchHostState`.
    let st = unsafe { &mut *iter.cast::<MsgBatchHostState>() };
    st.read_calls += 1;
    if !st.status.is_ok() {
        return st.status;
    }
    let to_write = st.hostile_read_count.min(max_count).min(st.payload.len());
    if st.elem_size > 0 && to_write > 0 && !out_buf.is_null() {
        // SAFETY: `out_buf` is `Scratch`'s uninitialized `[MaybeUninit<MsgPod>; max_count]` buffer.
        unsafe {
            core::ptr::copy_nonoverlapping(
                st.payload.as_ptr().cast::<u8>(),
                out_buf,
                to_write * st.elem_size,
            );
        }
    }
    // SAFETY: `out_read_count` is valid writable stack storage.
    unsafe { *out_read_count = st.hostile_read_count };
    FfiResult::OK
}

proptest! {
    #![proptest_config(proptest_config())]

    #[test]
    fn read_messages_batch_checked_clamps_hostile_read_count_and_guards_metadata(
        max_count in 0usize..=64,
        hostile_extra in 0usize..=256,
        use_usize_max in any::<bool>(),
        use_write_mode in any::<bool>(),
        bad_mode in any::<bool>(),
        bad_flag_resource in any::<bool>(),
        bad_flag_missing_msg in any::<bool>(),
        bad_size in any::<bool>(),
        bad_align in any::<bool>(),
        bad_uuid in any::<bool>(),
        fail_status in prop_oneof![
            Just(FfiResult::OK),
            Just(FfiResult::OUT_OF_RANGE),
            Just(FfiResult::NULL_POINTER),
            Just(FfiResult::TYPE_MISMATCH),
        ],
        seed in any::<u64>(),
    ) {
        let valid_uuid = ComponentUuid::from_parts("prop::MsgPod", core::mem::size_of::<MsgPod>(), core::mem::align_of::<MsgPod>(), 0);
        let payload: Vec<MsgPod> = (0..max_count)
            .map(|i| MsgPod {
                seq: seed.wrapping_add(i as u64),
                tag: (i as u32) ^ 0xA5A5,
                pad: 0,
            })
            .collect();

        let hostile_read_count = if use_usize_max {
            usize::MAX
        } else if hostile_extra == 0 {
            max_count / 2
        } else {
            max_count.saturating_add(hostile_extra)
        };

        let mode = if bad_mode {
            AccessMode::READ
        } else if use_write_mode {
            AccessMode::MESSAGE_WRITE
        } else {
            AccessMode::MESSAGE_READ
        };
        let mut storage_flags = if bad_flag_missing_msg {
            0
        } else {
            BAM_STORAGE_FLAG_MESSAGE
        };
        if bad_flag_resource {
            storage_flags |= BAM_STORAGE_FLAG_RESOURCE;
        }

        let mut state = MsgBatchHostState {
            mode,
            uuid: if bad_uuid { ComponentUuid::ZERO } else { valid_uuid },
            elem_size: if bad_size {
                core::mem::size_of::<MsgPod>() + 1
            } else {
                core::mem::size_of::<MsgPod>()
            },
            elem_align: if bad_align {
                core::mem::align_of::<MsgPod>() * 2
            } else {
                core::mem::align_of::<MsgPod>()
            },
            storage_flags,
            hostile_read_count,
            status: fail_status,
            payload: payload.clone(),
            read_calls: 0,
        };

        let mut api = make_mock_api();
        api.iter_column_info = Some(msg_col_info);
        api.msg_read_batch = Some(msg_read_batch);

        // SAFETY: Stack pointers `api` and `state` outlive `host` and `iter`.
        let host = unsafe { Host::from_raw(&api, (&raw const state).cast()) }.unwrap();
        // SAFETY: `state` implements mock `BamIter` callbacks in `api`.
        let mut iter = unsafe { ValidatedIter::from_raw((&raw mut state).cast(), &host) };

        let res = iter.read_messages_batch_checked::<MsgPod>(
            0,
            Some(UuidFilter::exact(valid_uuid)),
            max_count,
        );

        let expect_type_mismatch =
            bad_mode || bad_flag_resource || bad_flag_missing_msg || bad_size || bad_align || bad_uuid;
        if expect_type_mismatch {
            prop_assert_eq!(res, Err(FfiResult::TYPE_MISMATCH));
            prop_assert_eq!(state.read_calls, 0);
        } else if max_count == 0 {
            prop_assert_eq!(res, Ok(Vec::new()));
            prop_assert_eq!(state.read_calls, 0);
        } else if !fail_status.is_ok() {
            prop_assert_eq!(res, Err(fail_status));
            prop_assert_eq!(state.read_calls, 1);
        } else {
            let expected_len = hostile_read_count.min(max_count);
            let msgs = res.unwrap();
            prop_assert_eq!(msgs.len(), expected_len);
            prop_assert_eq!(msgs.as_slice(), &payload[..expected_len]);
            prop_assert_eq!(state.read_calls, 1);
        }

        // Also verify ZST message batch clamping across arbitrary `max_count` and `hostile_read_count`.
        let mut zst_state = MsgBatchHostState {
            mode: AccessMode::MESSAGE_READ,
            uuid: ComponentUuid::from_parts("prop::ZstMsg", 0, 1, 0),
            elem_size: 0,
            elem_align: 1,
            storage_flags: BAM_STORAGE_FLAG_MESSAGE,
            hostile_read_count,
            status: FfiResult::OK,
            payload: Vec::new(),
            read_calls: 0,
        };
        // SAFETY: Stack pointers `api` and `zst_state` outlive `zst_host` and `zst_iter`.
        let zst_host = unsafe { Host::from_raw(&api, (&raw const zst_state).cast()) }.unwrap();
        // SAFETY: `zst_state` implements mock `BamIter` callbacks in `api`.
        let mut zst_iter = unsafe { ValidatedIter::from_raw((&raw mut zst_state).cast(), &zst_host) };
        let zst_res = zst_iter.read_messages_batch_checked::<ZstPod>(0, None, max_count);
        prop_assert_eq!(zst_res.unwrap().len(), hostile_read_count.min(max_count));
    }
}

#[derive(Clone, Copy, Debug)]
enum RowPtrPlan {
    Null,
    Misaligned(usize),
    Valid(usize),
}

#[derive(Clone, Copy, Debug)]
enum OuterArrayPlan {
    Valid,
    Null,
    Misaligned(usize),
    OverflowLen,
}

fn row_ptr_plan_strategy() -> impl Strategy<Value = RowPtrPlan> {
    prop_oneof![
        1 => Just(RowPtrPlan::Null),
        1 => (1usize..8).prop_map(RowPtrPlan::Misaligned),
        4 => (0usize..16).prop_map(RowPtrPlan::Valid),
    ]
}

fn outer_array_plan_strategy() -> impl Strategy<Value = OuterArrayPlan> {
    prop_oneof![
        4 => Just(OuterArrayPlan::Valid),
        1 => Just(OuterArrayPlan::Null),
        1 => (1usize..core::mem::align_of::<*const u8>()).prop_map(OuterArrayPlan::Misaligned),
        1 => Just(OuterArrayPlan::OverflowLen),
    ]
}

proptest! {
    #![proptest_config(proptest_config())]

    #[test]
    fn sparse_rows_and_mut_hostile_pointer_array_fuzzing(
        outer_plan in outer_array_plan_strategy(),
        allow_null_rows in any::<bool>(),
        row_plans in prop::collection::vec(row_ptr_plan_strategy(), 0..=24),
        use_zst in any::<bool>(),
    ) {
        let mut backing: Vec<u64> = (0u64..16).map(|i| i * 100 + 7).collect();
        let len = row_plans.len();

        // Build aligned outer pointer array plus an unaligned byte buffer for Misaligned tests.
        let mut ptrs_ro: Vec<*const u8> = Vec::with_capacity(len);
        let mut ptrs_mut: Vec<*mut u8> = Vec::with_capacity(len);
        let mut expected_vals: Vec<Option<usize>> = Vec::with_capacity(len);
        let mut has_null = false;
        let mut has_misaligned_u64 = false;

        for plan in &row_plans {
            match *plan {
                RowPtrPlan::Null => {
                    has_null = true;
                    ptrs_ro.push(core::ptr::null());
                    ptrs_mut.push(core::ptr::null_mut());
                    expected_vals.push(None);
                }
                RowPtrPlan::Misaligned(offset) => {
                    has_misaligned_u64 = true;
                    // SAFETY: `offset` in `1..8` stays inside the 16-u64 `backing` allocation.
                    let p = unsafe { backing.as_mut_ptr().cast::<u8>().add(offset) };
                    ptrs_ro.push(p.cast_const());
                    ptrs_mut.push(p);
                    expected_vals.push(None);
                }
                RowPtrPlan::Valid(idx) => {
                    let slot = idx % backing.len();
                    // SAFETY: `slot < backing.len()`.
                    let p = unsafe { backing.as_mut_ptr().add(slot) }.cast::<u8>();
                    ptrs_ro.push(p.cast_const());
                    ptrs_mut.push(p);
                    expected_vals.push(Some(slot));
                }
            }
        }

        // Aligned outer buffer whose address we offset with `with_addr` to test misaligned pointers.
        let outer_anchor = [0usize; 4];

        match outer_plan {
            OuterArrayPlan::Null => {
                // SAFETY: Testing `from_raw_parts` null-outer validation; `len == 0` succeeds cleanly.
                let ro = unsafe { SparseRows::<u64>::from_raw_parts(core::ptr::null(), len, allow_null_rows) };
                // SAFETY: Testing `from_raw_parts` null-outer validation; `len == 0` succeeds cleanly.
                let rw = unsafe { SparseRowsMut::<u64>::from_raw_parts(core::ptr::null_mut(), len, allow_null_rows) };
                if len == 0 {
                    prop_assert!(ro.unwrap().is_empty());
                    prop_assert!(rw.unwrap().is_empty());
                } else {
                    prop_assert!(ro.is_none());
                    prop_assert!(rw.is_none());
                }
            }
            OuterArrayPlan::Misaligned(off) => {
                let bad_ptr = outer_anchor
                    .as_ptr()
                    .with_addr(outer_anchor.as_ptr().addr() + off)
                    .cast::<*const u8>();
                // SAFETY: `bad_ptr` is intentionally misaligned so `from_raw_parts` rejects `len > 0`
                // before dereferencing.
                let ro = unsafe { SparseRows::<u64>::from_raw_parts(bad_ptr, len, allow_null_rows) };
                // SAFETY: Intentionally misaligned outer pointer.
                let rw = unsafe { SparseRowsMut::<u64>::from_raw_parts(bad_ptr.cast::<*mut u8>(), len, allow_null_rows) };
                if len == 0 {
                    prop_assert!(ro.unwrap().is_empty());
                    prop_assert!(rw.unwrap().is_empty());
                } else {
                    prop_assert!(ro.is_none());
                    prop_assert!(rw.is_none());
                }
            }
            OuterArrayPlan::OverflowLen => {
                let huge_len = (isize::MAX as usize / core::mem::size_of::<*const u8>()) + 1;
                let dummy = core::ptr::NonNull::<*const u8>::dangling().as_ptr();
                // SAFETY: `huge_len * size_of::<*const u8>() > isize::MAX`, rejected before dereference.
                let ro = unsafe { SparseRows::<u64>::from_raw_parts(dummy, huge_len, allow_null_rows) };
                prop_assert!(ro.is_none());
                // SAFETY: `huge_len * size_of::<*mut u8>() > isize::MAX`, rejected before dereference.
                let rw = unsafe { SparseRowsMut::<u64>::from_raw_parts(dummy.cast::<*mut u8>(), huge_len, allow_null_rows) };
                prop_assert!(rw.is_none());
            }
            OuterArrayPlan::Valid if use_zst => {
                let expect_ok = len == 0 || allow_null_rows || !has_null;
                // SAFETY: `ptrs_ro` is valid for `len` pointers; `ZstPod` never dereferences row pointers.
                let ro = unsafe { SparseRows::<ZstPod>::from_raw_parts(ptrs_ro.as_ptr(), len, allow_null_rows) };
                prop_assert_eq!(ro.is_some(), expect_ok);
                if let Some(rows) = ro {
                    prop_assert_eq!(rows.len(), len);
                    let mut present = 0usize;
                    for (i, plan) in row_plans.iter().enumerate() {
                        let is_present = !matches!(plan, RowPtrPlan::Null);
                        prop_assert_eq!(rows.get(i).is_some(), is_present);
                        if is_present {
                            present += 1;
                        }
                    }
                    prop_assert!(rows.get(len).is_none());
                    prop_assert_eq!(rows.iter().count(), present);
                }

                // SAFETY: `ptrs_mut` is valid for `len` pointers; `ZstPod` never dereferences row pointers.
                let rw = unsafe { SparseRowsMut::<ZstPod>::from_raw_parts(ptrs_mut.as_ptr(), len, allow_null_rows) };
                prop_assert_eq!(rw.is_some(), expect_ok);
                if let Some(mut rows_mut) = rw {
                    let mut visited = 0usize;
                    rows_mut.for_each_mut(|_, _| visited += 1);
                    let expected_present = row_plans.iter().filter(|p| !matches!(p, RowPtrPlan::Null)).count();
                    prop_assert_eq!(visited, expected_present);
                }
            }
            OuterArrayPlan::Valid => {
                let expect_ok = len == 0 || (!has_misaligned_u64 && (allow_null_rows || !has_null));
                // SAFETY: `ptrs_ro` is valid for `len` pointers, and `from_raw_parts` validates
                // alignment/nulls before any row dereference.
                let ro = unsafe { SparseRows::<u64>::from_raw_parts(ptrs_ro.as_ptr(), len, allow_null_rows) };
                prop_assert_eq!(ro.is_some(), expect_ok);
                if let Some(rows) = ro {
                    prop_assert_eq!(rows.len(), len);
                    let mut iter_vals = Vec::new();
                    for (i, slot_opt) in expected_vals.iter().enumerate() {
                        match slot_opt {
                            Some(slot) => {
                                let expected = backing[*slot];
                                prop_assert_eq!(rows.get(i).copied(), Some(expected));
                                prop_assert_eq!(rows[i], expected);
                                iter_vals.push(expected);
                            }
                            None => prop_assert_eq!(rows.get(i), None),
                        }
                    }
                    prop_assert!(rows.get(len).is_none());
                    let actual_iter: Vec<u64> = rows.iter().copied().collect();
                    prop_assert_eq!(actual_iter, iter_vals);
                }

                // SAFETY: `ptrs_mut` is valid for `len` pointers; `SparseRowsMut` lends one `&mut u64`
                // at a time so duplicate row pointers remain sound.
                let rw = unsafe { SparseRowsMut::<u64>::from_raw_parts(ptrs_mut.as_ptr(), len, allow_null_rows) };
                prop_assert_eq!(rw.is_some(), expect_ok);
                if let Some(mut rows_mut) = rw {
                    let mut count = 0usize;
                    rows_mut.for_each_mut(|_, v| {
                        *v = v.wrapping_add(1);
                        count += 1;
                    });
                    let expected_present = expected_vals.iter().filter(|o| o.is_some()).count();
                    prop_assert_eq!(count, expected_present);
                    for (i, slot_opt) in expected_vals.iter().enumerate() {
                        if let Some(slot) = slot_opt {
                            prop_assert_eq!(rows_mut.get(i).copied(), Some(backing[*slot]));
                        } else {
                            prop_assert!(rows_mut.get_mut(i).is_none());
                        }
                    }
                }
            }
        }

        let _ = ABI_VERSION;
    }
}
