//! Typed tuple queries without magic column indices.
//!
//! Sealed over tuples of arity 1 through 16 (as well as single-term queries) covering `&T`,
//! `&mut T`, `Option<&T>`, `Option<&mut T>`, [`With<T>`], [`Without<T>`], [`Changed<T>`],
//! [`Added<T>`], [`Allow<T>`], [`Has<T>`], [`Or<(T0, T1, ...)>`] (2..=8 filter terms, with
//! [`Or2`] and [`Or3`] type aliases), and [`FfiEntity`] where `T: GuestComponent`.

use core::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use bamabi_guest_sys::{BatchColCache, ChunkColumnDispenser, Host};
use bamabi_sys::{
    AccessMode, BamChangeTicks, BamComponent, BamTermDesc, FfiEntity, FfiResult,
    MAX_TERMS_PER_QUERY,
};

use crate::{
    app::IntoSystemResult,
    component::{GuestComponent, guest_uuid_filter, resolve_guest_component},
    iter::ChunkIter,
};

mod private {
    /// Private sealing trait restricting [`super::QueryTerm`] implementations to this crate.
    pub trait TermSealed {}
    /// Private sealing trait restricting [`super::QueryTuple`] implementations to this crate.
    pub trait TupleSealed {}
    /// Private sealing trait restricting [`super::OrFilterTerm`] to valid filter markers.
    pub trait OrFilterSealed {}
    /// Private sealing trait restricting [`super::OrFilterTuple`] to 2..=8 filter tuples.
    pub trait OrFilterTupleSealed {}
}

/// Archetype inclusion filter marker (`AccessMode::FILTER_ONLY`) for [`QueryTuple`].
pub struct With<T>(PhantomData<fn() -> T>);

/// Archetype exclusion filter marker (`AccessMode::WITHOUT`) for [`QueryTuple`].
pub struct Without<T>(PhantomData<fn() -> T>);

/// Row-level change-tick filter with read-only slice access (`AccessMode::CHANGED`) for [`QueryTuple`].
pub struct Changed<T>(PhantomData<fn() -> T>);

/// Row-level added-tick filter with read-only slice access (`AccessMode::ADDED`) for [`QueryTuple`].
pub struct Added<T>(PhantomData<fn() -> T>);

/// Row-level change-tick filter with mutable slice access (`AccessMode::WRITE` + `BAM_TERM_FLAG_CHANGED`)
/// for [`QueryTuple`], yielding `&mut [T]`.
pub struct ChangedMut<T>(PhantomData<fn() -> T>);

/// Row-level added-tick filter with mutable slice access (`AccessMode::WRITE` + `BAM_TERM_FLAG_ADDED`)
/// for [`QueryTuple`], yielding `&mut [T]`.
pub struct AddedMut<T>(PhantomData<fn() -> T>);

/// Row-level change-tick filter without borrowing component data (`AccessMode::FILTER_ONLY` +
/// `BAM_TERM_FLAG_CHANGED`), yielding `()`.
pub struct WithChanged<T>(PhantomData<fn() -> T>);

/// Row-level added-tick filter without borrowing component data (`AccessMode::FILTER_ONLY` +
/// `BAM_TERM_FLAG_ADDED`), yielding `()`.
pub struct WithAdded<T>(PhantomData<fn() -> T>);

/// Read-only column query term yielding a [`RefSlice<'chunk, T>`] with per-row change-detection
/// inspection (`is_added`, `is_changed`, `row_ticks`) via `BamHostApi::iter_column_ticks` (slot 69).
pub struct Ref<T>(PhantomData<fn() -> T>);

/// Mutable column query term yielding a [`MutSlice<'chunk, T>`] with per-row change-detection
/// inspection (`is_added`, `is_changed`, `row_ticks`) via `BamHostApi::iter_column_ticks` (slot 69).
pub struct Mut<T>(PhantomData<fn() -> T>);

/// Read-only dense column slice paired with optional per-row change-detection tick metadata
/// retrieved via `BamHostApi::iter_column_ticks` (slot 69).
///
/// On older hosts where slot 69 is absent (`FfiResult::UNSUPPORTED`), [`Self::has_ticks`] returns
/// `false` and [`Self::is_added`] / [`Self::is_changed`] gracefully return `false`.
pub struct RefSlice<'chunk, T> {
    data: &'chunk [T],
    ticks: Option<(&'chunk [u32], &'chunk [u32], u32, u32)>,
}

impl<'chunk, T> RefSlice<'chunk, T> {
    /// Returns the underlying immutable component slice `&'chunk [T]`.
    #[inline]
    pub const fn as_slice(&self) -> &'chunk [T] {
        self.data
    }

    /// Returns the number of entity rows in this chunk slice.
    #[inline]
    pub const fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns `true` if this chunk slice contains zero rows.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Returns `true` if per-row change ticks were provided by the host (slot 69 available).
    #[inline]
    pub const fn has_ticks(&self) -> bool {
        self.ticks.is_some()
    }

    /// Returns the [`BamChangeTicks`] snapshot for `row`, or `None` if `row >= self.len()` or
    /// the host does not support `iter_column_ticks`.
    #[inline]
    pub fn row_ticks(&self, row: usize) -> Option<BamChangeTicks> {
        let (added, changed, last_run, this_run) = self.ticks?;
        Some(BamChangeTicks {
            added: *added.get(row)?,
            changed: *changed.get(row)?,
            last_run,
            this_run,
        })
    }

    /// Returns `true` if `row` was newly added since the system's previous run.
    #[inline]
    pub fn is_added(&self, row: usize) -> bool {
        let Some((added, _changed, last_run, this_run)) = self.ticks else {
            return false;
        };
        added
            .get(row)
            .is_some_and(|&tick| BamChangeTicks::is_tick_newer(tick, last_run, this_run))
    }

    /// Returns `true` if `row` was mutated since the system's previous run.
    #[inline]
    pub fn is_changed(&self, row: usize) -> bool {
        let Some((_added, changed, last_run, this_run)) = self.ticks else {
            return false;
        };
        changed
            .get(row)
            .is_some_and(|&tick| BamChangeTicks::is_tick_newer(tick, last_run, this_run))
    }
}

impl<T> Deref for RefSlice<'_, T> {
    type Target = [T];

    /// Dereferences to the underlying immutable component slice `&[T]`.
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.data
    }
}

/// Mutable dense column slice paired with optional per-row change-detection tick metadata
/// retrieved via `BamHostApi::iter_column_ticks` (slot 69).
///
/// On older hosts where slot 69 is absent (`FfiResult::UNSUPPORTED`), [`Self::has_ticks`] returns
/// `false` and [`Self::is_added`] / [`Self::is_changed`] gracefully return `false`.
pub struct MutSlice<'chunk, T> {
    data: &'chunk mut [T],
    ticks: Option<(&'chunk [u32], &'chunk [u32], u32, u32)>,
}

impl<'chunk, T> MutSlice<'chunk, T> {
    /// Returns the underlying immutable component slice `&[T]`.
    #[inline]
    pub const fn as_slice(&self) -> &[T] {
        self.data
    }

    /// Returns the underlying mutable component slice `&mut [T]`.
    #[inline]
    pub const fn as_mut_slice(&mut self) -> &mut [T] {
        self.data
    }

    /// Consumes this wrapper and returns the underlying `'chunk`-lived `&'chunk mut [T]`.
    #[inline]
    pub const fn into_mut_slice(self) -> &'chunk mut [T] {
        self.data
    }

    /// Returns the number of entity rows in this chunk slice.
    #[inline]
    pub const fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns `true` if this chunk slice contains zero rows.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Returns `true` if per-row change ticks were provided by the host (slot 69 available).
    #[inline]
    pub const fn has_ticks(&self) -> bool {
        self.ticks.is_some()
    }

    /// Returns the [`BamChangeTicks`] snapshot for `row`, or `None` if `row >= self.len()` or
    /// the host does not support `iter_column_ticks`.
    #[inline]
    pub fn row_ticks(&self, row: usize) -> Option<BamChangeTicks> {
        let (added, changed, last_run, this_run) = self.ticks?;
        Some(BamChangeTicks {
            added: *added.get(row)?,
            changed: *changed.get(row)?,
            last_run,
            this_run,
        })
    }

    /// Returns `true` if `row` was newly added since the system's previous run.
    #[inline]
    pub fn is_added(&self, row: usize) -> bool {
        let Some((added, _changed, last_run, this_run)) = self.ticks else {
            return false;
        };
        added
            .get(row)
            .is_some_and(|&tick| BamChangeTicks::is_tick_newer(tick, last_run, this_run))
    }

    /// Returns `true` if `row` was mutated since the system's previous run.
    #[inline]
    pub fn is_changed(&self, row: usize) -> bool {
        let Some((_added, changed, last_run, this_run)) = self.ticks else {
            return false;
        };
        changed
            .get(row)
            .is_some_and(|&tick| BamChangeTicks::is_tick_newer(tick, last_run, this_run))
    }
}

impl<T> Deref for MutSlice<'_, T> {
    type Target = [T];

    /// Dereferences to the underlying immutable component slice `&[T]`.
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.data
    }
}

impl<T> DerefMut for MutSlice<'_, T> {
    /// Mutably dereferences to the underlying component slice `&mut [T]`.
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data
    }
}

/// Archetype filter bypass marker (`AccessMode::ALLOW`) suppressing `DefaultQueryFilters`
/// (such as `Disabled`) for `T` without requiring `T` to be present on matched entities.
pub struct Allow<T>(PhantomData<fn() -> T>);

/// Archetypal presence indicator (`AccessMode::HAS`) yielding `bool` for each matched chunk
/// without borrowing `T`'s component storage (zero scheduler conflict with `&mut T`).
pub struct Has<T>(PhantomData<fn() -> T>);

/// Disjunctive `Or<(T0, T1, ...)>` query filter (`or_group != 0`) over 2..=8 [`OrFilterTerm`]
/// items for [`QueryTuple`].
///
/// Non-filter data terms (such as `&T` or `&mut T`) and 1-element tuples are rejected at compile
/// time:
///
/// ```compile_fail
/// use bamabi_guest::iter::{Or, QueryTuple};
/// use bamabi_guest::FfiChildOf;
///
/// fn assert_tuple<Q: QueryTuple>() {}
/// assert_tuple::<Or<(&FfiChildOf, &FfiChildOf)>>();
/// ```
///
/// ```compile_fail
/// use bamabi_guest::iter::{Or, QueryTuple, With};
/// use bamabi_guest::FfiChildOf;
///
/// fn assert_tuple<Q: QueryTuple>() {}
/// assert_tuple::<Or<(With<FfiChildOf>,)>>();
/// ```
pub struct Or<T>(PhantomData<fn() -> T>);

/// Disjunctive 2-branch `Or<(A, B)>` query filter (`or_group != 0`) for [`QueryTuple`].
///
/// Non-filter data terms (such as `&T` or `&mut T`) are rejected at compile time:
///
/// ```compile_fail
/// use bamabi_guest::iter::{Or2, QueryTuple};
/// use bamabi_guest::FfiChildOf;
///
/// fn assert_tuple<Q: QueryTuple>() {}
/// assert_tuple::<Or2<&FfiChildOf, &FfiChildOf>>();
/// ```
pub type Or2<A, B> = Or<(A, B)>;

/// Disjunctive 3-branch `Or<(A, B, C)>` query filter (`or_group != 0`) for [`QueryTuple`].
pub type Or3<A, B, C> = Or<(A, B, C)>;

/// Sealed trait implemented by filter terms permitted inside [`Or`] ([`Or2`] / [`Or3`]):
/// [`With<T>`], [`Without<T>`], [`Changed<T>`], and [`Added<T>`].
pub trait OrFilterTerm: private::OrFilterSealed {
    /// `'static` canonical type key distinguishing filter term types and access modes.
    type StaticKey: 'static;

    /// Resolves and appends the filter term with `or_group` assigned.
    fn push_filter_desc(
        host: &Host,
        manifest_name: &str,
        or_group: u8,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult>;

    /// Validates this filter term's descriptor in the active chunk without borrowing data slices.
    fn fetch_filter<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<(), FfiResult>;
}

/// Sealed trait implemented by 2..=8 tuples of [`OrFilterTerm`] items accepted by [`Or`].
pub trait OrFilterTuple: private::OrFilterTupleSealed {
    /// `'static` canonical type key distinguishing filter tuple shapes and branch access modes.
    type StaticKey: 'static;

    /// Number of disjunctive branches in this `Or<(...)>` group (`2..=8`).
    const BRANCH_COUNT: usize;

    /// Resolves and appends every branch in this tuple with the shared `or_group` ID.
    fn push_group_descs(
        host: &Host,
        manifest_name: &str,
        or_group: u8,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult>;

    /// Validates every branch descriptor in the active chunk without borrowing data slices.
    fn fetch_group_filters<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<(), FfiResult>;
}

/// Resolves `T: GuestComponent` and appends a [`BamTermDesc`] tagged with `or_group` to `out`,
/// rejecting queries that exceed `MAX_TERMS_PER_QUERY`.
#[inline]
fn push_resolved_filter_term<T: GuestComponent>(
    host: &Host,
    manifest_name: &str,
    mode: AccessMode,
    or_group: u8,
    out: &mut Vec<BamTermDesc>,
) -> Result<(), FfiResult> {
    if out.len() >= MAX_TERMS_PER_QUERY {
        return Err(FfiResult::CAPACITY_EXCEEDED);
    }
    let comp = resolve_guest_component::<T>(host, manifest_name)?;
    out.push(BamTermDesc::new(comp, mode).with_or_group(or_group));
    Ok(())
}

/// Allocates the next non-zero `or_group` ID (`1..=255`) above any group already present in `out`.
#[inline]
fn next_or_group_id(out: &[BamTermDesc]) -> Result<u8, FfiResult> {
    out.iter()
        .map(|t| t.or_group)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(FfiResult::CAPACITY_EXCEEDED)
}

/// Resolves `T: GuestComponent` and appends an ungrouped [`BamTermDesc`] with `mode` to `out`.
#[inline]
fn push_resolved_term<T: GuestComponent>(
    host: &Host,
    manifest_name: &str,
    mode: AccessMode,
    out: &mut Vec<BamTermDesc>,
) -> Result<(), FfiResult> {
    if out.len() >= MAX_TERMS_PER_QUERY {
        return Err(FfiResult::CAPACITY_EXCEEDED);
    }
    let comp = resolve_guest_component::<T>(host, manifest_name)?;
    out.push(BamTermDesc::new(comp, mode));
    Ok(())
}

/// Sealed trait implemented by individual typed query terms (`&T`, `&mut T`, `Option<&T>`,
/// `Option<&mut T>`, [`With<T>`], [`Without<T>`], [`Changed<T>`], [`Added<T>`], [`Allow<T>`],
/// [`Has<T>`], [`Or<(T0, T1, ...)>`], and [`FfiEntity`]).
pub trait QueryTerm: private::TermSealed {
    /// Zero-copy slice or unit marker yielded for each chunk of this term.
    type Slice<'chunk>;

    /// `'static` canonical type key distinguishing component types and access modes (`&'static T`
    /// vs `&'static mut T`, `Option<&'static T>` vs `Option<&'static mut T>`, etc.).
    type StaticKey: 'static;

    /// `true` if this term never requests mutable column access (`WRITE` / `OPTIONAL_WRITE`).
    const IS_READ_ONLY: bool = true;

    /// Number of [`BamTermDesc`] entries emitted by this term (`0` for [`FfiEntity`], `1` for
    /// single-component terms, `T::BRANCH_COUNT` for [`Or<T>`]).
    const TERM_COUNT: usize = 1;

    /// `true` if this term accesses the chunk's contiguous `FfiEntity` slice.
    const NEEDS_ENTITIES: bool = false;

    /// Bitmask (relative to bit `0` of this term) of writable columns whose change ticks should
    /// be marked when fetched via `BamHostApi::iter_next_chunk_batch`.
    const MARK_CHANGED_MASK: u32 = 0;

    /// Resolves and appends the corresponding [`BamTermDesc`] (if any) during system registration.
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult>;

    /// Validates and dispenses this term's slice from the active chunk.
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult>;
}

impl private::TermSealed for FfiEntity {}
impl QueryTerm for FfiEntity {
    type Slice<'chunk> = &'chunk [FfiEntity];
    type StaticKey = FfiEntity;
    const TERM_COUNT: usize = 0;
    const NEEDS_ENTITIES: bool = true;

    /// No-op: entity IDs are always present on every chunk without declaring a component term.
    #[inline]
    fn push_desc(
        _host: &Host,
        _manifest_name: &str,
        _out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        Ok(())
    }

    /// Dispenses the chunk's contiguous `&[FfiEntity]` slice.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        dispenser.entities()
    }
}

impl<T: GuestComponent> private::TermSealed for &T {}
impl<T: GuestComponent> QueryTerm for &T {
    type Slice<'chunk> = &'chunk [T];
    type StaticKey = &'static T;

    /// Appends an [`AccessMode::READ`] descriptor for `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::READ, out)
    }

    /// Dispenses the UUID-verified shared `&[T]` column slice.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_read::<T>(AccessMode::READ, Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for &mut T {}
impl<T: GuestComponent> QueryTerm for &mut T {
    type Slice<'chunk> = &'chunk mut [T];
    type StaticKey = &'static mut T;
    const IS_READ_ONLY: bool = false;
    const MARK_CHANGED_MASK: u32 = 1;

    /// Appends an [`AccessMode::WRITE`] descriptor for `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::WRITE, out)
    }

    /// Dispenses the UUID-verified exclusive `&mut [T]` column slice.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_write::<T>(Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for Option<&T> {}
impl<T: GuestComponent> QueryTerm for Option<&T> {
    type Slice<'chunk> = Option<&'chunk [T]>;
    type StaticKey = Option<&'static T>;

    /// Appends an [`AccessMode::OPTIONAL_READ`] descriptor for `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::OPTIONAL_READ, out)
    }

    /// Dispenses `Some(&[T])` when `T` is present in the chunk, or `None` when absent.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_optional_read::<T>(Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for Option<&mut T> {}
impl<T: GuestComponent> QueryTerm for Option<&mut T> {
    type Slice<'chunk> = Option<&'chunk mut [T]>;
    type StaticKey = Option<&'static mut T>;
    const IS_READ_ONLY: bool = false;
    const MARK_CHANGED_MASK: u32 = 1;

    /// Appends an [`AccessMode::OPTIONAL_WRITE`] descriptor for `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::OPTIONAL_WRITE, out)
    }

    /// Dispenses `Some(&mut [T])` when `T` is present in the chunk, or `None` when absent.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_optional_write::<T>(Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for With<T> {}
impl<T: GuestComponent> QueryTerm for With<T> {
    type Slice<'chunk> = ();
    type StaticKey = Self;

    /// Appends an [`AccessMode::FILTER_ONLY`] (`With<T>`) descriptor.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::FILTER_ONLY, out)
    }

    /// Validates the `FILTER_ONLY` column descriptor without borrowing data.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_filter::<T>(AccessMode::FILTER_ONLY, Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for Without<T> {}
impl<T: GuestComponent> QueryTerm for Without<T> {
    type Slice<'chunk> = ();
    type StaticKey = Self;

    /// Appends an [`AccessMode::WITHOUT`] (`Without<T>`) descriptor.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::WITHOUT, out)
    }

    /// Validates the `WITHOUT` column descriptor without borrowing data.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_filter::<T>(AccessMode::WITHOUT, Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for Changed<T> {}
impl<T: GuestComponent> QueryTerm for Changed<T> {
    type Slice<'chunk> = &'chunk [T];
    type StaticKey = Self;

    /// Appends an [`AccessMode::CHANGED`] descriptor for `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::CHANGED, out)
    }

    /// Dispenses the shared `&[T]` slice for rows matching `Changed<T>`.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_read::<T>(AccessMode::CHANGED, Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for Added<T> {}
impl<T: GuestComponent> QueryTerm for Added<T> {
    type Slice<'chunk> = &'chunk [T];
    type StaticKey = Self;

    /// Appends an [`AccessMode::ADDED`] descriptor for `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::ADDED, out)
    }

    /// Dispenses the shared `&[T]` slice for rows matching `Added<T>`.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_read::<T>(AccessMode::ADDED, Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for ChangedMut<T> {}
impl<T: GuestComponent> QueryTerm for ChangedMut<T> {
    type Slice<'chunk> = &'chunk mut [T];
    type StaticKey = Self;
    const IS_READ_ONLY: bool = false;
    const MARK_CHANGED_MASK: u32 = 1;

    /// Appends an [`AccessMode::WRITE`] descriptor tagged with `BAM_TERM_FLAG_CHANGED` for `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        if out.len() >= MAX_TERMS_PER_QUERY {
            return Err(FfiResult::CAPACITY_EXCEEDED);
        }
        let comp = resolve_guest_component::<T>(host, manifest_name)?;
        out.push(BamTermDesc::new(comp, AccessMode::WRITE).with_changed());
        Ok(())
    }

    /// Dispenses the UUID-verified exclusive `&mut [T]` slice for rows matching `ChangedMut<T>`.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_write::<T>(Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for AddedMut<T> {}
impl<T: GuestComponent> QueryTerm for AddedMut<T> {
    type Slice<'chunk> = &'chunk mut [T];
    type StaticKey = Self;
    const IS_READ_ONLY: bool = false;
    const MARK_CHANGED_MASK: u32 = 1;

    /// Appends an [`AccessMode::WRITE`] descriptor tagged with `BAM_TERM_FLAG_ADDED` for `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        if out.len() >= MAX_TERMS_PER_QUERY {
            return Err(FfiResult::CAPACITY_EXCEEDED);
        }
        let comp = resolve_guest_component::<T>(host, manifest_name)?;
        out.push(BamTermDesc::new(comp, AccessMode::WRITE).with_added());
        Ok(())
    }

    /// Dispenses the UUID-verified exclusive `&mut [T]` slice for rows matching `AddedMut<T>`.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_write::<T>(Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for WithChanged<T> {}
impl<T: GuestComponent> QueryTerm for WithChanged<T> {
    type Slice<'chunk> = ();
    type StaticKey = Self;

    /// Appends an [`AccessMode::FILTER_ONLY`] descriptor tagged with `BAM_TERM_FLAG_CHANGED`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        if out.len() >= MAX_TERMS_PER_QUERY {
            return Err(FfiResult::CAPACITY_EXCEEDED);
        }
        let comp = resolve_guest_component::<T>(host, manifest_name)?;
        out.push(BamTermDesc::new(comp, AccessMode::FILTER_ONLY).with_changed());
        Ok(())
    }

    /// Validates the `FILTER_ONLY` column descriptor without borrowing component data.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_filter::<T>(AccessMode::FILTER_ONLY, Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for WithAdded<T> {}
impl<T: GuestComponent> QueryTerm for WithAdded<T> {
    type Slice<'chunk> = ();
    type StaticKey = Self;

    /// Appends an [`AccessMode::FILTER_ONLY`] descriptor tagged with `BAM_TERM_FLAG_ADDED`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        if out.len() >= MAX_TERMS_PER_QUERY {
            return Err(FfiResult::CAPACITY_EXCEEDED);
        }
        let comp = resolve_guest_component::<T>(host, manifest_name)?;
        out.push(BamTermDesc::new(comp, AccessMode::FILTER_ONLY).with_added());
        Ok(())
    }

    /// Validates the `FILTER_ONLY` column descriptor without borrowing component data.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_filter::<T>(AccessMode::FILTER_ONLY, Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for Ref<T> {}
impl<T: GuestComponent> QueryTerm for Ref<T> {
    type Slice<'chunk> = RefSlice<'chunk, T>;
    type StaticKey = Self;

    /// Appends an [`AccessMode::READ`] descriptor for `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::READ, out)
    }

    /// Dispenses a [`RefSlice`] pairing `&[T]` with per-row change-detection ticks (slot 69),
    /// falling back gracefully to `has_ticks() == false` on hosts without slot 69.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let term_index = dispenser.next_term_index();
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        let data = dispenser.next_read::<T>(AccessMode::READ, Some(filter))?;
        let ticks = match dispenser.column_ticks(term_index) {
            Ok(t) => Some(t),
            Err(FfiResult::UNSUPPORTED) => None,
            Err(err) => return Err(err),
        };
        Ok(RefSlice { data, ticks })
    }
}

impl<T: GuestComponent> private::TermSealed for Mut<T> {}
impl<T: GuestComponent> QueryTerm for Mut<T> {
    type Slice<'chunk> = MutSlice<'chunk, T>;
    type StaticKey = Self;
    const IS_READ_ONLY: bool = false;
    const MARK_CHANGED_MASK: u32 = 1;

    /// Appends an [`AccessMode::WRITE`] descriptor for `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::WRITE, out)
    }

    /// Dispenses a [`MutSlice`] pairing `&mut [T]` with per-row change-detection ticks (slot 69),
    /// falling back gracefully to `has_ticks() == false` on hosts without slot 69.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let term_index = dispenser.next_term_index();
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        let data = dispenser.next_write::<T>(Some(filter))?;
        let ticks = match dispenser.column_ticks(term_index) {
            Ok(t) => Some(t),
            Err(FfiResult::UNSUPPORTED) => None,
            Err(err) => return Err(err),
        };
        Ok(MutSlice { data, ticks })
    }
}

impl<T: GuestComponent> private::TermSealed for Allow<T> {}
impl<T: GuestComponent> QueryTerm for Allow<T> {
    type Slice<'chunk> = ();
    type StaticKey = Self;

    /// Appends an [`AccessMode::ALLOW`] (`Allow<T>`) descriptor.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::ALLOW, out)
    }

    /// Validates the `ALLOW` column descriptor without borrowing data.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_filter::<T>(AccessMode::ALLOW, Some(filter))
    }
}

impl<T: GuestComponent> private::TermSealed for Has<T> {}
impl<T: GuestComponent> QueryTerm for Has<T> {
    type Slice<'chunk> = bool;
    type StaticKey = Self;

    /// Appends an [`AccessMode::HAS`] (`Has<T>`) descriptor.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        push_resolved_term::<T>(host, manifest_name, AccessMode::HAS, out)
    }

    /// Dispenses the archetypal `bool` presence flag for `T` in this chunk.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_has::<T>(Some(filter))
    }
}

/// Implements [`OrFilterTerm`] for a filter marker type (`With`, `Without`, `Changed`, or `Added`).
macro_rules! impl_or_filter_term {
    ($target:ty, $mode:expr) => {
        impl<T: GuestComponent> private::OrFilterSealed for $target {}
        impl<T: GuestComponent> OrFilterTerm for $target {
            type StaticKey = Self;

            /// Appends the filter term descriptor tagged with `or_group`.
            #[inline]
            fn push_filter_desc(
                host: &Host,
                manifest_name: &str,
                or_group: u8,
                out: &mut Vec<BamTermDesc>,
            ) -> Result<(), FfiResult> {
                push_resolved_filter_term::<T>(host, manifest_name, $mode, or_group, out)
            }

            /// Validates the filter descriptor in the active chunk without borrowing data.
            #[inline]
            fn fetch_filter<'chunk>(
                dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
            ) -> Result<(), FfiResult> {
                let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
                dispenser.next_filter::<T>($mode, Some(filter))
            }
        }
    };
}

impl_or_filter_term!(With<T>, AccessMode::FILTER_ONLY);
impl_or_filter_term!(Without<T>, AccessMode::WITHOUT);
impl_or_filter_term!(Changed<T>, AccessMode::CHANGED);
impl_or_filter_term!(Added<T>, AccessMode::ADDED);

impl<T: GuestComponent> private::OrFilterSealed for WithChanged<T> {}
impl<T: GuestComponent> OrFilterTerm for WithChanged<T> {
    type StaticKey = Self;

    /// Appends a `FILTER_ONLY` + `BAM_TERM_FLAG_CHANGED` descriptor tagged with `or_group`.
    #[inline]
    fn push_filter_desc(
        host: &Host,
        manifest_name: &str,
        or_group: u8,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        if out.len() >= MAX_TERMS_PER_QUERY {
            return Err(FfiResult::CAPACITY_EXCEEDED);
        }
        let comp = resolve_guest_component::<T>(host, manifest_name)?;
        out.push(
            BamTermDesc::new(comp, AccessMode::FILTER_ONLY)
                .with_changed()
                .with_or_group(or_group),
        );
        Ok(())
    }

    /// Validates the `FILTER_ONLY` descriptor in the active chunk without borrowing data.
    #[inline]
    fn fetch_filter<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<(), FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_filter::<T>(AccessMode::FILTER_ONLY, Some(filter))
    }
}

impl<T: GuestComponent> private::OrFilterSealed for WithAdded<T> {}
impl<T: GuestComponent> OrFilterTerm for WithAdded<T> {
    type StaticKey = Self;

    /// Appends a `FILTER_ONLY` + `BAM_TERM_FLAG_ADDED` descriptor tagged with `or_group`.
    #[inline]
    fn push_filter_desc(
        host: &Host,
        manifest_name: &str,
        or_group: u8,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        if out.len() >= MAX_TERMS_PER_QUERY {
            return Err(FfiResult::CAPACITY_EXCEEDED);
        }
        let comp = resolve_guest_component::<T>(host, manifest_name)?;
        out.push(
            BamTermDesc::new(comp, AccessMode::FILTER_ONLY)
                .with_added()
                .with_or_group(or_group),
        );
        Ok(())
    }

    /// Validates the `FILTER_ONLY` descriptor in the active chunk without borrowing data.
    #[inline]
    fn fetch_filter<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<(), FfiResult> {
        let filter = guest_uuid_filter::<T>(dispenser.base_host().manifest_name());
        dispenser.next_filter::<T>(AccessMode::FILTER_ONLY, Some(filter))
    }
}

/// Implements [`OrFilterTuple`] for fixed-arity tuples of 2..=8 [`OrFilterTerm`] branches.
macro_rules! impl_or_filter_tuple {
    ($count:expr => $($term:ident),+ $(,)?) => {
        impl<$($term: OrFilterTerm),+> private::OrFilterTupleSealed for ($($term,)+) {}

        impl<$($term: OrFilterTerm),+> OrFilterTuple for ($($term,)+) {
            type StaticKey = ($($term::StaticKey,)+);
            const BRANCH_COUNT: usize = $count;

            /// Appends all branch filter descriptors sharing `or_group`.
            #[inline]
            fn push_group_descs(
                host: &Host,
                manifest_name: &str,
                or_group: u8,
                out: &mut Vec<BamTermDesc>,
            ) -> Result<(), FfiResult> {
                $(
                    $term::push_filter_desc(host, manifest_name, or_group, out)?;
                )+
                Ok(())
            }

            /// Validates every branch filter descriptor in the active chunk.
            #[inline]
            fn fetch_group_filters<'chunk>(
                dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
            ) -> Result<(), FfiResult> {
                $(
                    $term::fetch_filter(dispenser)?;
                )+
                Ok(())
            }
        }
    };
}

impl_or_filter_tuple!(2 => T0, T1);
impl_or_filter_tuple!(3 => T0, T1, T2);
impl_or_filter_tuple!(4 => T0, T1, T2, T3);
impl_or_filter_tuple!(5 => T0, T1, T2, T3, T4);
impl_or_filter_tuple!(6 => T0, T1, T2, T3, T4, T5);
impl_or_filter_tuple!(7 => T0, T1, T2, T3, T4, T5, T6);
impl_or_filter_tuple!(8 => T0, T1, T2, T3, T4, T5, T6, T7);

impl<T: OrFilterTuple> private::TermSealed for Or<T> {}
impl<T: OrFilterTuple> QueryTerm for Or<T> {
    type Slice<'chunk> = ();
    type StaticKey = Or<T::StaticKey>;
    const TERM_COUNT: usize = T::BRANCH_COUNT;

    /// Allocates a fresh non-zero `or_group` ID and appends every branch descriptor in `T`.
    #[inline]
    fn push_desc(
        host: &Host,
        manifest_name: &str,
        out: &mut Vec<BamTermDesc>,
    ) -> Result<(), FfiResult> {
        let group_id = next_or_group_id(out)?;
        T::push_group_descs(host, manifest_name, group_id, out)
    }

    /// Validates every branch filter descriptor in `T` without borrowing data slices.
    #[inline]
    fn fetch_slice<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slice<'chunk>, FfiResult> {
        T::fetch_group_filters(dispenser)
    }
}

/// Sealed tuple trait over 1..=16 [`QueryTerm`] items (plus single-item shorthand).
pub trait QueryTuple: private::TupleSealed {
    /// Tuple (or single item) of chunk slices yielded per chunk.
    type Slices<'chunk>;

    /// `'static` canonical type key distinguishing tuple shapes, component types, and access modes.
    type StaticKey: 'static;

    /// `true` if no term in this tuple requests mutable column access (`WRITE` / `OPTIONAL_WRITE`).
    const IS_READ_ONLY: bool;

    /// Total number of [`BamTermDesc`] entries declared by this query tuple.
    const TERM_COUNT: usize;

    /// `true` if any term in this query tuple accesses the chunk's `FfiEntity` slice.
    const NEEDS_ENTITIES: bool;

    /// Bitmask of writable term indices (`0..32`) whose change ticks should be marked by
    /// `BamHostApi::iter_next_chunk_batch`.
    const MARK_CHANGED_MASK: u32;

    /// Collects the ordered [`BamTermDesc`] list for this query tuple.
    fn collect_descs(host: &Host, manifest_name: &str) -> Result<Vec<BamTermDesc>, FfiResult>;

    /// Dispenses all slices in declaration order from `dispenser`, verifying disjointness and UUIDs.
    fn fetch_slices<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slices<'chunk>, FfiResult>;

    /// Invokes `f` with tuple slice fields unpacked into separate top-level arguments behind an
    /// `#[inline(never)]` boundary so LLVM preserves per-slice `noalias`/`readonly` attributes.
    fn invoke_unpacked<F, R>(f: &mut F, slices: Self::Slices<'_>) -> FfiResult
    where
        F: FnMut(Self::Slices<'_>) -> R,
        R: IntoSystemResult;
}

/// Implements [`QueryTuple`] directly on a single [`QueryTerm`] type as a 1-item shorthand.
macro_rules! impl_single_query_tuple {
    ($($lifetime:lifetime,)? $ty_param:ident : $bound:ident => $target:ty) => {
        impl<$($lifetime,)? $ty_param: $bound> private::TupleSealed for $target {}
        impl<$($lifetime,)? $ty_param: $bound> QueryTuple for $target {
            type Slices<'chunk> = <Self as QueryTerm>::Slice<'chunk>;
            type StaticKey = <Self as QueryTerm>::StaticKey;
            const IS_READ_ONLY: bool = <Self as QueryTerm>::IS_READ_ONLY;
            const TERM_COUNT: usize = <Self as QueryTerm>::TERM_COUNT;
            const NEEDS_ENTITIES: bool = <Self as QueryTerm>::NEEDS_ENTITIES;
            const MARK_CHANGED_MASK: u32 = <Self as QueryTerm>::MARK_CHANGED_MASK;

            /// Collects the single term descriptor for this shorthand query.
            #[inline]
            fn collect_descs(
                host: &Host,
                manifest_name: &str,
            ) -> Result<Vec<BamTermDesc>, FfiResult> {
                let mut out = Vec::with_capacity(1);
                <Self as QueryTerm>::push_desc(host, manifest_name, &mut out)?;
                Ok(out)
            }

            /// Dispenses the single term slice from `dispenser`.
            #[inline]
            fn fetch_slices<'chunk>(
                dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
            ) -> Result<Self::Slices<'chunk>, FfiResult> {
                <Self as QueryTerm>::fetch_slice(dispenser)
            }

            /// Invokes `f` with the single slice across an `#[inline(never)]` `noalias` boundary.
            #[inline]
            fn invoke_unpacked<F, R>(f: &mut F, slices: Self::Slices<'_>) -> FfiResult
            where
                F: FnMut(Self::Slices<'_>) -> R,
                R: IntoSystemResult,
            {
                #[inline(never)]
                fn call_unpacked<S, F, R>(f: &mut F, s: S) -> FfiResult
                where
                    F: FnMut(S) -> R,
                    R: IntoSystemResult,
                {
                    f(s).into_ffi_result()
                }
                call_unpacked(f, slices)
            }
        }
    };
}

impl private::TupleSealed for FfiEntity {}
impl QueryTuple for FfiEntity {
    type Slices<'chunk> = &'chunk [FfiEntity];
    type StaticKey = FfiEntity;
    const IS_READ_ONLY: bool = true;
    const TERM_COUNT: usize = 0;
    const NEEDS_ENTITIES: bool = true;
    const MARK_CHANGED_MASK: u32 = 0;

    /// Returns an empty descriptor list because entity IDs require no component term.
    #[inline]
    fn collect_descs(_host: &Host, _manifest_name: &str) -> Result<Vec<BamTermDesc>, FfiResult> {
        Ok(Vec::new())
    }

    /// Dispenses the chunk's contiguous `&[FfiEntity]` slice.
    #[inline]
    fn fetch_slices<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slices<'chunk>, FfiResult> {
        dispenser.entities()
    }

    /// Invokes `f` with the entity slice across an `#[inline(never)]` `noalias` boundary.
    #[inline]
    fn invoke_unpacked<F, R>(f: &mut F, slices: Self::Slices<'_>) -> FfiResult
    where
        F: FnMut(Self::Slices<'_>) -> R,
        R: IntoSystemResult,
    {
        #[inline(never)]
        fn call_unpacked<S, F, R>(f: &mut F, s: S) -> FfiResult
        where
            F: FnMut(S) -> R,
            R: IntoSystemResult,
        {
            f(s).into_ffi_result()
        }
        call_unpacked(f, slices)
    }
}

impl_single_query_tuple!('q, T: GuestComponent => &'q T);
impl_single_query_tuple!('q, T: GuestComponent => &'q mut T);
impl_single_query_tuple!('q, T: GuestComponent => Option<&'q T>);
impl_single_query_tuple!('q, T: GuestComponent => Option<&'q mut T>);
impl_single_query_tuple!(T: GuestComponent => With<T>);
impl_single_query_tuple!(T: GuestComponent => Without<T>);
impl_single_query_tuple!(T: GuestComponent => Changed<T>);
impl_single_query_tuple!(T: GuestComponent => Added<T>);
impl_single_query_tuple!(T: GuestComponent => ChangedMut<T>);
impl_single_query_tuple!(T: GuestComponent => AddedMut<T>);
impl_single_query_tuple!(T: GuestComponent => WithChanged<T>);
impl_single_query_tuple!(T: GuestComponent => WithAdded<T>);
impl_single_query_tuple!(T: GuestComponent => Ref<T>);
impl_single_query_tuple!(T: GuestComponent => Mut<T>);
impl_single_query_tuple!(T: GuestComponent => Allow<T>);
impl_single_query_tuple!(T: GuestComponent => Has<T>);

impl<T: OrFilterTuple> private::TupleSealed for Or<T> {}
impl<T: OrFilterTuple> QueryTuple for Or<T> {
    type Slices<'chunk> = ();
    type StaticKey = Or<T::StaticKey>;
    const IS_READ_ONLY: bool = true;
    const TERM_COUNT: usize = T::BRANCH_COUNT;
    const NEEDS_ENTITIES: bool = false;
    const MARK_CHANGED_MASK: u32 = 0;

    /// Collects all branch filter descriptors in `T` under a single allocated `or_group` ID.
    #[inline]
    fn collect_descs(host: &Host, manifest_name: &str) -> Result<Vec<BamTermDesc>, FfiResult> {
        let mut out = Vec::with_capacity(T::BRANCH_COUNT);
        <Self as QueryTerm>::push_desc(host, manifest_name, &mut out)?;
        Ok(out)
    }

    /// Validates every branch filter descriptor in `T` without borrowing data slices.
    #[inline]
    fn fetch_slices<'chunk>(
        dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
    ) -> Result<Self::Slices<'chunk>, FfiResult> {
        <Self as QueryTerm>::fetch_slice(dispenser)
    }

    /// Invokes `f` across an `#[inline(never)]` boundary for consistency with data tuples.
    #[inline]
    fn invoke_unpacked<F, R>(f: &mut F, slices: Self::Slices<'_>) -> FfiResult
    where
        F: FnMut(Self::Slices<'_>) -> R,
        R: IntoSystemResult,
    {
        #[inline(never)]
        fn call_unpacked<S, F, R>(f: &mut F, s: S) -> FfiResult
        where
            F: FnMut(S) -> R,
            R: IntoSystemResult,
        {
            f(s).into_ffi_result()
        }
        call_unpacked(f, slices)
    }
}

/// Implements [`QueryTuple`] for fixed-arity tuples of 1..=16 [`QueryTerm`] elements.
macro_rules! impl_query_tuple {
    ($cap:expr => $($slice:ident : $term:ident : $var:ident),+ $(,)?) => {
        impl<$($term: QueryTerm),+> private::TupleSealed for ($($term,)+) {}

        impl<$($term: QueryTerm),+> QueryTuple for ($($term,)+) {
            type Slices<'chunk> = ($($term::Slice<'chunk>,)+);
            type StaticKey = ($($term::StaticKey,)+);
            const IS_READ_ONLY: bool = true $(&& $term::IS_READ_ONLY)+;
            const TERM_COUNT: usize = 0 $(+ $term::TERM_COUNT)+;
            const NEEDS_ENTITIES: bool = false $(|| $term::NEEDS_ENTITIES)+;
            const MARK_CHANGED_MASK: u32 = {
                let mut mask = 0u32;
                let mut offset = 0usize;
                $(
                    if offset < 32 {
                        mask |= $term::MARK_CHANGED_MASK << offset;
                    }
                    offset += $term::TERM_COUNT;
                )+
                let _ = offset;
                mask
            };

            /// Collects descriptors for every tuple element in declaration order.
            #[inline]
            fn collect_descs(
                host: &Host,
                manifest_name: &str,
            ) -> Result<Vec<BamTermDesc>, FfiResult> {
                let mut out = Vec::with_capacity($cap);
                $(
                    $term::push_desc(host, manifest_name, &mut out)?;
                )+
                Ok(out)
            }

            /// Dispenses each tuple element's slice in declaration order from `dispenser`.
            #[inline]
            fn fetch_slices<'chunk>(
                dispenser: &mut ChunkColumnDispenser<'chunk, '_>,
            ) -> Result<Self::Slices<'chunk>, FfiResult> {
                Ok(($(
                    $term::fetch_slice(dispenser)?,
                )+))
            }

            /// Unpacks tuple slice fields into separate top-level arguments behind an
            /// `#[inline(never)]` boundary so LLVM preserves per-slice `noalias`/`readonly`.
            #[inline]
            fn invoke_unpacked<F, R>(f: &mut F, slices: Self::Slices<'_>) -> FfiResult
            where
                F: FnMut(Self::Slices<'_>) -> R,
                R: IntoSystemResult,
            {
                #[inline(never)]
                #[allow(clippy::too_many_arguments)]
                fn call_unpacked<$($slice,)+ F, R>(f: &mut F, $($var: $slice,)+) -> FfiResult
                where
                    F: FnMut(($($slice,)+)) -> R,
                    R: IntoSystemResult,
                {
                    f(($($var,)+)).into_ffi_result()
                }
                let ($($var,)+) = slices;
                call_unpacked(f, $($var,)+)
            }
        }
    };
}

impl_query_tuple!(1 => S0: T0: s0);
impl_query_tuple!(2 => S0: T0: s0, S1: T1: s1);
impl_query_tuple!(3 => S0: T0: s0, S1: T1: s1, S2: T2: s2);
impl_query_tuple!(4 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3);
impl_query_tuple!(5 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4);
impl_query_tuple!(6 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5);
impl_query_tuple!(7 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5, S6: T6: s6);
impl_query_tuple!(8 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5, S6: T6: s6, S7: T7: s7);
impl_query_tuple!(9 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5, S6: T6: s6, S7: T7: s7, S8: T8: s8);
impl_query_tuple!(10 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5, S6: T6: s6, S7: T7: s7, S8: T8: s8, S9: T9: s9);
impl_query_tuple!(11 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5, S6: T6: s6, S7: T7: s7, S8: T8: s8, S9: T9: s9, S10: T10: s10);
impl_query_tuple!(12 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5, S6: T6: s6, S7: T7: s7, S8: T8: s8, S9: T9: s9, S10: T10: s10, S11: T11: s11);
impl_query_tuple!(13 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5, S6: T6: s6, S7: T7: s7, S8: T8: s8, S9: T9: s9, S10: T10: s10, S11: T11: s11, S12: T12: s12);
impl_query_tuple!(14 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5, S6: T6: s6, S7: T7: s7, S8: T8: s8, S9: T9: s9, S10: T10: s10, S11: T11: s11, S12: T12: s12, S13: T13: s13);
impl_query_tuple!(15 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5, S6: T6: s6, S7: T7: s7, S8: T8: s8, S9: T9: s9, S10: T10: s10, S11: T11: s11, S12: T12: s12, S13: T13: s13, S14: T14: s14);
impl_query_tuple!(16 => S0: T0: s0, S1: T1: s1, S2: T2: s2, S3: T3: s3, S4: T4: s4, S5: T5: s5, S6: T6: s6, S7: T7: s7, S8: T8: s8, S9: T9: s9, S10: T10: s10, S11: T11: s11, S12: T12: s12, S13: T13: s13, S14: T14: s14, S15: T15: s15);

/// Error returned by [`TypedChunkIter::single_chunk`] (mirroring Bevy's `QuerySingleError`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuerySingleError {
    /// Zero entities matched the query.
    NoEntities,
    /// Two or more entities matched the query.
    MultipleEntities,
    /// Underlying FFI validation or layout error occurred while checking/fetching the chunk.
    Ffi(FfiResult),
}

impl core::fmt::Display for QuerySingleError {
    /// Formats the single-entity query error into a concise human-readable diagnostic.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoEntities => f.write_str("no entities matched the query"),
            Self::MultipleEntities => f.write_str("multiple entities matched the query"),
            Self::Ffi(err) => write!(f, "FFI query error: {err}"),
        }
    }
}

impl std::error::Error for QuerySingleError {}

/// Typed chunk iterator over a declared [`QueryTuple`] `Q`, dispensing borrow-checked,
/// UUID-verified column slices without magic column index literals.
pub struct TypedChunkIter<'iter, 'ctx, Q: QueryTuple> {
    pub(crate) iter: &'iter mut ChunkIter<'ctx>,
    pub(crate) batch_cache: BatchColCache<'ctx>,
    pub(crate) _marker: PhantomData<fn() -> Q>,
}

impl<'iter, 'ctx, Q: QueryTuple> TypedChunkIter<'iter, 'ctx, Q> {
    /// Advances to the next matching chunk and returns `Some(Ok(slices))`, `Some(Err(status))` on
    /// layout/UUID/access mismatch, or `None` when exhausted.
    #[inline]
    pub fn try_next_chunk(&mut self) -> Option<Result<Q::Slices<'_>, FfiResult>> {
        self.iter.inner.advance_batch_into(
            &mut self.batch_cache,
            Q::TERM_COUNT,
            Q::NEEDS_ENTITIES,
            Q::MARK_CHANGED_MASK,
        )?;
        let mut dispenser =
            ChunkColumnDispenser::new_with_batch(&mut self.iter.inner, &self.batch_cache);
        Some(Q::fetch_slices(&mut dispenser))
    }

    /// Advances to the next matching chunk and yields typed `Q::Slices<'_>`, returning `None`
    /// when exhausted or if `Q` does not match the registered query signature.
    #[inline]
    pub fn next_chunk(&mut self) -> Option<Q::Slices<'_>> {
        self.try_next_chunk()?.ok()
    }

    /// Resets the underlying iterator back to its initial chunk for nested loops.
    #[inline]
    pub fn reset(&mut self) -> FfiResult {
        self.iter.reset()
    }

    /// Non-destructively counts all matching entities in this query via `BamHostApi::iter_count`
    /// (slot 57) without advancing or resetting the active chunk cursor.
    #[inline]
    pub fn count(&mut self) -> Result<usize, FfiResult> {
        self.iter.count()
    }

    /// Non-destructively returns `true` iff zero entities match this query (mirroring Bevy's
    /// `Query::is_empty`).
    #[inline]
    pub fn is_empty(&mut self) -> Result<bool, FfiResult> {
        self.iter.is_empty()
    }

    /// Yields the single-entity chunk slices if **exactly one** entity matches this query
    /// (mirroring Bevy's `Query::single` / `Query::single_mut`).
    ///
    /// Resets the iterator to the beginning, verifies the exact match count (`0` ->
    /// [`QuerySingleError::NoEntities`], `> 1` -> [`QuerySingleError::MultipleEntities`]), and
    /// dispenses `Q::Slices<'_>` (whose slice components each have length `1`). On ABI `0.2.0`
    /// hosts where `iter_count` is unsupported, counts matching entities across all chunks via
    /// `reset` + `next_chunk` before re-seeking to the unique single-entity chunk.
    pub fn single_chunk(&mut self) -> Result<Q::Slices<'_>, QuerySingleError> {
        match self.iter.count() {
            Ok(0) => return Err(QuerySingleError::NoEntities),
            Ok(n) if n > 1 => return Err(QuerySingleError::MultipleEntities),
            Ok(_) => {}
            Err(FfiResult::UNSUPPORTED) => {
                let reset_status = self.iter.reset();
                if !reset_status.is_ok() {
                    return Err(QuerySingleError::Ffi(reset_status));
                }
                let mut total = 0usize;
                while let Some(chunk_len) = self.iter.inner.next_chunk() {
                    total = total.saturating_add(chunk_len);
                    if total > 1 {
                        return Err(QuerySingleError::MultipleEntities);
                    }
                }
                if total == 0 {
                    return Err(QuerySingleError::NoEntities);
                }
            }
            Err(err) => return Err(QuerySingleError::Ffi(err)),
        }
        let reset_status = self.iter.reset();
        if !reset_status.is_ok() {
            return Err(QuerySingleError::Ffi(reset_status));
        }
        loop {
            let Some(len) = self.iter.inner.next_chunk() else {
                return Err(QuerySingleError::NoEntities);
            };
            if len == 0 {
                continue;
            }
            if len != 1 {
                return Err(QuerySingleError::MultipleEntities);
            }
            let mut dispenser = ChunkColumnDispenser::new(&mut self.iter.inner);
            return Q::fetch_slices(&mut dispenser).map_err(QuerySingleError::Ffi);
        }
    }

    /// Walks an ancestor relationship chain upward along `Rel: GuestComponent` starting from
    /// `entity` via `BamHostApi::iter_up_cascade` (slot 58) until locating an ancestor carrying
    /// read-only component `T: GuestComponent` at `term_index`, validating `T`'s layout and UUID.
    ///
    /// Returns `Ok(Some((owner, value)))` if an ancestor carries `T`, `Ok(None)` if no ancestor
    /// carries `T`, or `Err(status)` on resolution, permission, layout, cycle, or unsupported
    /// errors.
    #[inline]
    pub fn up_cascade<Rel: GuestComponent, T: GuestComponent>(
        &mut self,
        entity: FfiEntity,
        term_index: usize,
    ) -> Result<Option<(FfiEntity, &T)>, FfiResult> {
        let host = self.iter.inner.host();
        let rel = resolve_guest_component::<Rel>(host, host.manifest_name())?;
        self.iter.up_cascade::<T>(rel, entity, term_index)
    }

    /// Walks an ancestor relationship chain upward along pre-resolved relationship handle
    /// `relation` starting from `entity` via `BamHostApi::iter_up_cascade` (slot 58) until
    /// locating an ancestor carrying read-only component `T: GuestComponent` at `term_index`.
    #[inline]
    pub fn up_cascade_handle<T: GuestComponent>(
        &mut self,
        relation: BamComponent,
        entity: FfiEntity,
        term_index: usize,
    ) -> Result<Option<(FfiEntity, &T)>, FfiResult> {
        self.iter.up_cascade::<T>(relation, entity, term_index)
    }

    /// Iterates sequentially over all chunks, propagating any `TYPE_MISMATCH` / `ACCESS_CONFLICT`
    /// or callback error.
    pub fn for_each<F, R>(&mut self, mut f: F) -> FfiResult
    where
        F: FnMut(Q::Slices<'_>) -> R,
        R: IntoSystemResult,
    {
        while let Some(chunk_res) = self.try_next_chunk() {
            let slices = match chunk_res {
                Ok(s) => s,
                Err(err) => return err,
            };
            let status = Q::invoke_unpacked(&mut f, slices);
            if !status.is_ok() {
                return status;
            }
        }
        FfiResult::OK
    }

    /// Iterates in parallel over disjoint chunk partitions on the host `ComputeTaskPool`, yielding
    /// typed `Q::Slices<'_>` to `f`.
    pub fn par_for_each<F, R>(&mut self, f: F) -> FfiResult
    where
        F: Fn(Q::Slices<'_>) -> R + Sync,
        R: IntoSystemResult,
    {
        self.iter.inner.par_for_each_chunk(|task_iter| {
            let mut dispenser = ChunkColumnDispenser::new(task_iter);
            let slices = match Q::fetch_slices(&mut dispenser) {
                Ok(s) => s,
                Err(err) => return err,
            };
            let mut adapter = &f;
            Q::invoke_unpacked(&mut adapter, slices)
        })
    }

    /// Iterates in parallel over disjoint chunk partitions on the host `ComputeTaskPool` in
    /// strictly read-only mode via `BamHostApi::par_for_each_chunk_readonly` (slot 59), yielding
    /// typed `Q::Slices<'_>` to `f`.
    ///
    /// Rejects any [`QueryTuple`] declaring writable terms (`&mut T` or `Option<&mut T>`) with
    /// [`FfiResult::PERMISSION_DENIED`] before crossing FFI, and the host also rejects queries
    /// registered with writable terms.
    pub fn par_for_each_chunk_readonly<F, R>(&mut self, f: F) -> FfiResult
    where
        F: Fn(Q::Slices<'_>) -> R + Sync,
        R: IntoSystemResult,
    {
        if !Q::IS_READ_ONLY {
            return FfiResult::PERMISSION_DENIED;
        }
        self.iter.inner.par_for_each_chunk_readonly(|task_iter| {
            let mut dispenser = ChunkColumnDispenser::new(task_iter);
            let slices = match Q::fetch_slices(&mut dispenser) {
                Ok(s) => s,
                Err(err) => return err,
            };
            let mut adapter = &f;
            Q::invoke_unpacked(&mut adapter, slices)
        })
    }

    /// Iterates over all unordered `K`-combinations of matching entities across all chunks
    /// (both within-chunk and cross-chunk) for read-only [`CopyableQueryTuple`] queries,
    /// mirroring Bevy's `Query::iter_combinations::<K>()`.
    ///
    /// Each `K`-tuple is yielded as `[Q::OwnedItem; K]` in lexicographical entity order.
    /// Short-circuits and returns the first non-`OK` [`FfiResult`] from either chunk fetching or
    /// `f`.
    pub fn for_each_combination<const K: usize, F, R>(&mut self, mut f: F) -> FfiResult
    where
        Q: CopyableQueryTuple,
        F: FnMut([Q::OwnedItem; K]) -> R,
        R: IntoSystemResult,
    {
        if K == 0 {
            return FfiResult::OK;
        }
        let mut items: Vec<Q::OwnedItem> = Vec::new();
        while let Some(chunk_len) = self.iter.inner.advance_batch_into(
            &mut self.batch_cache,
            Q::TERM_COUNT,
            Q::NEEDS_ENTITIES,
            Q::MARK_CHANGED_MASK,
        ) {
            if chunk_len == 0 {
                continue;
            }
            let mut dispenser =
                ChunkColumnDispenser::new_with_batch(&mut self.iter.inner, &self.batch_cache);
            let slices = match Q::fetch_slices(&mut dispenser) {
                Ok(s) => s,
                Err(err) => return err,
            };
            Q::push_chunk_items(slices, chunk_len, &mut items);
        }
        let n = items.len();
        if n < K {
            return FfiResult::OK;
        }
        let mut indices: [usize; K] = core::array::from_fn(|i| i);
        loop {
            let combo = core::array::from_fn(|j| items[indices[j]]);
            let status = f(combo).into_ffi_result();
            if !status.is_ok() {
                return status;
            }
            // Advance lexicographical K-combination indices.
            let mut pos = K;
            while pos > 0 {
                pos -= 1;
                if indices[pos] < n - (K - pos) {
                    indices[pos] += 1;
                    for next in (pos + 1)..K {
                        indices[next] = indices[next - 1] + 1;
                    }
                    break;
                }
                if pos == 0 {
                    return FfiResult::OK;
                }
            }
        }
    }

    /// Iterates over all unordered within-chunk pairs `(a, b)` with disjoint mutable borrows
    /// (`slice::split_at_mut`, 100% `#![forbid(unsafe_code)]`, zero allocation), mirroring Bevy's
    /// `Query::iter_combinations_mut::<2>()` on each archetype chunk.
    ///
    /// Short-circuits and returns the first non-`OK` [`FfiResult`] from either chunk fetching or
    /// `f`.
    pub fn for_each_combination_mut<F, R>(&mut self, mut f: F) -> FfiResult
    where
        Q: PairwiseQueryTuple,
        F: FnMut(Q::PairItem<'_>, Q::PairItem<'_>) -> R,
        R: IntoSystemResult,
    {
        while let Some(chunk_len) = self.iter.inner.advance_batch_into(
            &mut self.batch_cache,
            Q::TERM_COUNT,
            Q::NEEDS_ENTITIES,
            Q::MARK_CHANGED_MASK,
        ) {
            if chunk_len < 2 {
                continue;
            }
            let mut dispenser =
                ChunkColumnDispenser::new_with_batch(&mut self.iter.inner, &self.batch_cache);
            let slices = match Q::fetch_slices(&mut dispenser) {
                Ok(s) => s,
                Err(err) => return err,
            };
            let status = Q::for_each_pair_in_chunk(slices, chunk_len, &mut f);
            if !status.is_ok() {
                return status;
            }
        }
        FfiResult::OK
    }
}

/// Sealed trait for read-only [`QueryTerm`] types whose per-row values are `Copy`, enabling
/// cross-chunk [`TypedChunkIter::for_each_combination`].
pub trait CopyableQueryTerm: QueryTerm {
    /// Owned `Copy` per-row item extracted from `Self::Slice<'_>`.
    type OwnedItem: Copy;

    /// Reads the `Copy` item at `row` from `slice`, or `None` if out of bounds.
    fn get_row(slice: &Self::Slice<'_>, row: usize) -> Option<Self::OwnedItem>;
}

impl CopyableQueryTerm for FfiEntity {
    type OwnedItem = FfiEntity;
    #[inline]
    fn get_row(slice: &Self::Slice<'_>, row: usize) -> Option<Self::OwnedItem> {
        slice.get(row).copied()
    }
}

impl<T: GuestComponent> CopyableQueryTerm for &T {
    type OwnedItem = T;
    #[inline]
    fn get_row(slice: &Self::Slice<'_>, row: usize) -> Option<Self::OwnedItem> {
        slice.get(row).copied()
    }
}

impl<T: GuestComponent> CopyableQueryTerm for Option<&T> {
    type OwnedItem = Option<T>;
    #[inline]
    fn get_row(slice: &Self::Slice<'_>, row: usize) -> Option<Self::OwnedItem> {
        match slice {
            Some(s) => s.get(row).copied().map(Some),
            None => Some(None),
        }
    }
}

impl<T: GuestComponent> CopyableQueryTerm for Changed<T> {
    type OwnedItem = T;
    #[inline]
    fn get_row(slice: &Self::Slice<'_>, row: usize) -> Option<Self::OwnedItem> {
        slice.get(row).copied()
    }
}

impl<T: GuestComponent> CopyableQueryTerm for Added<T> {
    type OwnedItem = T;
    #[inline]
    fn get_row(slice: &Self::Slice<'_>, row: usize) -> Option<Self::OwnedItem> {
        slice.get(row).copied()
    }
}

impl<T: GuestComponent> CopyableQueryTerm for Ref<T> {
    type OwnedItem = T;
    #[inline]
    fn get_row(slice: &Self::Slice<'_>, row: usize) -> Option<Self::OwnedItem> {
        slice.as_slice().get(row).copied()
    }
}

impl<T: GuestComponent> CopyableQueryTerm for With<T> {
    type OwnedItem = ();
    #[inline]
    fn get_row(_slice: &Self::Slice<'_>, _row: usize) -> Option<Self::OwnedItem> {
        Some(())
    }
}

impl<T: GuestComponent> CopyableQueryTerm for Without<T> {
    type OwnedItem = ();
    #[inline]
    fn get_row(_slice: &Self::Slice<'_>, _row: usize) -> Option<Self::OwnedItem> {
        Some(())
    }
}

impl<T: GuestComponent> CopyableQueryTerm for WithChanged<T> {
    type OwnedItem = ();
    #[inline]
    fn get_row(_slice: &Self::Slice<'_>, _row: usize) -> Option<Self::OwnedItem> {
        Some(())
    }
}

impl<T: GuestComponent> CopyableQueryTerm for WithAdded<T> {
    type OwnedItem = ();
    #[inline]
    fn get_row(_slice: &Self::Slice<'_>, _row: usize) -> Option<Self::OwnedItem> {
        Some(())
    }
}

impl<T: GuestComponent> CopyableQueryTerm for Allow<T> {
    type OwnedItem = ();
    #[inline]
    fn get_row(_slice: &Self::Slice<'_>, _row: usize) -> Option<Self::OwnedItem> {
        Some(())
    }
}

impl<T: GuestComponent> CopyableQueryTerm for Has<T> {
    type OwnedItem = bool;
    #[inline]
    fn get_row(slice: &Self::Slice<'_>, _row: usize) -> Option<Self::OwnedItem> {
        Some(*slice)
    }
}

impl<T: OrFilterTuple> CopyableQueryTerm for Or<T> {
    type OwnedItem = ();
    #[inline]
    fn get_row(_slice: &Self::Slice<'_>, _row: usize) -> Option<Self::OwnedItem> {
        Some(())
    }
}

/// Sealed trait for read-only [`QueryTuple`] types supporting cross-chunk `K`-combinations via
/// [`TypedChunkIter::for_each_combination`].
pub trait CopyableQueryTuple: QueryTuple {
    /// Owned `Copy` item representing one entity row.
    type OwnedItem: Copy;

    /// Appends all `chunk_len` copied entity items from `slices` into `out`.
    fn push_chunk_items(slices: Self::Slices<'_>, chunk_len: usize, out: &mut Vec<Self::OwnedItem>);
}

macro_rules! impl_single_copyable_tuple {
    ($($lifetime:lifetime,)? $ty_param:ident : $bound:ident => $target:ty) => {
        impl<$($lifetime,)? $ty_param: $bound> CopyableQueryTuple for $target {
            type OwnedItem = <Self as CopyableQueryTerm>::OwnedItem;

            #[inline]
            fn push_chunk_items(
                slices: Self::Slices<'_>,
                chunk_len: usize,
                out: &mut Vec<Self::OwnedItem>,
            ) {
                out.reserve(chunk_len);
                for row in 0..chunk_len {
                    if let Some(item) = <Self as CopyableQueryTerm>::get_row(&slices, row) {
                        out.push(item);
                    }
                }
            }
        }
    };
}

impl CopyableQueryTuple for FfiEntity {
    type OwnedItem = FfiEntity;
    #[inline]
    fn push_chunk_items(
        slices: Self::Slices<'_>,
        chunk_len: usize,
        out: &mut Vec<Self::OwnedItem>,
    ) {
        out.extend_from_slice(&slices[..chunk_len.min(slices.len())]);
    }
}

impl_single_copyable_tuple!('q, T: GuestComponent => &'q T);
impl_single_copyable_tuple!('q, T: GuestComponent => Option<&'q T>);
impl_single_copyable_tuple!(T: GuestComponent => Changed<T>);
impl_single_copyable_tuple!(T: GuestComponent => Added<T>);
impl_single_copyable_tuple!(T: GuestComponent => Ref<T>);
impl_single_copyable_tuple!(T: GuestComponent => With<T>);
impl_single_copyable_tuple!(T: GuestComponent => Without<T>);
impl_single_copyable_tuple!(T: GuestComponent => WithChanged<T>);
impl_single_copyable_tuple!(T: GuestComponent => WithAdded<T>);
impl_single_copyable_tuple!(T: GuestComponent => Allow<T>);
impl_single_copyable_tuple!(T: GuestComponent => Has<T>);
impl_single_copyable_tuple!(T: OrFilterTuple => Or<T>);

macro_rules! impl_copyable_query_tuple {
    ($($term:ident : $var:ident),+ $(,)?) => {
        impl<$($term: CopyableQueryTerm),+> CopyableQueryTuple for ($($term,)+) {
            type OwnedItem = ($($term::OwnedItem,)+);

            #[inline]
            fn push_chunk_items(
                slices: Self::Slices<'_>,
                chunk_len: usize,
                out: &mut Vec<Self::OwnedItem>,
            ) {
                let ($($var,)+) = slices;
                out.reserve(chunk_len);
                for row in 0..chunk_len {
                    if let ($(Some($var),)+) = ($($term::get_row(&$var, row),)+) {
                        out.push(($($var,)+));
                    }
                }
            }
        }
    };
}

impl_copyable_query_tuple!(T0: s0);
impl_copyable_query_tuple!(T0: s0, T1: s1);
impl_copyable_query_tuple!(T0: s0, T1: s1, T2: s2);
impl_copyable_query_tuple!(T0: s0, T1: s1, T2: s2, T3: s3);
impl_copyable_query_tuple!(T0: s0, T1: s1, T2: s2, T3: s3, T4: s4);
impl_copyable_query_tuple!(T0: s0, T1: s1, T2: s2, T3: s3, T4: s4, T5: s5);
impl_copyable_query_tuple!(T0: s0, T1: s1, T2: s2, T3: s3, T4: s4, T5: s5, T6: s6);
impl_copyable_query_tuple!(T0: s0, T1: s1, T2: s2, T3: s3, T4: s4, T5: s5, T6: s6, T7: s7);

/// Sealed trait for [`QueryTerm`] types supporting zero-allocation within-chunk pairwise
/// splitting via `slice::split_at` / `slice::split_at_mut`.
pub trait PairwiseQueryTerm: QueryTerm {
    /// Per-row borrowed item yielded for each half of a pair.
    type PairItem<'a>;

    /// Remaining tail view over rows `(row + 1)..chunk_len`.
    type PairTail<'a>;

    /// Splits row `row` and its suffix `(row + 1)..` out of `slice`.
    fn split_at_row<'a>(
        slice: &'a mut Self::Slice<'_>,
        row: usize,
    ) -> Option<(Self::PairItem<'a>, Self::PairTail<'a>)>;

    /// Reborrows the left-hand item for a single inner-loop pair callback.
    fn reborrow_item<'b, 'a: 'b>(item: &'b mut Self::PairItem<'a>) -> Self::PairItem<'b>;

    /// Pops the next right-hand item from `tail`.
    fn next_tail_item<'a>(tail: &mut Self::PairTail<'a>) -> Option<Self::PairItem<'a>>;
}

impl PairwiseQueryTerm for FfiEntity {
    type PairItem<'a> = FfiEntity;
    type PairTail<'a> = &'a [FfiEntity];

    #[inline]
    fn split_at_row<'a>(
        slice: &'a mut Self::Slice<'_>,
        row: usize,
    ) -> Option<(Self::PairItem<'a>, Self::PairTail<'a>)> {
        let mid = row.checked_add(1)?;
        if mid > slice.len() {
            return None;
        }
        let (left, right) = slice.split_at(mid);
        Some((left[row], right))
    }

    #[inline]
    fn reborrow_item<'b, 'a: 'b>(item: &'b mut Self::PairItem<'a>) -> Self::PairItem<'b> {
        *item
    }

    #[inline]
    fn next_tail_item<'a>(tail: &mut Self::PairTail<'a>) -> Option<Self::PairItem<'a>> {
        let (&first, rest) = tail.split_first()?;
        *tail = rest;
        Some(first)
    }
}

macro_rules! impl_pairwise_read_slice {
    ($($lifetime:lifetime,)? $ty_param:ident => $target:ty) => {
        impl<$($lifetime,)? $ty_param: GuestComponent> PairwiseQueryTerm for $target {
            type PairItem<'a> = &'a $ty_param;
            type PairTail<'a> = &'a [$ty_param];

            #[inline]
            fn split_at_row<'a>(
                slice: &'a mut Self::Slice<'_>,
                row: usize,
            ) -> Option<(Self::PairItem<'a>, Self::PairTail<'a>)> {
                let mid = row.checked_add(1)?;
                if mid > slice.len() {
                    return None;
                }
                let (left, right) = slice.split_at(mid);
                Some((&left[row], right))
            }

            #[inline]
            fn reborrow_item<'b, 'a: 'b>(item: &'b mut Self::PairItem<'a>) -> Self::PairItem<'b> {
                *item
            }

            #[inline]
            fn next_tail_item<'a>(tail: &mut Self::PairTail<'a>) -> Option<Self::PairItem<'a>> {
                let (first, rest) = tail.split_first()?;
                *tail = rest;
                Some(first)
            }
        }
    };
}

macro_rules! impl_pairwise_write_slice {
    ($($lifetime:lifetime,)? $ty_param:ident => $target:ty) => {
        impl<$($lifetime,)? $ty_param: GuestComponent> PairwiseQueryTerm for $target {
            type PairItem<'a> = &'a mut $ty_param;
            type PairTail<'a> = &'a mut [$ty_param];

            #[inline]
            fn split_at_row<'a>(
                slice: &'a mut Self::Slice<'_>,
                row: usize,
            ) -> Option<(Self::PairItem<'a>, Self::PairTail<'a>)> {
                let mid = row.checked_add(1)?;
                if mid > slice.len() {
                    return None;
                }
                let (left, right) = slice.split_at_mut(mid);
                Some((&mut left[row], right))
            }

            #[inline]
            fn reborrow_item<'b, 'a: 'b>(item: &'b mut Self::PairItem<'a>) -> Self::PairItem<'b> {
                *item
            }

            #[inline]
            fn next_tail_item<'a>(tail: &mut Self::PairTail<'a>) -> Option<Self::PairItem<'a>> {
                let slice = core::mem::take(tail);
                let (first, rest) = slice.split_first_mut()?;
                *tail = rest;
                Some(first)
            }
        }
    };
}

impl_pairwise_read_slice!('q, T => &'q T);
impl_pairwise_read_slice!(T => Changed<T>);
impl_pairwise_read_slice!(T => Added<T>);
impl_pairwise_write_slice!('q, T => &'q mut T);
impl_pairwise_write_slice!(T => ChangedMut<T>);
impl_pairwise_write_slice!(T => AddedMut<T>);

impl<T: GuestComponent> PairwiseQueryTerm for Ref<T> {
    type PairItem<'a> = &'a T;
    type PairTail<'a> = &'a [T];

    #[inline]
    fn split_at_row<'a>(
        slice: &'a mut Self::Slice<'_>,
        row: usize,
    ) -> Option<(Self::PairItem<'a>, Self::PairTail<'a>)> {
        let data = slice.as_slice();
        let mid = row.checked_add(1)?;
        if mid > data.len() {
            return None;
        }
        let (left, right) = data.split_at(mid);
        Some((&left[row], right))
    }

    #[inline]
    fn reborrow_item<'b, 'a: 'b>(item: &'b mut Self::PairItem<'a>) -> Self::PairItem<'b> {
        *item
    }

    #[inline]
    fn next_tail_item<'a>(tail: &mut Self::PairTail<'a>) -> Option<Self::PairItem<'a>> {
        let (first, rest) = tail.split_first()?;
        *tail = rest;
        Some(first)
    }
}

impl<T: GuestComponent> PairwiseQueryTerm for Mut<T> {
    type PairItem<'a> = &'a mut T;
    type PairTail<'a> = &'a mut [T];

    #[inline]
    fn split_at_row<'a>(
        slice: &'a mut Self::Slice<'_>,
        row: usize,
    ) -> Option<(Self::PairItem<'a>, Self::PairTail<'a>)> {
        let data = slice.as_mut_slice();
        let mid = row.checked_add(1)?;
        if mid > data.len() {
            return None;
        }
        let (left, right) = data.split_at_mut(mid);
        Some((&mut left[row], right))
    }

    #[inline]
    fn reborrow_item<'b, 'a: 'b>(item: &'b mut Self::PairItem<'a>) -> Self::PairItem<'b> {
        *item
    }

    #[inline]
    fn next_tail_item<'a>(tail: &mut Self::PairTail<'a>) -> Option<Self::PairItem<'a>> {
        let slice = core::mem::take(tail);
        let (first, rest) = slice.split_first_mut()?;
        *tail = rest;
        Some(first)
    }
}

impl<T: GuestComponent> PairwiseQueryTerm for Option<&T> {
    type PairItem<'a> = Option<&'a T>;
    type PairTail<'a> = Option<&'a [T]>;

    #[inline]
    fn split_at_row<'a>(
        slice: &'a mut Self::Slice<'_>,
        row: usize,
    ) -> Option<(Self::PairItem<'a>, Self::PairTail<'a>)> {
        match slice {
            Some(s) => {
                let mid = row.checked_add(1)?;
                if mid > s.len() {
                    return None;
                }
                let (left, right) = s.split_at(mid);
                Some((Some(&left[row]), Some(right)))
            }
            None => Some((None, None)),
        }
    }

    #[inline]
    fn reborrow_item<'b, 'a: 'b>(item: &'b mut Self::PairItem<'a>) -> Self::PairItem<'b> {
        *item
    }

    #[inline]
    fn next_tail_item<'a>(tail: &mut Self::PairTail<'a>) -> Option<Self::PairItem<'a>> {
        match tail {
            Some(s) => {
                let (first, rest) = s.split_first()?;
                *s = rest;
                Some(Some(first))
            }
            None => Some(None),
        }
    }
}

impl<T: GuestComponent> PairwiseQueryTerm for Option<&mut T> {
    type PairItem<'a> = Option<&'a mut T>;
    type PairTail<'a> = Option<&'a mut [T]>;

    #[inline]
    fn split_at_row<'a>(
        slice: &'a mut Self::Slice<'_>,
        row: usize,
    ) -> Option<(Self::PairItem<'a>, Self::PairTail<'a>)> {
        match slice {
            Some(s) => {
                let mid = row.checked_add(1)?;
                if mid > s.len() {
                    return None;
                }
                let (left, right) = s.split_at_mut(mid);
                Some((Some(&mut left[row]), Some(right)))
            }
            None => Some((None, None)),
        }
    }

    #[inline]
    fn reborrow_item<'b, 'a: 'b>(item: &'b mut Self::PairItem<'a>) -> Self::PairItem<'b> {
        item.as_deref_mut()
    }

    #[inline]
    fn next_tail_item<'a>(tail: &mut Self::PairTail<'a>) -> Option<Self::PairItem<'a>> {
        match tail {
            Some(s) => {
                let slice = core::mem::take(s);
                let (first, rest) = slice.split_first_mut()?;
                *s = rest;
                Some(Some(first))
            }
            None => Some(None),
        }
    }
}

macro_rules! impl_pairwise_copy_marker {
    ($bound:ident => $target:ty, $item:ty) => {
        impl<T: $bound> PairwiseQueryTerm for $target {
            type PairItem<'a> = $item;
            type PairTail<'a> = $item;

            #[inline]
            fn split_at_row<'a>(
                slice: &'a mut Self::Slice<'_>,
                _row: usize,
            ) -> Option<(Self::PairItem<'a>, Self::PairTail<'a>)> {
                Some((*slice, *slice))
            }

            #[inline]
            fn reborrow_item<'b, 'a: 'b>(item: &'b mut Self::PairItem<'a>) -> Self::PairItem<'b> {
                *item
            }

            #[inline]
            fn next_tail_item<'a>(tail: &mut Self::PairTail<'a>) -> Option<Self::PairItem<'a>> {
                Some(*tail)
            }
        }
    };
}

impl_pairwise_copy_marker!(GuestComponent => With<T>, ());
impl_pairwise_copy_marker!(GuestComponent => Without<T>, ());
impl_pairwise_copy_marker!(GuestComponent => WithChanged<T>, ());
impl_pairwise_copy_marker!(GuestComponent => WithAdded<T>, ());
impl_pairwise_copy_marker!(GuestComponent => Allow<T>, ());
impl_pairwise_copy_marker!(GuestComponent => Has<T>, bool);
impl_pairwise_copy_marker!(OrFilterTuple => Or<T>, ());

/// Sealed trait for [`QueryTuple`] types supporting zero-allocation within-chunk mutable pairwise
/// iteration via [`TypedChunkIter::for_each_combination_mut`].
pub trait PairwiseQueryTuple: QueryTuple {
    /// Per-row borrowed item tuple yielded for each entity in a pair.
    type PairItem<'a>;

    /// Invokes `f(a, b)` for every unordered pair `0 <= i < j < chunk_len` inside the active chunk.
    fn for_each_pair_in_chunk<F, R>(
        slices: Self::Slices<'_>,
        chunk_len: usize,
        f: &mut F,
    ) -> FfiResult
    where
        F: FnMut(Self::PairItem<'_>, Self::PairItem<'_>) -> R,
        R: IntoSystemResult;
}

macro_rules! impl_single_pairwise_tuple {
    ($($lifetime:lifetime,)? $ty_param:ident : $bound:ident => $target:ty) => {
        impl<$($lifetime,)? $ty_param: $bound> PairwiseQueryTuple for $target {
            type PairItem<'a> = <Self as PairwiseQueryTerm>::PairItem<'a>;

            fn for_each_pair_in_chunk<F, R>(
                mut slices: Self::Slices<'_>,
                chunk_len: usize,
                f: &mut F,
            ) -> FfiResult
            where
                F: FnMut(Self::PairItem<'_>, Self::PairItem<'_>) -> R,
                R: IntoSystemResult,
            {
                for row in 0..chunk_len.saturating_sub(1) {
                    let Some((mut left, mut right)) =
                        <Self as PairwiseQueryTerm>::split_at_row(&mut slices, row)
                    else {
                        break;
                    };
                    for _ in (row + 1)..chunk_len {
                        let Some(b) = <Self as PairwiseQueryTerm>::next_tail_item(&mut right) else {
                            break;
                        };
                        let a = <Self as PairwiseQueryTerm>::reborrow_item(&mut left);
                        let status = f(a, b).into_ffi_result();
                        if !status.is_ok() {
                            return status;
                        }
                    }
                }
                FfiResult::OK
            }
        }
    };
}

impl PairwiseQueryTuple for FfiEntity {
    type PairItem<'a> = FfiEntity;

    fn for_each_pair_in_chunk<F, R>(
        mut slices: Self::Slices<'_>,
        chunk_len: usize,
        f: &mut F,
    ) -> FfiResult
    where
        F: FnMut(Self::PairItem<'_>, Self::PairItem<'_>) -> R,
        R: IntoSystemResult,
    {
        for row in 0..chunk_len.saturating_sub(1) {
            let Some((mut left, mut right)) =
                <Self as PairwiseQueryTerm>::split_at_row(&mut slices, row)
            else {
                break;
            };
            for _ in (row + 1)..chunk_len {
                let Some(b) = <Self as PairwiseQueryTerm>::next_tail_item(&mut right) else {
                    break;
                };
                let a = <Self as PairwiseQueryTerm>::reborrow_item(&mut left);
                let status = f(a, b).into_ffi_result();
                if !status.is_ok() {
                    return status;
                }
            }
        }
        FfiResult::OK
    }
}

impl_single_pairwise_tuple!('q, T: GuestComponent => &'q T);
impl_single_pairwise_tuple!('q, T: GuestComponent => &'q mut T);
impl_single_pairwise_tuple!('q, T: GuestComponent => Option<&'q T>);
impl_single_pairwise_tuple!('q, T: GuestComponent => Option<&'q mut T>);
impl_single_pairwise_tuple!(T: GuestComponent => Changed<T>);
impl_single_pairwise_tuple!(T: GuestComponent => Added<T>);
impl_single_pairwise_tuple!(T: GuestComponent => ChangedMut<T>);
impl_single_pairwise_tuple!(T: GuestComponent => AddedMut<T>);
impl_single_pairwise_tuple!(T: GuestComponent => Ref<T>);
impl_single_pairwise_tuple!(T: GuestComponent => Mut<T>);
impl_single_pairwise_tuple!(T: GuestComponent => With<T>);
impl_single_pairwise_tuple!(T: GuestComponent => Without<T>);
impl_single_pairwise_tuple!(T: GuestComponent => WithChanged<T>);
impl_single_pairwise_tuple!(T: GuestComponent => WithAdded<T>);
impl_single_pairwise_tuple!(T: GuestComponent => Allow<T>);
impl_single_pairwise_tuple!(T: GuestComponent => Has<T>);
impl_single_pairwise_tuple!(T: OrFilterTuple => Or<T>);

macro_rules! impl_pairwise_query_tuple {
    ($($term:ident : $s:ident : $l:ident : $r:ident : $a:ident : $b:ident),+ $(,)?) => {
        impl<$($term: PairwiseQueryTerm),+> PairwiseQueryTuple for ($($term,)+) {
            type PairItem<'a> = ($($term::PairItem<'a>,)+);

            fn for_each_pair_in_chunk<F, R>(
                slices: Self::Slices<'_>,
                chunk_len: usize,
                f: &mut F,
            ) -> FfiResult
            where
                F: FnMut(Self::PairItem<'_>, Self::PairItem<'_>) -> R,
                R: IntoSystemResult,
            {
                let ($(mut $s,)+) = slices;
                for row in 0..chunk_len.saturating_sub(1) {
                    $(
                        let Some((mut $l, mut $r)) = $term::split_at_row(&mut $s, row) else {
                            break;
                        };
                    )+
                    for _ in (row + 1)..chunk_len {
                        $(
                            let Some($b) = $term::next_tail_item(&mut $r) else {
                                break;
                            };
                        )+
                        $(
                            let $a = $term::reborrow_item(&mut $l);
                        )+
                        let status = f(($($a,)+), ($($b,)+)).into_ffi_result();
                        if !status.is_ok() {
                            return status;
                        }
                    }
                }
                FfiResult::OK
            }
        }
    };
}

impl_pairwise_query_tuple!(T0: s0: l0: r0: a0: b0);
impl_pairwise_query_tuple!(T0: s0: l0: r0: a0: b0, T1: s1: l1: r1: a1: b1);
impl_pairwise_query_tuple!(T0: s0: l0: r0: a0: b0, T1: s1: l1: r1: a1: b1, T2: s2: l2: r2: a2: b2);
impl_pairwise_query_tuple!(T0: s0: l0: r0: a0: b0, T1: s1: l1: r1: a1: b1, T2: s2: l2: r2: a2: b2, T3: s3: l3: r3: a3: b3);
impl_pairwise_query_tuple!(T0: s0: l0: r0: a0: b0, T1: s1: l1: r1: a1: b1, T2: s2: l2: r2: a2: b2, T3: s3: l3: r3: a3: b3, T4: s4: l4: r4: a4: b4);
impl_pairwise_query_tuple!(T0: s0: l0: r0: a0: b0, T1: s1: l1: r1: a1: b1, T2: s2: l2: r2: a2: b2, T3: s3: l3: r3: a3: b3, T4: s4: l4: r4: a4: b4, T5: s5: l5: r5: a5: b5);
impl_pairwise_query_tuple!(T0: s0: l0: r0: a0: b0, T1: s1: l1: r1: a1: b1, T2: s2: l2: r2: a2: b2, T3: s3: l3: r3: a3: b3, T4: s4: l4: r4: a4: b4, T5: s5: l5: r5: a5: b5, T6: s6: l6: r6: a6: b6);
impl_pairwise_query_tuple!(T0: s0: l0: r0: a0: b0, T1: s1: l1: r1: a1: b1, T2: s2: l2: r2: a2: b2, T3: s3: l3: r3: a3: b3, T4: s4: l4: r4: a4: b4, T5: s5: l5: r5: a5: b5, T6: s6: l6: r6: a6: b6, T7: s7: l7: r7: a7: b7);
