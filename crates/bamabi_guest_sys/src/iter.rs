//! Validated chunk iteration, `BamColumnInfo` layout/UUID verification, dense slices, indirect
//! sparse-set row views, untracked mutation + `mark_changed_range`, and singleton resource access.

use core::{
    cell::{Cell, UnsafeCell},
    ffi::c_void,
    marker::PhantomData,
    mem::MaybeUninit,
    ops::Index,
    sync::atomic::{AtomicU64, Ordering},
};
use std::ffi::CString;

use bamabi_sys::{
    AbiPod, AccessMode, BamChangeTicks, BamColumnInfo, BamComponent, BamIter, ComponentUuid,
    FfiBool, FfiEntity, FfiResult, FfiSlice, MAX_TERMS_PER_QUERY, ModComponent, StrView,
};

use crate::{
    host::{Host, ValidatedStringVTable, ValidatedWireVTable},
    instance::qualify_mod_component_identity,
    panic::catch_guest_result,
    trampoline::LazyInvocationHost,
};

/// Expected [`ComponentUuid`] predicate for typed column and resource introspection.
///
/// Supports either a single exact UUID or a primary manifest-qualified UUID with an unqualified
/// fallback (matching `auto-namespaced `GuestComponent` resolution).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UuidFilter {
    primary: ComponentUuid,
    fallback: Option<ComponentUuid>,
}

impl UuidFilter {
    /// Matches only `uuid`.
    #[inline]
    pub const fn exact(uuid: ComponentUuid) -> Self {
        Self {
            primary: uuid,
            fallback: None,
        }
    }

    /// Matches either `primary` (e.g. `"<manifest>::<Name>"`) or `fallback` (e.g. `"<Name>"`).
    #[inline]
    pub const fn with_fallback(primary: ComponentUuid, fallback: ComponentUuid) -> Self {
        Self {
            primary,
            fallback: Some(fallback),
        }
    }

    /// Returns `true` if `actual` matches `primary` or `fallback`.
    #[inline]
    pub fn matches(self, actual: ComponentUuid) -> bool {
        actual == self.primary || self.fallback.is_some_and(|fb| actual == fb)
    }
}

impl From<ComponentUuid> for UuidFilter {
    /// Converts a single [`ComponentUuid`] into an exact [`UuidFilter`].
    #[inline]
    fn from(uuid: ComponentUuid) -> Self {
        Self::exact(uuid)
    }
}

/// Validates that `info` describes a readable dense column matching `T: AbiPod` (access mode,
/// element size/alignment, and optional UUID filter) before exposing a shared `&[T]` slice.
#[inline]
fn validate_dense_read_info<T: AbiPod>(
    info: &BamColumnInfo,
    expected_uuid: Option<UuidFilter>,
) -> bool {
    if !info.mode.has_entity_column_data() || info.is_resource() || info.is_sparse_set() {
        return false;
    }
    if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
        return false;
    }
    if let Some(filter) = expected_uuid
        && !filter.matches(info.uuid)
    {
        return false;
    }
    true
}

/// Validates that `info` describes a writable dense column matching `T: AbiPod` (mutable access
/// mode, non-immutable flag, element size/alignment, and optional UUID filter) before exposing
/// an exclusive `&mut [T]` slice.
#[inline]
fn validate_dense_write_info<T: AbiPod>(
    info: &BamColumnInfo,
    expected_uuid: Option<UuidFilter>,
) -> bool {
    if !info.mode.is_mutable()
        || !info.mode.has_entity_column_data()
        || info.is_immutable()
        || info.is_resource()
        || info.is_sparse_set()
    {
        return false;
    }
    if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
        return false;
    }
    if let Some(filter) = expected_uuid
        && !filter.matches(info.uuid)
    {
        return false;
    }
    true
}

/// Validates that `info` describes a readable row-indexed (sparse-set) column matching `T: AbiPod`
/// before constructing a [`SparseRows`] view.
#[inline]
fn validate_rows_read_info<T: AbiPod>(
    info: &BamColumnInfo,
    expected_uuid: Option<UuidFilter>,
) -> bool {
    if !info.mode.has_entity_column_data() || info.is_resource() {
        return false;
    }
    if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
        return false;
    }
    if let Some(filter) = expected_uuid
        && !filter.matches(info.uuid)
    {
        return false;
    }
    true
}

/// Validates that `info` describes a writable row-indexed (sparse-set) column matching `T: AbiPod`
/// (rejecting immutable columns) before constructing a [`SparseRowsMut`] view.
#[inline]
fn validate_rows_write_info<T: AbiPod>(
    info: &BamColumnInfo,
    expected_uuid: Option<UuidFilter>,
) -> bool {
    if !info.mode.is_mutable()
        || !info.mode.has_entity_column_data()
        || info.is_immutable()
        || info.is_resource()
    {
        return false;
    }
    if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
        return false;
    }
    if let Some(filter) = expected_uuid
        && !filter.matches(info.uuid)
    {
        return false;
    }
    true
}

/// Validates that `info` describes a readable singleton resource matching `R: AbiPod` (resource
/// access mode, size/alignment, and optional UUID filter).
#[inline]
fn validate_resource_read_info<R: AbiPod>(
    info: &BamColumnInfo,
    expected_uuid: Option<UuidFilter>,
) -> bool {
    if !info.mode.is_resource() {
        return false;
    }
    if info.size != core::mem::size_of::<R>() || info.align != core::mem::align_of::<R>() {
        return false;
    }
    if let Some(filter) = expected_uuid
        && !filter.matches(info.uuid)
    {
        return false;
    }
    true
}

/// Validates that `info` describes a writable singleton resource matching `R: AbiPod` (exclusive
/// write access, non-immutable, size/alignment, and optional UUID filter).
#[inline]
fn validate_resource_write_info<R: AbiPod>(
    info: &BamColumnInfo,
    expected_uuid: Option<UuidFilter>,
) -> bool {
    if info.mode != AccessMode::RESOURCE_WRITE || info.is_immutable() {
        return false;
    }
    if info.size != core::mem::size_of::<R>() || info.align != core::mem::align_of::<R>() {
        return false;
    }
    if let Some(filter) = expected_uuid
        && !filter.matches(info.uuid)
    {
        return false;
    }
    true
}

/// Reinterprets a pre-validated dense column pointer `(ptr, len)` as `&'a [T]` without repeating
/// null, alignment, or `isize::MAX` byte-length checks.
///
/// # Safety
/// - If `size_of::<T>() == 0`, `ptr` must be non-null when `len > 0` (the returned slice uses
///   `NonNull::dangling()`).
/// - If `size_of::<T>() > 0` and `len > 0`, `ptr` must be non-null, aligned to `align_of::<T>()`,
///   `len * size_of::<T>() <= isize::MAX`, and `ptr` must point to `len` contiguous initialized
///   elements of `T` valid for shared reads during `'a`.
#[inline(always)]
pub(crate) unsafe fn raw_dense_slice_unchecked<'a, T: AbiPod>(
    ptr: *const u8,
    len: usize,
) -> &'a [T] {
    if core::mem::size_of::<T>() == 0 {
        // SAFETY: `T: AbiPod` is a zero-sized type; `NonNull::dangling()` is aligned and valid for
        // any `len` without dereferencing memory.
        return unsafe {
            core::slice::from_raw_parts(core::ptr::NonNull::<T>::dangling().as_ptr(), len)
        };
    }
    if len == 0 {
        return &[];
    }
    // SAFETY: Caller guarantees `ptr` is non-null, aligned for `T`, `len * size_of::<T>() <= isize::MAX`,
    // and points to `len` contiguous initialized `T` elements valid for `'a`.
    unsafe { core::slice::from_raw_parts(ptr.cast::<T>(), len) }
}

/// Reinterprets a pre-validated writable dense column pointer `(ptr, len)` as `&'a mut [T]`
/// without repeating null, alignment, or `isize::MAX` byte-length checks.
///
/// # Safety
/// - If `size_of::<T>() == 0`, `ptr` must be non-null when `len > 0` (the returned slice uses
///   `NonNull::dangling()`).
/// - If `size_of::<T>() > 0` and `len > 0`, `ptr` must be non-null, aligned to `align_of::<T>()`,
///   `len * size_of::<T>() <= isize::MAX`, and `ptr` must point to `len` contiguous exclusively
///   accessible, initialized elements of `T` valid for `'a`.
#[inline(always)]
pub(crate) unsafe fn raw_dense_slice_mut_unchecked<'a, T: AbiPod>(
    ptr: *mut u8,
    len: usize,
) -> &'a mut [T] {
    if core::mem::size_of::<T>() == 0 {
        // SAFETY: `T: AbiPod` is zero-sized; `NonNull::dangling()` forms a sound mutable ZST slice.
        return unsafe {
            core::slice::from_raw_parts_mut(core::ptr::NonNull::<T>::dangling().as_ptr(), len)
        };
    }
    if len == 0 {
        return &mut [];
    }
    // SAFETY: Caller guarantees `ptr` is non-null, aligned for `T`, `len * size_of::<T>() <= isize::MAX`,
    // and points to `len` contiguous exclusively accessible `T` elements valid for `'a`.
    unsafe { core::slice::from_raw_parts_mut(ptr.cast::<T>(), len) }
}

/// Reinterprets a host-supplied dense column pointer `(ptr, len)` as `&'a [T]` after verifying
/// alignment, non-nullness (for non-empty slices), ZST rules, and `isize::MAX` byte-length bounds.
///
/// # Safety
/// `ptr` must point to `len` initialized, validly readable elements of `T` for lifetime `'a`.
#[inline]
pub(crate) unsafe fn raw_dense_slice<'a, T: AbiPod>(ptr: *const u8, len: usize) -> Option<&'a [T]> {
    let elem_size = core::mem::size_of::<T>();
    if elem_size == 0 {
        if ptr.is_null() && len > 0 {
            return None;
        }
        // SAFETY: `T: AbiPod` is a zero-sized type and `ptr` is non-null when `len > 0`.
        return Some(unsafe { raw_dense_slice_unchecked::<T>(ptr, len) });
    }
    if ptr.is_null() {
        return None;
    }
    if len == 0 {
        return Some(&[]);
    }
    if !(ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
        return None;
    }
    let byte_len = len.checked_mul(elem_size)?;
    if byte_len > isize::MAX as usize {
        return None;
    }
    // SAFETY: Verified non-null, aligned for `T`, `byte_len <= isize::MAX`, and caller guarantees
    // `ptr` points to `len` contiguous initialized `T` elements valid for `'a`.
    Some(unsafe { raw_dense_slice_unchecked::<T>(ptr, len) })
}

/// Reinterprets a host-supplied writable dense column pointer `(ptr, len)` as `&'a mut [T]` after
/// verifying alignment, non-nullness, ZST rules, and `isize::MAX` byte-length bounds.
///
/// # Safety
/// `ptr` must point to `len` exclusively accessible, initialized elements of `T` for lifetime `'a`.
#[inline]
pub(crate) unsafe fn raw_dense_slice_mut<'a, T: AbiPod>(
    ptr: *mut u8,
    len: usize,
) -> Option<&'a mut [T]> {
    let elem_size = core::mem::size_of::<T>();
    if elem_size == 0 {
        if ptr.is_null() && len > 0 {
            return None;
        }
        // SAFETY: `T: AbiPod` is zero-sized and `ptr` is non-null when `len > 0`.
        return Some(unsafe { raw_dense_slice_mut_unchecked::<T>(ptr, len) });
    }
    if ptr.is_null() {
        return None;
    }
    if len == 0 {
        return Some(&mut []);
    }
    if !(ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
        return None;
    }
    let byte_len = len.checked_mul(elem_size)?;
    if byte_len > isize::MAX as usize {
        return None;
    }
    // SAFETY: Verified non-null, aligned for `T`, `byte_len <= isize::MAX`, and caller guarantees
    // exclusive mutable access to `len` contiguous `T` elements valid for `'a`.
    Some(unsafe { raw_dense_slice_mut_unchecked::<T>(ptr, len) })
}

/// Validates strided/dense slice metadata (`valid`, element size/alignment, stride compatibility,
/// and byte-range overflow) for a host-returned `BamSliceMeta`.
#[inline]
fn validate_slice_meta<T: AbiPod>(
    valid: bool,
    expected_size: Option<usize>,
    expected_align: Option<usize>,
    uuid: Option<ComponentUuid>,
    expected_uuid: Option<UuidFilter>,
) -> bool {
    if !valid {
        return false;
    }
    if let Some(filter) = expected_uuid {
        let Some(actual_uuid) = uuid else {
            return false;
        };
        if !filter.matches(actual_uuid) {
            return false;
        }
    }
    let Some(sz) = expected_size else {
        return false;
    };
    if sz != core::mem::size_of::<T>() {
        return false;
    }
    if let Some(al) = expected_align
        && al != core::mem::align_of::<T>()
    {
        return false;
    }
    true
}

/// Computes the checked byte offset `index * elem_stride` within a validated strided slice,
/// returning `None` if `!valid`, `index >= len`, or the multiplication overflows `isize::MAX`.
#[inline]
pub(crate) fn checked_slice_elem_offset(
    valid: bool,
    len: usize,
    expected_size: Option<usize>,
    index: usize,
    elem_stride: usize,
) -> Option<usize> {
    if !valid || index >= len {
        return None;
    }
    let expected = expected_size?;
    if expected != elem_stride {
        return None;
    }
    let byte_offset = index.checked_mul(elem_stride)?;
    let total = len.checked_mul(elem_stride)?;
    if total > isize::MAX as usize {
        return None;
    }
    Some(byte_offset)
}

/// Validated read-only dense column view carrying [`BamColumnInfo`] or explicit stride metadata.
pub struct ValidatedSlice<'a> {
    ptr: *const u8,
    len: usize,
    expected_size: Option<usize>,
    expected_align: Option<usize>,
    uuid: Option<ComponentUuid>,
    manifest_name: &'a str,
    valid: bool,
    _marker: PhantomData<&'a [u8]>,
}

impl<'a> ValidatedSlice<'a> {
    /// Constructs an empty/invalid slice whose typed conversions return `None`.
    #[inline]
    pub const fn invalid() -> Self {
        Self {
            ptr: core::ptr::null(),
            len: 0,
            expected_size: None,
            expected_align: None,
            uuid: None,
            manifest_name: "",
            valid: false,
            _marker: PhantomData,
        }
    }

    /// Constructs a [`ValidatedSlice`] from raw parts with exact element byte `stride` (`0` means
    /// zero-sized elements only; non-ZST typed casts are rejected when `stride == 0`).
    ///
    /// # Safety
    /// If `ptr` is non-null and `len > 0` and `stride > 0`, `ptr` must remain valid for shared
    /// reads of `len` contiguous elements of `stride` bytes for lifetime `'a`.
    #[inline]
    pub const unsafe fn from_raw_parts(ptr: *const u8, len: usize, stride: usize) -> Self {
        Self {
            ptr,
            len,
            expected_size: Some(stride),
            expected_align: None,
            uuid: None,
            manifest_name: "",
            valid: !ptr.is_null() || len == 0,
            _marker: PhantomData,
        }
    }

    /// Constructs a [`ValidatedSlice`] stamped with host [`BamColumnInfo`] layout and UUID metadata.
    ///
    /// # Safety
    /// If `ptr` is non-null and `len > 0`, `ptr` must remain valid for shared reads of `len`
    /// contiguous elements described by `info` for lifetime `'a`.
    #[inline]
    pub const unsafe fn from_raw_parts_with_info(
        ptr: *const u8,
        len: usize,
        info: BamColumnInfo,
    ) -> Self {
        let ok_mode =
            info.mode.has_entity_column_data() && !info.is_resource() && !info.is_sparse_set();
        Self {
            ptr,
            len,
            expected_size: Some(info.size),
            expected_align: Some(info.align),
            uuid: Some(info.uuid),
            manifest_name: "",
            valid: ok_mode && (!ptr.is_null() || (len == 0 && !info.mode.is_optional())),
            _marker: PhantomData,
        }
    }

    /// Stamps this slice with the owning mod instance's `manifest_name` for auto-namespaced
    /// `GuestComponent` UUID matching.
    #[inline]
    pub const fn with_manifest_name(mut self, manifest_name: &'a str) -> Self {
        self.manifest_name = manifest_name;
        self
    }

    /// Returns the owning mod instance's manifest name (`""` if unspecified).
    #[inline]
    pub const fn manifest_name(&self) -> &'a str {
        self.manifest_name
    }

    /// Returns the number of elements in this chunk view (`0` if invalid/absent).
    #[inline]
    pub const fn len(&self) -> usize {
        if self.valid { self.len } else { 0 }
    }

    /// Returns `true` if `len() == 0`.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the raw base byte pointer.
    #[inline]
    pub const fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Returns the known element byte stride (`0` if invalid or ZST).
    #[inline]
    pub const fn stride(&self) -> usize {
        match self.expected_size {
            Some(s) => s,
            None => 0,
        }
    }

    /// Returns the column's [`ComponentUuid`] if constructed with [`BamColumnInfo`].
    #[inline]
    pub const fn uuid(&self) -> Option<ComponentUuid> {
        self.uuid
    }

    /// Computes a bounds-checked element pointer at `index` using `elem_stride` bytes per element.
    #[inline]
    pub fn get_ptr(&self, index: usize, elem_stride: usize) -> Option<*const u8> {
        if self.ptr.is_null() {
            return None;
        }
        let byte_offset = checked_slice_elem_offset(
            self.valid,
            self.len,
            self.expected_size,
            index,
            elem_stride,
        )?;
        // SAFETY: `index < self.len`, `total <= isize::MAX`, and `self.ptr` covers `total` bytes.
        Some(unsafe { self.ptr.add(byte_offset) })
    }

    /// Casts this column view to `&[T]` after verifying size, alignment, and optional `expected_uuid`.
    #[inline]
    pub fn as_typed_slice_checked<T: AbiPod>(
        &self,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&'a [T]> {
        if !validate_slice_meta::<T>(
            self.valid,
            self.expected_size,
            self.expected_align,
            self.uuid,
            expected_uuid,
        ) {
            return None;
        }
        if self.len == 0 && (!self.ptr.is_null() || core::mem::size_of::<T>() == 0) {
            return Some(&[]);
        }
        // SAFETY: Layout/UUID checks succeeded and `self.ptr` covers `self.len` elements for `'a`.
        unsafe { raw_dense_slice::<T>(self.ptr, self.len) }
    }

    /// Casts this column view to `&[T]` (`T: AbiPod`), validating size and alignment.
    #[inline]
    pub fn as_typed_slice<T: AbiPod>(&self) -> Option<&'a [T]> {
        self.as_typed_slice_checked::<T>(None)
    }
}

/// Validated mutable dense column view carrying [`BamColumnInfo`] or explicit stride metadata.
pub struct ValidatedSliceMut<'a> {
    ptr: *mut u8,
    len: usize,
    expected_size: Option<usize>,
    expected_align: Option<usize>,
    uuid: Option<ComponentUuid>,
    manifest_name: &'a str,
    valid: bool,
    _marker: PhantomData<&'a mut [u8]>,
}

impl<'a> ValidatedSliceMut<'a> {
    /// Constructs an invalid mutable slice whose typed conversions return `None`.
    #[inline]
    pub const fn invalid() -> Self {
        Self {
            ptr: core::ptr::null_mut(),
            len: 0,
            expected_size: None,
            expected_align: None,
            uuid: None,
            manifest_name: "",
            valid: false,
            _marker: PhantomData,
        }
    }

    /// Constructs a [`ValidatedSliceMut`] from raw parts with exact element byte `stride` (`0`
    /// means zero-sized elements only; non-ZST typed casts are rejected when `stride == 0`).
    ///
    /// # Safety
    /// If `ptr` is non-null and `len > 0` and `stride > 0`, `ptr` must remain valid for exclusive
    /// mutable reads and writes of `len` contiguous elements of `stride` bytes for lifetime `'a`.
    #[inline]
    pub const unsafe fn from_raw_parts(ptr: *mut u8, len: usize, stride: usize) -> Self {
        Self {
            ptr,
            len,
            expected_size: Some(stride),
            expected_align: None,
            uuid: None,
            manifest_name: "",
            valid: !ptr.is_null() || len == 0,
            _marker: PhantomData,
        }
    }

    /// Constructs a [`ValidatedSliceMut`] stamped with host [`BamColumnInfo`] layout and UUID metadata.
    ///
    /// # Safety
    /// If `ptr` is non-null and `len > 0`, `ptr` must remain valid for exclusive mutable access to
    /// `len` contiguous elements described by `info` for lifetime `'a`.
    #[inline]
    pub const unsafe fn from_raw_parts_with_info(
        ptr: *mut u8,
        len: usize,
        info: BamColumnInfo,
    ) -> Self {
        let ok_mode = info.mode.is_mutable()
            && info.mode.has_entity_column_data()
            && !info.is_immutable()
            && !info.is_resource()
            && !info.is_sparse_set();
        Self {
            ptr,
            len,
            expected_size: Some(info.size),
            expected_align: Some(info.align),
            uuid: Some(info.uuid),
            manifest_name: "",
            valid: ok_mode && (!ptr.is_null() || (len == 0 && !info.mode.is_optional())),
            _marker: PhantomData,
        }
    }

    /// Stamps this slice with the owning mod instance's `manifest_name` for auto-namespaced
    /// `GuestComponent` UUID matching.
    #[inline]
    pub const fn with_manifest_name(mut self, manifest_name: &'a str) -> Self {
        self.manifest_name = manifest_name;
        self
    }

    /// Returns the owning mod instance's manifest name (`""` if unspecified).
    #[inline]
    pub const fn manifest_name(&self) -> &'a str {
        self.manifest_name
    }

    /// Returns the number of elements in this chunk view (`0` if invalid/absent).
    #[inline]
    pub const fn len(&self) -> usize {
        if self.valid { self.len } else { 0 }
    }

    /// Returns `true` if `len() == 0`.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the raw base byte pointer.
    #[inline]
    pub const fn as_ptr(&self) -> *const u8 {
        self.ptr.cast_const()
    }

    /// Returns the raw mutable base byte pointer.
    #[inline]
    pub const fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }

    /// Returns the known element byte stride (`0` if invalid or ZST).
    #[inline]
    pub const fn stride(&self) -> usize {
        match self.expected_size {
            Some(s) => s,
            None => 0,
        }
    }

    /// Returns the column's [`ComponentUuid`] if constructed with [`BamColumnInfo`].
    #[inline]
    pub const fn uuid(&self) -> Option<ComponentUuid> {
        self.uuid
    }

    /// Computes a bounds-checked read-only element pointer at `index`.
    #[inline]
    pub fn get_ptr(&self, index: usize, elem_stride: usize) -> Option<*const u8> {
        if self.ptr.is_null() {
            return None;
        }
        let byte_offset = checked_slice_elem_offset(
            self.valid,
            self.len,
            self.expected_size,
            index,
            elem_stride,
        )?;
        // SAFETY: `index < self.len`, `total <= isize::MAX`, and `self.ptr` covers `total` bytes.
        Some(unsafe { self.ptr.add(byte_offset).cast_const() })
    }

    /// Computes a bounds-checked mutable element pointer at `index`.
    #[inline]
    pub fn get_mut_ptr(&mut self, index: usize, elem_stride: usize) -> Option<*mut u8> {
        if self.ptr.is_null() {
            return None;
        }
        let byte_offset = checked_slice_elem_offset(
            self.valid,
            self.len,
            self.expected_size,
            index,
            elem_stride,
        )?;
        // SAFETY: `index < self.len`, `total <= isize::MAX`, and `self.ptr` covers `total` bytes.
        Some(unsafe { self.ptr.add(byte_offset) })
    }

    /// Borrows this column view as `&[T]` (tied to `&self`) after verifying size, alignment, and
    /// optional `expected_uuid`.
    #[inline]
    pub fn as_typed_slice_checked<T: AbiPod>(
        &self,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&[T]> {
        if !validate_slice_meta::<T>(
            self.valid,
            self.expected_size,
            self.expected_align,
            self.uuid,
            expected_uuid,
        ) {
            return None;
        }
        if self.len == 0 && (!self.ptr.is_null() || core::mem::size_of::<T>() == 0) {
            return Some(&[]);
        }
        // SAFETY: Layout/UUID checks succeeded and `&self` borrows `self.ptr` immutably.
        unsafe { raw_dense_slice::<T>(self.ptr.cast_const(), self.len) }
    }

    /// Borrows this column view as `&[T]` (`T: AbiPod`), tied to `&self`.
    #[inline]
    pub fn as_typed_slice<T: AbiPod>(&self) -> Option<&[T]> {
        self.as_typed_slice_checked::<T>(None)
    }

    /// Borrows this column view as `&mut [T]` (tied to `&mut self`) after verifying size,
    /// alignment, and optional `expected_uuid`.
    #[inline]
    pub fn as_typed_slice_mut_checked<T: AbiPod>(
        &mut self,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&mut [T]> {
        if !validate_slice_meta::<T>(
            self.valid,
            self.expected_size,
            self.expected_align,
            self.uuid,
            expected_uuid,
        ) {
            return None;
        }
        if self.len == 0 && (!self.ptr.is_null() || core::mem::size_of::<T>() == 0) {
            return Some(&mut []);
        }
        // SAFETY: Layout/UUID checks succeeded and `&mut self` guarantees exclusive access.
        unsafe { raw_dense_slice_mut::<T>(self.ptr, self.len) }
    }

    /// Borrows this column view as `&mut [T]` (`T: AbiPod`), tied to `&mut self`.
    #[inline]
    pub fn as_typed_slice_mut<T: AbiPod>(&mut self) -> Option<&mut [T]> {
        self.as_typed_slice_mut_checked::<T>(None)
    }

    /// Consumes this wrapper and returns `&'a mut [T]` with the full chunk lifetime `'a`.
    #[inline]
    pub fn into_typed_slice_mut_checked<T: AbiPod>(
        self,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&'a mut [T]> {
        if !validate_slice_meta::<T>(
            self.valid,
            self.expected_size,
            self.expected_align,
            self.uuid,
            expected_uuid,
        ) {
            return None;
        }
        if self.len == 0 && (!self.ptr.is_null() || core::mem::size_of::<T>() == 0) {
            return Some(&mut []);
        }
        // SAFETY: Consumes `self`, transferring exclusive `'a` access to the returned slice.
        unsafe { raw_dense_slice_mut::<T>(self.ptr, self.len) }
    }
}

/// Safe read-only view over an indirect `[ *const u8; chunk_len ]` sparse-set row pointer array.
pub struct SparseRows<'a, T: AbiPod> {
    ptrs: *const *const u8,
    len: usize,
    _marker: PhantomData<&'a [T]>,
}

// SAFETY: `T: AbiPod` is `Sync`, and `ptrs` points to host-owned read-only row pointers valid for `'a`.
unsafe impl<T: AbiPod> Send for SparseRows<'_, T> {}
// SAFETY: Shared access to `SparseRows` only yields `&T` references where `T: Sync`.
unsafe impl<T: AbiPod> Sync for SparseRows<'_, T> {}

impl<'a, T: AbiPod> SparseRows<'a, T> {
    /// Validates the outer `ptrs` array (`len * size_of::<*const u8>() <= isize::MAX`, non-null,
    /// and aligned when `len > 0`) in `O(1)` and constructs a [`SparseRows`] view.
    ///
    /// Individual inner row pointers are validated lazily in [`Self::get`]: null rows and
    /// misaligned non-ZST row pointers yield `None` without dereferencing.
    ///
    /// # Safety
    /// If `len > 0`, `ptrs` must be non-null, aligned to `*const u8`, and readable for `len`
    /// pointers for lifetime `'a`, where every non-null, properly aligned `ptrs[i]` points to an
    /// initialized `T` valid for shared reads during `'a`.
    pub unsafe fn from_raw_parts(
        ptrs: *const *const u8,
        len: usize,
        _allow_null_rows: bool,
    ) -> Option<Self> {
        if len == 0 {
            return Some(Self {
                ptrs: core::ptr::NonNull::<*const u8>::dangling().as_ptr(),
                len: 0,
                _marker: PhantomData,
            });
        }
        if ptrs.is_null()
            || !(ptrs as usize).is_multiple_of(core::mem::align_of::<*const u8>())
            || len
                .checked_mul(core::mem::size_of::<*const u8>())
                .is_none_or(|bytes| bytes > isize::MAX as usize)
        {
            return None;
        }
        Some(Self {
            ptrs,
            len,
            _marker: PhantomData,
        })
    }

    /// Number of entity rows in the current chunk.
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if `self.len() == 0`.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns `&T` for `row`, or `None` if `row >= self.len()`, the row pointer is null, or
    /// (for non-ZST `T`) the row pointer is misaligned for `T`.
    #[inline]
    pub fn get(&self, row: usize) -> Option<&T> {
        if row >= self.len {
            return None;
        }
        // SAFETY: `row < self.len`, and `from_raw_parts` verified `self.ptrs` is readable for `self.len`.
        let row_ptr = unsafe { *self.ptrs.add(row) };
        if row_ptr.is_null() {
            return None;
        }
        if core::mem::size_of::<T>() == 0 {
            // SAFETY: Zero-sized `T: AbiPod` may be referenced via `NonNull::dangling()`.
            return Some(unsafe { core::ptr::NonNull::<T>::dangling().as_ref() });
        }
        if !(row_ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
            return None;
        }
        // SAFETY: Verified `row_ptr` is non-null and aligned to `T`; `from_raw_parts` caller
        // contract guarantees every non-null aligned entry points to an initialized `T` valid for `'a`.
        Some(unsafe { &*row_ptr.cast::<T>() })
    }

    /// Iterates over all present `&T` row values in entity order.
    #[inline]
    pub fn iter(&self) -> SparseRowsIter<'_, 'a, T> {
        SparseRowsIter {
            rows: self,
            index: 0,
        }
    }
}

impl<T: AbiPod> Index<usize> for SparseRows<'_, T> {
    type Output = T;

    /// Returns `&T` at `index`, panicking if out of bounds or if an optional sparse row is absent.
    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        self.get(index)
            .expect("sparse row index out of bounds or absent optional component")
    }
}

/// Iterator over present `&T` items in [`SparseRows`].
pub struct SparseRowsIter<'b, 'a, T: AbiPod> {
    rows: &'b SparseRows<'a, T>,
    index: usize,
}

impl<'b, T: AbiPod> Iterator for SparseRowsIter<'b, '_, T> {
    type Item = &'b T;

    /// Advances past absent optional rows and yields the next present `&T` element.
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.rows.len {
            let idx = self.index;
            self.index += 1;
            if let Some(val) = self.rows.get(idx) {
                return Some(val);
            }
        }
        None
    }

    /// Returns `(0, Some(remaining))` because optional sparse rows may be absent.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.rows.len.saturating_sub(self.index)))
    }
}

impl<'b, 'a, T: AbiPod> IntoIterator for &'b SparseRows<'a, T> {
    type Item = &'b T;
    type IntoIter = SparseRowsIter<'b, 'a, T>;

    /// Delegates to [`SparseRows::iter`].
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Safe mutable view over an indirect `[ *mut u8; chunk_len ]` sparse-set row pointer array.
///
/// Mutable row access (`get_mut` and `for_each_mut`) lends at most one `&mut T` at a time tied to
/// `&mut self`, preventing aliasing mutable borrows even if a faulty host returned duplicate row
/// pointers.
pub struct SparseRowsMut<'a, T: AbiPod> {
    ptrs: *const *mut u8,
    len: usize,
    _marker: PhantomData<&'a mut [T]>,
}

// SAFETY: `T: AbiPod` is `Send + Sync`, and `SparseRowsMut` holds exclusive access to the column's
// row pointers for `'a`.
unsafe impl<T: AbiPod> Send for SparseRowsMut<'_, T> {}
// SAFETY: Shared references (`&SparseRowsMut`) only yield `&T` (`T: Sync`).
unsafe impl<T: AbiPod> Sync for SparseRowsMut<'_, T> {}

impl<'a, T: AbiPod> SparseRowsMut<'a, T> {
    /// Validates the outer `ptrs` array (`len * size_of::<*mut u8>() <= isize::MAX`, non-null,
    /// and aligned when `len > 0`) in `O(1)` and constructs a [`SparseRowsMut`] view.
    ///
    /// Individual inner row pointers are validated lazily in [`Self::get`] and [`Self::get_mut`]:
    /// null rows and misaligned non-ZST row pointers yield `None` without dereferencing.
    ///
    /// # Safety
    /// If `len > 0`, `ptrs` must be non-null, aligned to `*mut u8`, and readable for `len` pointers
    /// for lifetime `'a`, where every non-null, properly aligned `ptrs[i]` points to an initialized
    /// `T` valid for exclusive mutable access during `'a`.
    pub unsafe fn from_raw_parts(
        ptrs: *const *mut u8,
        len: usize,
        _allow_null_rows: bool,
    ) -> Option<Self> {
        if len == 0 {
            return Some(Self {
                ptrs: core::ptr::NonNull::<*mut u8>::dangling().as_ptr(),
                len: 0,
                _marker: PhantomData,
            });
        }
        if ptrs.is_null()
            || !(ptrs as usize).is_multiple_of(core::mem::align_of::<*mut u8>())
            || len
                .checked_mul(core::mem::size_of::<*mut u8>())
                .is_none_or(|bytes| bytes > isize::MAX as usize)
        {
            return None;
        }
        Some(Self {
            ptrs,
            len,
            _marker: PhantomData,
        })
    }

    /// Number of entity rows in the current chunk.
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if `self.len() == 0`.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Borrows row `row` immutably (`None` if out of bounds, null, or misaligned for non-ZST `T`).
    #[inline]
    pub fn get(&self, row: usize) -> Option<&T> {
        if row >= self.len {
            return None;
        }
        // SAFETY: `row < self.len`, and `from_raw_parts` verified `self.ptrs` is readable.
        let row_ptr = unsafe { *self.ptrs.add(row) };
        if row_ptr.is_null() {
            return None;
        }
        if core::mem::size_of::<T>() == 0 {
            // SAFETY: Zero-sized `T: AbiPod` may be referenced via `NonNull::dangling()`.
            return Some(unsafe { core::ptr::NonNull::<T>::dangling().as_ref() });
        }
        if !(row_ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
            return None;
        }
        // SAFETY: Verified `row_ptr` is non-null and aligned to `T`; caller contract guarantees
        // initialization, and `&self` prevents concurrent mutation.
        Some(unsafe { &*row_ptr.cast_const().cast::<T>() })
    }

    /// Borrows row `row` mutably (`None` if out of bounds, null, or misaligned for non-ZST `T`).
    #[inline]
    pub fn get_mut(&mut self, row: usize) -> Option<&mut T> {
        if row >= self.len {
            return None;
        }
        // SAFETY: `row < self.len`, and `from_raw_parts` verified `self.ptrs` is readable.
        let row_ptr = unsafe { *self.ptrs.add(row) };
        if row_ptr.is_null() {
            return None;
        }
        if core::mem::size_of::<T>() == 0 {
            // SAFETY: Zero-sized `T: AbiPod` may be mutably referenced via `NonNull::dangling()`.
            return Some(unsafe { core::ptr::NonNull::<T>::dangling().as_mut() });
        }
        if !(row_ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
            return None;
        }
        // SAFETY: Verified `row_ptr` is non-null and aligned to `T`; caller contract guarantees
        // initialization, and tying the borrow to `&mut self` guarantees at most one `&mut T`
        // exists at any instant.
        Some(unsafe { &mut *row_ptr.cast::<T>() })
    }

    /// Invokes `f(row_index, &mut T)` sequentially for every present row in `0..self.len()`.
    #[inline]
    pub fn for_each_mut(&mut self, mut f: impl FnMut(usize, &mut T)) {
        let len = self.len;
        for row in 0..len {
            if let Some(item) = self.get_mut(row) {
                f(row, item);
            }
        }
    }
}

/// Number of leading query term indices whose [`BamColumnInfo`] and validated term metadata are
/// cached inline inside [`QuerySlotScratch`].
///
/// Covers 0..8 terms (matching [`INLINE_BATCH_COL_CAP`]) without inflating per-tick stack frames,
/// eliminating steady-state `BamHostApi::iter_column_info` FFI calls and redundant per-chunk /
/// per-resource layout + UUID validation across ticks.
const INLINE_COL_INFO_CAP: usize = 8;

/// Maximum number of column pointers cached inline per chunk inside [`ValidatedIter`] when
/// advancing via `BamHostApi::iter_next_chunk_batch` (slot 68).
const INLINE_BATCH_COL_CAP: usize = 8;

const _: () = {
    assert!(INLINE_COL_INFO_CAP <= 8);
    assert!(INLINE_BATCH_COL_CAP <= 32);
};

static NEXT_ITER_ID: AtomicU64 = AtomicU64::new(1);

#[inline]
fn alloc_iter_id() -> u64 {
    let id = NEXT_ITER_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        NEXT_ITER_ID.fetch_add(1, Ordering::Relaxed)
    } else {
        id
    }
}

struct BatchColPtrBuf([MaybeUninit<*mut u8>; INLINE_BATCH_COL_CAP]);

// SAFETY: Pointers written into `BatchColPtrBuf` during `advance_batch` are only read during the
// same active tick while `SystemClosureState::closure.try_with_mut` holds exclusive access, and a
// fresh `ValidatedIter` with `scratch_batch_epoch = 0` (`!= batch_epoch`) is created on each tick.
unsafe impl Send for BatchColPtrBuf {}

/// Classification of a cached term validation entry inside [`QuerySlotScratch`].
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum CachedTermKind {
    DenseRead = 0,
    DenseReadAny = 1,
    DenseWrite = 2,
    DenseWriteAny = 3,
    OptionalRead = 4,
    OptionalWrite = 5,
    Filter = 6,
    FilterUntyped = 7,
    Has = 8,
    HasUntyped = 9,
    ResourceRead = 10,
    ResourceWrite = 11,
}

/// Cached metadata produced after validating a term descriptor `(term_index, kind, TypeId,
/// expected_mode, expected_uuid)` against host [`BamColumnInfo`].
#[derive(Clone, Copy)]
struct CachedTermEntry {
    type_id: core::any::TypeId,
    expected_uuid: Option<UuidFilter>,
    expected_mode: AccessMode,
    info_mode: AccessMode,
    component: BamComponent,
    kind: CachedTermKind,
}

impl CachedTermEntry {
    #[inline(always)]
    fn matches<T: 'static>(
        &self,
        kind: CachedTermKind,
        expected_mode: AccessMode,
        expected_uuid: Option<UuidFilter>,
    ) -> bool {
        self.kind == kind
            && self.expected_mode == expected_mode
            && self.type_id == core::any::TypeId::of::<T>()
            && self.expected_uuid == expected_uuid
    }
}

/// Read-only `Sync` snapshot of initialized `BamColumnInfo` and [`CachedTermEntry`] slots from
/// [`QuerySlotScratch`], shared across parallel worker threads in `par_for_each_chunk*`.
#[derive(Clone, Copy)]
pub(crate) struct FrozenSlotCache {
    col_info_mask: u8,
    term_cache_mask: u8,
    col_infos: [MaybeUninit<BamColumnInfo>; INLINE_COL_INFO_CAP],
    term_entries: [MaybeUninit<CachedTermEntry>; INLINE_COL_INFO_CAP],
}

/// 8-byte tagged pointer referencing either a per-slot mutable [`QuerySlotScratch`] (low bit `0`)
/// or a frozen read-only [`FrozenSlotCache`] snapshot (low bit `1`).
#[derive(Clone, Copy)]
struct SlotCacheRef<'a> {
    tagged_ptr: core::ptr::NonNull<u8>,
    _marker: PhantomData<&'a ()>,
}

impl<'a> SlotCacheRef<'a> {
    #[inline(always)]
    fn mutable(scratch: &'a QuerySlotScratch) -> Self {
        let ptr = core::ptr::from_ref(scratch).cast::<u8>().cast_mut();
        // SAFETY: `scratch` is a live `&'a QuerySlotScratch` (non-null, aligned >= 2), so bit 0 is `0`.
        let tagged_ptr = unsafe { core::ptr::NonNull::new_unchecked(ptr) };
        Self {
            tagged_ptr,
            _marker: PhantomData,
        }
    }

    #[inline(always)]
    fn frozen(cache: &'a FrozenSlotCache) -> Self {
        let ptr = core::ptr::from_ref(cache)
            .cast::<u8>()
            .cast_mut()
            .map_addr(|addr| addr | 1);
        // SAFETY: `cache` is a live `&'a FrozenSlotCache` (non-null, aligned >= 2), and `map_addr`
        // preserves strict provenance while setting tag bit `0` to `1`.
        let tagged_ptr = unsafe { core::ptr::NonNull::new_unchecked(ptr) };
        Self {
            tagged_ptr,
            _marker: PhantomData,
        }
    }

    #[inline(always)]
    #[allow(clippy::cast_ptr_alignment)]
    fn as_mutable(self) -> Option<&'a QuerySlotScratch> {
        let ptr = self.tagged_ptr.as_ptr();
        if (ptr.addr() & 1) == 0 {
            // SAFETY: Tag bit `0 == 0` is only produced by `Self::mutable` from `&'a QuerySlotScratch`,
            // which already satisfies `align_of::<QuerySlotScratch>()`.
            Some(unsafe { &*ptr.cast_const().cast::<QuerySlotScratch>() })
        } else {
            None
        }
    }

    #[inline(always)]
    #[allow(clippy::cast_ptr_alignment)]
    fn as_frozen(self) -> Option<&'a FrozenSlotCache> {
        let ptr = self.tagged_ptr.as_ptr();
        if (ptr.addr() & 1) == 1 {
            let unmasked = ptr
                .map_addr(|addr| addr & !1)
                .cast_const()
                .cast::<FrozenSlotCache>();
            // SAFETY: Tag bit `0 == 1` is only produced by `Self::frozen` from `&'a FrozenSlotCache`,
            // and clearing bit `0` via `map_addr` restores the exact aligned pointer and provenance.
            Some(unsafe { &*unmasked })
        } else {
            None
        }
    }
}

/// Per-query-slot scratch storage for persistent `BamColumnInfo`/term metadata caching and
/// per-tick slot-68 batch column pointers.
///
/// Kept inside `SystemClosureState::slot_scratches` (rather than inline inside [`ValidatedIter`])
/// so [`ValidatedIter`] stays `<= 64` bytes on stack frames.
///
/// # Cache Lifetime Invariant
/// Under the wire-level [`bamabi_sys::BamIterColumnInfoFn`] invariance contract and
/// [`bamabi_sys::BamMigrateComponentColumnFn`]'s cross-mod `FfiResult::ACCESS_CONFLICT`
/// exclusivity rule, a registered system's `(query_index, term_index)` [`BamColumnInfo`] is
/// immutable across all chunks and ticks for the lifetime of that registered system (and a
/// hot-reload of the owning mod tears down the old `GuestInstance` and allocates a fresh
/// `SlotScratchesCell`). Therefore:
/// - Internal `SystemClosureState` invocations call `Self::reset_per_tick` on tick entry,
///   retaining `col_info_mask` and `term_cache_mask` across ticks.
/// - External callers reusing a [`QuerySlotScratch`] via [`ValidatedIter::from_raw_with_scratch`]
///   have no single-system binding and MUST clear all caches via `Self::reset_all`.
pub struct QuerySlotScratch {
    col_info_mask: Cell<u8>,
    term_cache_mask: Cell<u8>,
    batch_col_count: Cell<u8>,
    col_infos: UnsafeCell<[MaybeUninit<BamColumnInfo>; INLINE_COL_INFO_CAP]>,
    term_entries: UnsafeCell<[MaybeUninit<CachedTermEntry>; INLINE_COL_INFO_CAP]>,
    batch_col_ptrs: UnsafeCell<BatchColPtrBuf>,
}

impl QuerySlotScratch {
    /// Creates an empty, uninitialized per-query-slot scratch buffer without zeroing the
    /// `BamColumnInfo`, term entry, or pointer arrays.
    #[inline]
    pub const fn new() -> Self {
        Self {
            col_info_mask: Cell::new(0),
            term_cache_mask: Cell::new(0),
            batch_col_count: Cell::new(0),
            col_infos: UnsafeCell::new([const { MaybeUninit::uninit() }; INLINE_COL_INFO_CAP]),
            term_entries: UnsafeCell::new([const { MaybeUninit::uninit() }; INLINE_COL_INFO_CAP]),
            batch_col_ptrs: UnsafeCell::new(BatchColPtrBuf(
                [const { MaybeUninit::uninit() }; INLINE_BATCH_COL_CAP],
            )),
        }
    }

    /// Resets per-tick ephemeral slot state (`batch_col_count`) on system tick entry while
    /// preserving `col_info_mask` (`col_infos`) and `term_cache_mask` (`term_entries`) across ticks.
    ///
    /// # Safety Contract Note
    /// Sound solely because `SystemClosureState` binds each [`QuerySlotScratch`] 1:1 to a single
    /// registered system whose [`BamColumnInfo`] is invariant for its lifetime per
    /// [`bamabi_sys::BamIterColumnInfoFn`] and [`bamabi_sys::BamMigrateComponentColumnFn`]. Public
    /// scratch reuse via [`ValidatedIter::from_raw_with_scratch`] must call [`Self::reset_all`].
    #[inline]
    pub(crate) fn reset_per_tick(&self) {
        self.batch_col_count.set(0);
    }

    /// Resets all cached slot state (`col_info_mask`, `term_cache_mask`, and `batch_col_count`)
    /// when binding a fresh external `BamIter` via [`ValidatedIter::from_raw_with_scratch`],
    /// preventing stale `BamColumnInfo` or term validation caches from leaking across distinct
    /// iterators or schema revisions.
    #[inline]
    pub(crate) fn reset_all(&self) {
        self.col_info_mask.set(0);
        self.term_cache_mask.set(0);
        self.batch_col_count.set(0);
    }

    /// Snapshots initialized `BamColumnInfo` and [`CachedTermEntry`] slots into a `Sync`
    /// [`FrozenSlotCache`] for lock-free worker reads in `par_for_each_chunk*`.
    fn snapshot_frozen(&self) -> Option<FrozenSlotCache> {
        let col_mask = self.col_info_mask.get();
        let term_mask = self.term_cache_mask.get();
        if col_mask == 0 && term_mask == 0 {
            return None;
        }
        let mut col_infos = [const { MaybeUninit::uninit() }; INLINE_COL_INFO_CAP];
        let mut term_entries = [const { MaybeUninit::uninit() }; INLINE_COL_INFO_CAP];
        let mut idx = 0usize;
        while idx < INLINE_COL_INFO_CAP {
            let bit = 1u8 << idx;
            if (col_mask & bit) != 0 {
                // SAFETY: `col_mask & bit != 0` proves `col_infos[idx]` was initialized; reading
                // via `UnsafeCell::get()` avoids aliasing references under Miri.
                let info = unsafe { self.col_infos.get().cast::<BamColumnInfo>().add(idx).read() };
                col_infos[idx].write(info);
            }
            if (term_mask & bit) != 0 {
                // SAFETY: `term_mask & bit != 0` proves `term_entries[idx]` was initialized.
                let entry = unsafe {
                    self.term_entries
                        .get()
                        .cast::<CachedTermEntry>()
                        .add(idx)
                        .read()
                };
                term_entries[idx].write(entry);
            }
            idx += 1;
        }
        Some(FrozenSlotCache {
            col_info_mask: col_mask,
            term_cache_mask: term_mask,
            col_infos,
            term_entries,
        })
    }
}

impl Default for QuerySlotScratch {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// Safe stack-friendly buffer for slot-68 (`iter_next_chunk_batch`) column pointers, bound to
/// the active tick lifetime `'ctx` and tagged with `(iter_id, batch_epoch)` so stale or
/// cross-iterator caches never expose dangling pointers in safe code.
pub struct BatchColCache<'ctx> {
    ptrs: [MaybeUninit<*mut u8>; INLINE_BATCH_COL_CAP],
    len: u8,
    iter_id: u64,
    batch_epoch: u64,
    _marker: PhantomData<&'ctx ()>,
}

impl<'ctx> BatchColCache<'ctx> {
    /// Creates an empty batch column pointer cache without zero-initializing `ptrs`.
    #[inline]
    pub const fn new() -> Self {
        Self {
            ptrs: [const { MaybeUninit::uninit() }; INLINE_BATCH_COL_CAP],
            len: 0,
            iter_id: 0,
            batch_epoch: 0,
            _marker: PhantomData,
        }
    }
}

impl Default for BatchColCache<'_> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// Validated query chunk and singleton resource accessor wrapping `*mut BamIter`.
pub struct ValidatedIter<'a> {
    raw: *mut BamIter,
    host: &'a Host,
    lazy_host: Option<&'a LazyInvocationHost<'a>>,
    current_len: usize,
    batch_entities: *const FfiEntity,
    slot_cache: Option<SlotCacheRef<'a>>,
    iter_id: u64,
    batch_epoch: u32,
    scratch_batch_epoch: u32,
}

const _: () = {
    assert!(core::mem::size_of::<ValidatedIter<'static>>() <= 64);
    assert!(core::mem::size_of::<Option<ValidatedIter<'static>>>() <= 64);
    assert!(!core::mem::needs_drop::<ValidatedIter<'static>>());
    assert!(!core::mem::needs_drop::<QuerySlotScratch>());
    assert!(!core::mem::needs_drop::<FrozenSlotCache>());
    assert!(core::mem::align_of::<QuerySlotScratch>() >= 2);
    assert!(core::mem::align_of::<FrozenSlotCache>() >= 2);
};

impl<'a> ValidatedIter<'a> {
    /// Advances `batch_epoch` monotonically on any chunk transition or reset (`0` marks
    /// uncacheable overflow saturation).
    #[inline]
    fn bump_batch_epoch(&mut self) {
        if self.batch_epoch != 0 {
            self.batch_epoch = self.batch_epoch.checked_add(1).unwrap_or(0);
        }
    }

    /// Wraps a raw [`BamIter`] pointer obtained from `BamHostApi::sys_query`.
    ///
    /// A null `raw` pointer constructs an exhausted iterator yielding zero chunks.
    ///
    /// # Safety
    /// If `raw` is non-null, it must point to an active [`BamIter`] belonging to the current
    /// system invocation on `host` and must not outlive `'a`.
    #[inline]
    pub unsafe fn from_raw(raw: *mut BamIter, host: &'a Host) -> Self {
        Self {
            raw,
            host,
            lazy_host: None,
            current_len: 0,
            batch_entities: core::ptr::null(),
            slot_cache: None,
            iter_id: alloc_iter_id(),
            batch_epoch: 1,
            scratch_batch_epoch: 0,
        }
    }

    /// Wraps a raw [`BamIter`] pointer bound exclusively to caller-owned [`QuerySlotScratch`] for
    /// `BamColumnInfo` caching and slot-68 batch column pointers.
    ///
    /// Unlike `SystemClosureState::slot_scratches` (which uses `QuerySlotScratch::reset_per_tick`
    /// under [`bamabi_sys::BamIterColumnInfoFn`]'s single-system invariance contract), this
    /// constructor always invokes `QuerySlotScratch::reset_all` so reusing a caller-owned
    /// `&mut QuerySlotScratch` across distinct iterators or migrated schemas cannot leak stale
    /// `BamColumnInfo` or term caches.
    ///
    /// # Safety
    /// If `raw` is non-null, it must point to an active [`BamIter`] belonging to the current
    /// system invocation on `host` and must not outlive `'a`.
    #[inline]
    pub unsafe fn from_raw_with_scratch(
        raw: *mut BamIter,
        host: &'a Host,
        scratch: &'a mut QuerySlotScratch,
    ) -> Self {
        scratch.reset_all();
        Self {
            raw,
            host,
            lazy_host: None,
            current_len: 0,
            batch_entities: core::ptr::null(),
            slot_cache: Some(SlotCacheRef::mutable(&*scratch)),
            iter_id: alloc_iter_id(),
            batch_epoch: 1,
            scratch_batch_epoch: 0,
        }
    }

    /// Wraps a raw [`BamIter`] pointer paired with an optional lazy per-tick host resolver and
    /// optional per-slot scratch storage.
    ///
    /// # Safety
    /// - If `raw` is non-null, it must point to an active [`BamIter`] belonging to the current
    ///   system invocation on `host` and must not outlive `'a`.
    /// - If `slot_scratch` is `Some`, it must not be shared with any other active [`ValidatedIter`]
    ///   for the duration of `'a`.
    #[inline(always)]
    pub(crate) unsafe fn from_raw_with_parts(
        raw: *mut BamIter,
        host: &'a Host,
        lazy_host: Option<&'a LazyInvocationHost<'a>>,
        slot_scratch: Option<&'a QuerySlotScratch>,
    ) -> Self {
        Self {
            raw,
            host,
            lazy_host,
            current_len: 0,
            batch_entities: core::ptr::null(),
            slot_cache: slot_scratch.map(SlotCacheRef::mutable),
            iter_id: alloc_iter_id(),
            batch_epoch: 1,
            scratch_batch_epoch: 0,
        }
    }

    /// Wraps a sub-iterator [`BamIter`] pointer for a `par_for_each_chunk*` worker task with an
    /// optional frozen read-only metadata cache snapshot.
    ///
    /// # Safety
    /// - `raw` must point to an active worker sub-range [`BamIter`] on `host` valid for `'a`.
    /// - If `frozen_cache` is `Some`, it must outlive `'a`.
    #[inline]
    pub(crate) unsafe fn from_raw_with_frozen_cache(
        raw: *mut BamIter,
        host: &'a Host,
        frozen_cache: Option<&'a FrozenSlotCache>,
    ) -> Self {
        Self {
            raw,
            host,
            lazy_host: None,
            current_len: 0,
            batch_entities: core::ptr::null(),
            slot_cache: frozen_cache.map(SlotCacheRef::frozen),
            iter_id: alloc_iter_id(),
            batch_epoch: 1,
            scratch_batch_epoch: 0,
        }
    }

    /// Constructs a disabled/exhausted iterator (`raw = null`) yielding zero chunks.
    #[inline]
    pub const fn empty(host: &'a Host) -> Self {
        Self {
            raw: core::ptr::null_mut(),
            host,
            lazy_host: None,
            current_len: 0,
            batch_entities: core::ptr::null(),
            slot_cache: None,
            iter_id: 0,
            batch_epoch: 1,
            scratch_batch_epoch: 0,
        }
    }

    /// Returns `true` if the underlying [`BamIter`] pointer is non-null.
    #[inline]
    pub fn is_valid(&self) -> bool {
        !self.raw.is_null()
    }

    /// Returns the raw `*mut BamIter` handle.
    #[inline]
    pub const fn as_raw(&self) -> *mut BamIter {
        self.raw
    }

    /// Returns the init-time base [`Host`] table without invoking `BamHostApi::sys_host_ctx`.
    ///
    /// Safe for immutable per-instance metadata (`api()`, `manifest_name()`) on hot paths.
    #[inline]
    pub const fn base_host(&self) -> &'a Host {
        self.host
    }

    /// Returns the active [`Host`] table for this invocation, lazily resolving
    /// `BamHostApi::sys_host_ctx` when backed by a trampoline frame.
    #[inline]
    pub fn host(&self) -> &'a Host {
        match self.lazy_host {
            Some(lazy) => lazy.resolve(),
            None => self.host,
        }
    }

    /// Returns the number of entity rows in the currently active chunk (`0` before `next_chunk` or
    /// after exhaustion).
    #[inline]
    pub const fn current_len(&self) -> usize {
        self.current_len
    }

    /// Advances to the next non-empty chunk, returning `Some(chunk_len)` while chunks remain or
    /// `None` when exhausted.
    #[inline]
    pub fn next_chunk(&mut self) -> Option<usize> {
        self.bump_batch_epoch();
        self.batch_entities = core::ptr::null();
        if self.raw.is_null() {
            self.current_len = 0;
            return None;
        }
        let Some(next_fn) = self.host.api().iter_next_chunk else {
            self.current_len = 0;
            return None;
        };
        let mut len: usize = 0;
        // SAFETY: `self.raw` is a valid active `BamIter` for `'a` and `len` is writable stack storage.
        let has_more = unsafe { next_fn(self.raw, &mut len) };
        if has_more.get() {
            self.current_len = len;
            Some(len)
        } else {
            self.current_len = 0;
            None
        }
    }

    /// Advances to the next non-empty chunk via `BamHostApi::iter_next_chunk_batch` (slot 68) when
    /// available, `slot_scratch` is bound, and `col_count <= INLINE_BATCH_COL_CAP`, populating the
    /// slot scratch batch pointer cache for subsequent [`ChunkColumnDispenser`] reads; otherwise
    /// falls back to [`Self::next_chunk`].
    #[inline]
    pub fn advance_batch(
        &mut self,
        col_count: usize,
        needs_entities: bool,
        mark_changed_mask: u32,
    ) -> Option<usize> {
        self.bump_batch_epoch();
        self.batch_entities = core::ptr::null();
        if col_count <= INLINE_BATCH_COL_CAP {
            if let Some(scratch) = self.slot_cache.and_then(SlotCacheRef::as_mutable) {
                if let Some(batch_fn) = host_slot!(self.host.api(), iter_next_chunk_batch) {
                    if self.raw.is_null() {
                        self.current_len = 0;
                        return None;
                    }
                    // SAFETY: `self.slot_scratch` belongs exclusively to this `ValidatedIter<'a>`
                    // (derived from `&'a mut QuerySlotScratch` or exclusive `query_taken_mask` slot),
                    // and `&mut self` grants exclusive access for this call.
                    let ptrs = unsafe { &mut (*scratch.batch_col_ptrs.get()).0 };
                    for slot in &mut ptrs[..col_count] {
                        slot.write(core::ptr::null_mut());
                    }
                    let mut len: usize = 0;
                    let mut entities_ptr: *const FfiEntity = core::ptr::null();
                    let ent_out = if needs_entities {
                        core::ptr::from_mut(&mut entities_ptr)
                    } else {
                        core::ptr::null_mut()
                    };
                    let col_out = if col_count > 0 {
                        ptrs.as_mut_ptr().cast::<*mut u8>()
                    } else {
                        core::ptr::null_mut()
                    };
                    // SAFETY: `self.raw` is a valid active `BamIter` for `'a`, `len` is writable stack
                    // storage, `ent_out` is either null or points to `entities_ptr`, and `col_out`
                    // points to `col_count` initialized writable `*mut u8` slots when `col_count > 0`.
                    let has_more = unsafe {
                        batch_fn(
                            self.raw,
                            &mut len,
                            ent_out,
                            col_out,
                            col_count,
                            mark_changed_mask,
                        )
                    };
                    if has_more.get() {
                        self.current_len = len;
                        scratch.batch_col_count.set(col_count as u8);
                        self.scratch_batch_epoch = self.batch_epoch;
                        self.batch_entities = if needs_entities {
                            entities_ptr
                        } else {
                            core::ptr::null()
                        };
                        return Some(len);
                    }
                    self.current_len = 0;
                    return None;
                }
            }
        }
        self.next_chunk()
    }

    /// Advances to the next non-empty chunk via `BamHostApi::iter_next_chunk_batch` (slot 68)
    /// into caller-provided `cache`, falling back to [`Self::next_chunk`] when slot 68 is
    /// unavailable or `col_count > INLINE_BATCH_COL_CAP`.
    #[inline]
    pub fn advance_batch_into(
        &mut self,
        cache: &mut BatchColCache<'a>,
        col_count: usize,
        needs_entities: bool,
        mark_changed_mask: u32,
    ) -> Option<usize> {
        self.bump_batch_epoch();
        self.batch_entities = core::ptr::null();
        cache.len = 0;
        cache.iter_id = 0;
        cache.batch_epoch = 0;
        if col_count <= INLINE_BATCH_COL_CAP {
            if let Some(batch_fn) = host_slot!(self.host.api(), iter_next_chunk_batch) {
                if self.raw.is_null() {
                    self.current_len = 0;
                    return None;
                }
                for slot in &mut cache.ptrs[..col_count] {
                    slot.write(core::ptr::null_mut());
                }
                let mut len: usize = 0;
                let mut entities_ptr: *const FfiEntity = core::ptr::null();
                let ent_out = if needs_entities {
                    core::ptr::from_mut(&mut entities_ptr)
                } else {
                    core::ptr::null_mut()
                };
                let col_out = if col_count > 0 {
                    cache.ptrs.as_mut_ptr().cast::<*mut u8>()
                } else {
                    core::ptr::null_mut()
                };
                // SAFETY: `self.raw` is a valid active `BamIter` for `'a`, `len` is writable stack
                // storage, `ent_out` is either null or points to `entities_ptr`, and `col_out`
                // points to `col_count` initialized writable `*mut u8` slots when `col_count > 0`.
                let has_more = unsafe {
                    batch_fn(
                        self.raw,
                        &mut len,
                        ent_out,
                        col_out,
                        col_count,
                        mark_changed_mask,
                    )
                };
                if has_more.get() {
                    self.current_len = len;
                    self.batch_entities = if needs_entities {
                        entities_ptr
                    } else {
                        core::ptr::null()
                    };
                    if self.iter_id != 0 && self.batch_epoch != 0 {
                        cache.len = col_count as u8;
                        cache.iter_id = self.iter_id;
                        cache.batch_epoch = u64::from(self.batch_epoch);
                    }
                    return Some(len);
                }
                self.current_len = 0;
                return None;
            }
        }
        self.next_chunk()
    }

    /// Advances to the next non-empty chunk and populates `out_entities` (when `Some`) and
    /// `out_col_ptrs` in a single FFI call via `BamHostApi::iter_next_chunk_batch` (slot 68).
    ///
    /// Returns:
    /// - `Ok(Some(len))` when a chunk was fetched via slot 68.
    /// - `Ok(None)` when the iterator is exhausted or null.
    /// - `Err(FfiResult::UNSUPPORTED)` when the host does not cover or provide slot 68.
    #[inline]
    pub fn next_chunk_batch(
        &mut self,
        mut out_entities: Option<&mut *const FfiEntity>,
        out_col_ptrs: &mut [*mut u8],
        mark_changed_mask: u32,
    ) -> Result<Option<usize>, FfiResult> {
        self.bump_batch_epoch();
        self.batch_entities = core::ptr::null();
        let batch_fn =
            host_slot!(self.host.api(), iter_next_chunk_batch).ok_or(FfiResult::UNSUPPORTED)?;
        if self.raw.is_null() {
            self.current_len = 0;
            if let Some(ent) = out_entities.as_deref_mut() {
                *ent = core::ptr::null();
            }
            out_col_ptrs.fill(core::ptr::null_mut());
            return Ok(None);
        }
        let mut len: usize = 0;
        let ent_ptr = out_entities.map_or(core::ptr::null_mut(), core::ptr::from_mut);
        let col_ptr = if out_col_ptrs.is_empty() {
            core::ptr::null_mut()
        } else {
            out_col_ptrs.as_mut_ptr()
        };
        // SAFETY: `self.raw` is a valid active `BamIter` for `'a`, `len` is writable stack storage,
        // `ent_ptr` is either null or writable `*const FfiEntity` storage, and `col_ptr` points to
        // `out_col_ptrs.len()` writable `*mut u8` slots when non-empty.
        let has_more = unsafe {
            batch_fn(
                self.raw,
                &mut len,
                ent_ptr,
                col_ptr,
                out_col_ptrs.len(),
                mark_changed_mask,
            )
        };
        if has_more.get() {
            self.current_len = len;
            Ok(Some(len))
        } else {
            self.current_len = 0;
            Ok(None)
        }
    }

    /// Resets the iterator back to its initial chunk so nested query loops can re-traverse.
    #[inline]
    pub fn reset(&mut self) -> FfiResult {
        self.bump_batch_epoch();
        self.current_len = 0;
        self.batch_entities = core::ptr::null();
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let Some(reset_fn) = self.host.api().iter_reset else {
            return FfiResult::NULL_POINTER;
        };
        // SAFETY: `self.raw` is a valid active `BamIter` for `'a`.
        unsafe { reset_fn(self.raw) }
    }

    /// Queries layout, UUID, access mode, and storage flags for `term_index`.
    ///
    /// Caches successful results for `term_index < INLINE_COL_INFO_CAP` inside `slot_cache` when
    /// bound mutably, and reads cached results lock-free from frozen snapshots during parallel
    /// chunk execution.
    #[inline]
    pub fn column_info(&self, term_index: usize) -> Result<BamColumnInfo, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        if term_index < INLINE_COL_INFO_CAP {
            if let Some(cache) = self.slot_cache {
                let bit = 1u8 << term_index;
                if let Some(scratch) = cache.as_mutable() {
                    if (scratch.col_info_mask.get() & bit) != 0 {
                        // SAFETY: `col_info_mask & bit != 0` proves slot `term_index` in `scratch`
                        // was initialized by `column_info_uncached`, `term_index < INLINE_COL_INFO_CAP`,
                        // and `BamColumnInfo` is `Copy`. Reading via raw pointer through
                        // `UnsafeCell::get()` avoids forming aliasing references under Miri.
                        let info = unsafe {
                            scratch
                                .col_infos
                                .get()
                                .cast::<BamColumnInfo>()
                                .add(term_index)
                                .read()
                        };
                        return Ok(info);
                    }
                } else if let Some(frozen) = cache.as_frozen() {
                    if (frozen.col_info_mask & bit) != 0 {
                        // SAFETY: `col_info_mask & bit != 0` proves `frozen.col_infos[term_index]`
                        // was initialized by `snapshot_frozen`.
                        return Ok(unsafe { frozen.col_infos[term_index].assume_init_read() });
                    }
                }
            }
        }
        self.column_info_uncached(term_index)
    }

    /// Cold FFI path invoking `BamHostApi::iter_column_info` and populating `slot_cache`'s
    /// inline cache (when mutable) for `term_index < INLINE_COL_INFO_CAP` on [`FfiResult::OK`].
    #[cold]
    #[inline(never)]
    fn column_info_uncached(&self, term_index: usize) -> Result<BamColumnInfo, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let Some(info_fn) = self.host.api().iter_column_info else {
            return Err(FfiResult::NULL_POINTER);
        };
        let mut out = MaybeUninit::<BamColumnInfo>::zeroed();
        // SAFETY: `self.raw` is valid for `'a` and `out` provides writable `BamColumnInfo` storage.
        let status = unsafe { info_fn(self.raw.cast_const(), term_index, out.as_mut_ptr()) };
        if status.is_ok() {
            // SAFETY: Host reported `FfiResult::OK`, confirming `*out` was initialized.
            let info = unsafe { out.assume_init() };
            if term_index < INLINE_COL_INFO_CAP {
                if let Some(scratch) = self.slot_cache.and_then(SlotCacheRef::as_mutable) {
                    // SAFETY: `scratch` belongs exclusively to this `ValidatedIter<'a>`,
                    // `term_index < INLINE_COL_INFO_CAP`, and writing directly via `ptr::write`
                    // through `UnsafeCell::get()` preserves `SharedReadWrite` provenance under Miri
                    // Stacked/Tree Borrows.
                    unsafe {
                        scratch
                            .col_infos
                            .get()
                            .cast::<BamColumnInfo>()
                            .add(term_index)
                            .write(info);
                    }
                    scratch
                        .col_info_mask
                        .set(scratch.col_info_mask.get() | (1u8 << term_index));
                }
            }
            Ok(info)
        } else {
            Err(status)
        }
    }

    /// Looks up a persistent validated term entry matching `(term_index, kind, TypeId::of::<T>(),
    /// expected_mode, expected_uuid)`.
    #[inline]
    fn cached_term_entry<T: 'static>(
        &self,
        term_index: usize,
        kind: CachedTermKind,
        expected_mode: AccessMode,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<CachedTermEntry> {
        if self.raw.is_null() || term_index >= INLINE_COL_INFO_CAP {
            return None;
        }
        let cache = self.slot_cache?;
        let bit = 1u8 << term_index;
        if let Some(scratch) = cache.as_mutable() {
            if (scratch.term_cache_mask.get() & bit) != 0 {
                // SAFETY: `term_cache_mask & bit != 0` proves slot `term_index` was initialized
                // by `store_cached_term` and `CachedTermEntry` is `Copy`.
                let entry = unsafe {
                    scratch
                        .term_entries
                        .get()
                        .cast::<CachedTermEntry>()
                        .add(term_index)
                        .read()
                };
                if entry.matches::<T>(kind, expected_mode, expected_uuid) {
                    return Some(entry);
                }
            }
        } else if let Some(frozen) = cache.as_frozen() {
            if (frozen.term_cache_mask & bit) != 0 {
                // SAFETY: `term_cache_mask & bit != 0` proves `frozen.term_entries[term_index]`
                // was initialized by `snapshot_frozen`.
                let entry = unsafe { frozen.term_entries[term_index].assume_init_read() };
                if entry.matches::<T>(kind, expected_mode, expected_uuid) {
                    return Some(entry);
                }
            }
        }
        None
    }

    /// Stores a validated term entry into `slot_cache` when mutable and `term_index < INLINE_COL_INFO_CAP`.
    #[inline]
    fn store_cached_term(&self, term_index: usize, entry: CachedTermEntry) {
        if term_index >= INLINE_COL_INFO_CAP {
            return;
        }
        let Some(scratch) = self.slot_cache.and_then(SlotCacheRef::as_mutable) else {
            return;
        };
        // SAFETY: `scratch` belongs exclusively to this `ValidatedIter<'a>`,
        // `term_index < INLINE_COL_INFO_CAP`, and writing via `ptr::write` through
        // `UnsafeCell::get()` preserves `SharedReadWrite` provenance under Miri.
        unsafe {
            scratch
                .term_entries
                .get()
                .cast::<CachedTermEntry>()
                .add(term_index)
                .write(entry);
        }
        scratch
            .term_cache_mask
            .set(scratch.term_cache_mask.get() | (1u8 << term_index));
    }

    /// Validates a dense read term (`READ` / `CHANGED` / `ADDED`) via the persistent term cache,
    /// falling back to `column_info` + `validate_dense_read_info` on miss or key mismatch.
    #[inline]
    fn validate_dense_read_term<T: AbiPod>(
        &self,
        term_index: usize,
        expected_mode: AccessMode,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<BamComponent, FfiResult> {
        if let Some(entry) = self.cached_term_entry::<T>(
            term_index,
            CachedTermKind::DenseRead,
            expected_mode,
            expected_uuid,
        ) {
            return Ok(entry.component);
        }
        let info = self.column_info(term_index)?;
        if info.mode != expected_mode || !validate_dense_read_info::<T>(&info, expected_uuid) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        self.store_cached_term(
            term_index,
            CachedTermEntry {
                type_id: core::any::TypeId::of::<T>(),
                expected_uuid,
                expected_mode,
                info_mode: info.mode,
                component: info.component,
                kind: CachedTermKind::DenseRead,
            },
        );
        Ok(info.component)
    }

    /// Validates a dense writable term (`WRITE`) via the persistent term cache.
    #[inline]
    fn validate_dense_write_term<T: AbiPod>(
        &self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<BamComponent, FfiResult> {
        if let Some(entry) = self.cached_term_entry::<T>(
            term_index,
            CachedTermKind::DenseWrite,
            AccessMode::WRITE,
            expected_uuid,
        ) {
            return Ok(entry.component);
        }
        let info = self.column_info(term_index)?;
        if info.mode != AccessMode::WRITE || !validate_dense_write_info::<T>(&info, expected_uuid) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        self.store_cached_term(
            term_index,
            CachedTermEntry {
                type_id: core::any::TypeId::of::<T>(),
                expected_uuid,
                expected_mode: AccessMode::WRITE,
                info_mode: info.mode,
                component: info.component,
                kind: CachedTermKind::DenseWrite,
            },
        );
        Ok(info.component)
    }

    /// Validates an optional dense read term (`OPTIONAL_READ`) via the persistent term cache.
    #[inline]
    fn validate_optional_read_term<T: AbiPod>(
        &self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<BamComponent, FfiResult> {
        if let Some(entry) = self.cached_term_entry::<T>(
            term_index,
            CachedTermKind::OptionalRead,
            AccessMode::OPTIONAL_READ,
            expected_uuid,
        ) {
            return Ok(entry.component);
        }
        let info = self.column_info(term_index)?;
        if info.mode != AccessMode::OPTIONAL_READ
            || !validate_dense_read_info::<T>(&info, expected_uuid)
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        self.store_cached_term(
            term_index,
            CachedTermEntry {
                type_id: core::any::TypeId::of::<T>(),
                expected_uuid,
                expected_mode: AccessMode::OPTIONAL_READ,
                info_mode: info.mode,
                component: info.component,
                kind: CachedTermKind::OptionalRead,
            },
        );
        Ok(info.component)
    }

    /// Validates an optional dense write term (`OPTIONAL_WRITE`) via the persistent term cache.
    #[inline]
    fn validate_optional_write_term<T: AbiPod>(
        &self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<BamComponent, FfiResult> {
        if let Some(entry) = self.cached_term_entry::<T>(
            term_index,
            CachedTermKind::OptionalWrite,
            AccessMode::OPTIONAL_WRITE,
            expected_uuid,
        ) {
            return Ok(entry.component);
        }
        let info = self.column_info(term_index)?;
        if info.mode != AccessMode::OPTIONAL_WRITE
            || !validate_dense_write_info::<T>(&info, expected_uuid)
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        self.store_cached_term(
            term_index,
            CachedTermEntry {
                type_id: core::any::TypeId::of::<T>(),
                expected_uuid,
                expected_mode: AccessMode::OPTIONAL_WRITE,
                info_mode: info.mode,
                component: info.component,
                kind: CachedTermKind::OptionalWrite,
            },
        );
        Ok(info.component)
    }

    /// Validates a typed filter term (`FILTER_ONLY` / `WITHOUT` / `ALLOW`) via the persistent cache.
    #[inline]
    fn validate_filter_term<T: AbiPod>(
        &self,
        term_index: usize,
        expected_mode: AccessMode,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<(), FfiResult> {
        if self
            .cached_term_entry::<T>(
                term_index,
                CachedTermKind::Filter,
                expected_mode,
                expected_uuid,
            )
            .is_some()
        {
            return Ok(());
        }
        let info = self.column_info(term_index)?;
        if info.mode != expected_mode || info.is_resource() || info.is_message() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if let Some(filter) = expected_uuid
            && !filter.matches(info.uuid)
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        self.store_cached_term(
            term_index,
            CachedTermEntry {
                type_id: core::any::TypeId::of::<T>(),
                expected_uuid,
                expected_mode,
                info_mode: info.mode,
                component: info.component,
                kind: CachedTermKind::Filter,
            },
        );
        Ok(())
    }

    /// Validates an untyped/bridged filter term (`FILTER_ONLY` / `WITHOUT` / `ALLOW`) via the
    /// persistent cache.
    #[inline]
    fn validate_filter_untyped_term(
        &self,
        term_index: usize,
        expected_mode: AccessMode,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<(), FfiResult> {
        if self
            .cached_term_entry::<()>(
                term_index,
                CachedTermKind::FilterUntyped,
                expected_mode,
                expected_uuid,
            )
            .is_some()
        {
            return Ok(());
        }
        let info = self.column_info(term_index)?;
        if info.mode != expected_mode || info.is_resource() || info.is_message() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if let Some(filter) = expected_uuid
            && !filter.matches(info.uuid)
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        self.store_cached_term(
            term_index,
            CachedTermEntry {
                type_id: core::any::TypeId::of::<()>(),
                expected_uuid,
                expected_mode,
                info_mode: info.mode,
                component: info.component,
                kind: CachedTermKind::FilterUntyped,
            },
        );
        Ok(())
    }

    /// Validates a typed `AccessMode::HAS` term via the persistent cache.
    #[inline]
    fn validate_has_term<T: AbiPod>(
        &self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<(), FfiResult> {
        if self
            .cached_term_entry::<T>(
                term_index,
                CachedTermKind::Has,
                AccessMode::HAS,
                expected_uuid,
            )
            .is_some()
        {
            return Ok(());
        }
        let info = self.column_info(term_index)?;
        if info.mode != AccessMode::HAS || info.is_resource() || info.is_message() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if let Some(filter) = expected_uuid
            && !filter.matches(info.uuid)
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        self.store_cached_term(
            term_index,
            CachedTermEntry {
                type_id: core::any::TypeId::of::<T>(),
                expected_uuid,
                expected_mode: AccessMode::HAS,
                info_mode: info.mode,
                component: info.component,
                kind: CachedTermKind::Has,
            },
        );
        Ok(())
    }

    /// Validates an untyped `AccessMode::HAS` term via the persistent cache.
    #[inline]
    fn validate_has_untyped_term(
        &self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<(), FfiResult> {
        if self
            .cached_term_entry::<()>(
                term_index,
                CachedTermKind::HasUntyped,
                AccessMode::HAS,
                expected_uuid,
            )
            .is_some()
        {
            return Ok(());
        }
        let info = self.column_info(term_index)?;
        if info.mode != AccessMode::HAS || info.is_resource() || info.is_message() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if let Some(filter) = expected_uuid
            && !filter.matches(info.uuid)
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        self.store_cached_term(
            term_index,
            CachedTermEntry {
                type_id: core::any::TypeId::of::<()>(),
                expected_uuid,
                expected_mode: AccessMode::HAS,
                info_mode: info.mode,
                component: info.component,
                kind: CachedTermKind::HasUntyped,
            },
        );
        Ok(())
    }

    /// Borrows the `[FfiEntity; current_len]` slice for the active chunk (`&[]` if empty or unavailable).
    #[inline]
    pub fn entities(&self) -> &[FfiEntity] {
        if self.raw.is_null() || self.current_len == 0 {
            return &[];
        }
        let Some(entities_fn) = self.host.api().iter_entities else {
            return &[];
        };
        // SAFETY: `self.raw` is positioned at an active chunk of `self.current_len` rows.
        let ptr = unsafe { entities_fn(self.raw.cast_const()) };
        if ptr.is_null() || !(ptr as usize).is_multiple_of(core::mem::align_of::<FfiEntity>()) {
            return &[];
        }
        let Some(byte_len) = self
            .current_len
            .checked_mul(core::mem::size_of::<FfiEntity>())
        else {
            return &[];
        };
        if byte_len > isize::MAX as usize {
            return &[];
        }
        // SAFETY: Verified non-null, aligned `FfiEntity` pointer covering `self.current_len` entities
        // valid until the next `next_chunk`/`reset` mutation of `self`.
        unsafe { core::slice::from_raw_parts(ptr, self.current_len) }
    }

    /// Retrieves per-row `added` and `changed` tick slices (`[u32; current_len]`) alongside the
    /// enclosing system's `(last_run, this_run)` tick window for dense column `term_index` in the
    /// active chunk via `BamHostApi::iter_column_ticks` (slot 69).
    ///
    /// Returns [`FfiResult::UNSUPPORTED`] when the host does not cover or populate slot 69,
    /// [`FfiResult::NULL_POINTER`] when `self` is null or the host returns null pointers for a
    /// non-empty chunk, [`FfiResult::TYPE_MISMATCH`] when returned pointers are misaligned for
    /// `u32`, or [`FfiResult::OUT_OF_RANGE`] if `current_len * size_of::<u32>() > isize::MAX`.
    #[inline]
    pub fn column_ticks(&self, term_index: usize) -> Result<(&[u32], &[u32], u32, u32), FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let ticks_fn =
            host_slot!(self.host.api(), iter_column_ticks).ok_or(FfiResult::UNSUPPORTED)?;
        let mut added_ptr: *const u32 = core::ptr::null();
        let mut changed_ptr: *const u32 = core::ptr::null();
        let mut last_run: u32 = 0;
        let mut this_run: u32 = 0;
        // SAFETY: `self.raw` is a valid active `BamIter` for `'a` and all four out-pointers point
        // to aligned writable stack storage.
        let status = unsafe {
            ticks_fn(
                self.raw.cast_const(),
                term_index,
                &mut added_ptr,
                &mut changed_ptr,
                &mut last_run,
                &mut this_run,
            )
        };
        if !status.is_ok() {
            return Err(status);
        }
        if self.current_len == 0 {
            return Ok((&[], &[], last_run, this_run));
        }
        if added_ptr.is_null() || changed_ptr.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        if !(added_ptr as usize).is_multiple_of(core::mem::align_of::<u32>())
            || !(changed_ptr as usize).is_multiple_of(core::mem::align_of::<u32>())
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        let byte_len = self
            .current_len
            .checked_mul(core::mem::size_of::<u32>())
            .ok_or(FfiResult::OUT_OF_RANGE)?;
        if byte_len > isize::MAX as usize {
            return Err(FfiResult::OUT_OF_RANGE);
        }
        // SAFETY: Host reported `OK`, both pointers are non-null and aligned for `u32`,
        // `byte_len <= isize::MAX`, and the tick arrays remain valid for shared reads until the
        // next chunk advance or reset on `self`.
        let added = unsafe { core::slice::from_raw_parts(added_ptr, self.current_len) };
        // SAFETY: Same invariants as `added` above.
        let changed = unsafe { core::slice::from_raw_parts(changed_ptr, self.current_len) };
        Ok((added, changed, last_run, this_run))
    }

    /// Raw read-only byte pointer for dense column `term_index` in the active chunk (or null).
    #[inline]
    pub fn column_readonly_ptr(&self, term_index: usize) -> *const u8 {
        if self.raw.is_null() || self.current_len == 0 {
            return core::ptr::null();
        }
        let Some(col_fn) = self.host.api().iter_column_readonly else {
            return core::ptr::null();
        };
        // SAFETY: `self.raw` is an active `BamIter` positioned at a valid chunk.
        unsafe { col_fn(self.raw.cast_const(), term_index) }
    }

    /// Raw mutable byte pointer for dense column `term_index` (marks chunk ticks changed; or null).
    #[inline]
    pub fn column_mutable_ptr(&mut self, term_index: usize) -> *mut u8 {
        if self.raw.is_null() || self.current_len == 0 {
            return core::ptr::null_mut();
        }
        let Some(col_fn) = self.host.api().iter_column_mutable else {
            return core::ptr::null_mut();
        };
        // SAFETY: `self.raw` is an active `BamIter` and `&mut self` prevents concurrent access.
        unsafe { col_fn(self.raw, term_index) }
    }

    /// Raw mutable byte pointer for dense column `term_index` without marking change ticks.
    #[inline]
    pub fn column_mutable_untracked_ptr(&mut self, term_index: usize) -> *mut u8 {
        if self.raw.is_null() || self.current_len == 0 {
            return core::ptr::null_mut();
        }
        let Some(col_fn) = self.host.api().iter_column_mutable_untracked else {
            return core::ptr::null_mut();
        };
        // SAFETY: `self.raw` is an active `BamIter` and `&mut self` prevents concurrent access.
        unsafe { col_fn(self.raw, term_index) }
    }

    /// Marks rows `start_row..start_row + row_count` in writable column `term_index` as changed.
    #[inline]
    pub fn mark_changed_range(
        &mut self,
        term_index: usize,
        start_row: usize,
        row_count: usize,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let Some(end_row) = start_row.checked_add(row_count) else {
            return FfiResult::OUT_OF_RANGE;
        };
        if end_row > self.current_len {
            return FfiResult::OUT_OF_RANGE;
        }
        let Some(mark_fn) = self.host.api().iter_mark_changed_range else {
            return FfiResult::NULL_POINTER;
        };
        // SAFETY: `self.raw` is active and `start_row..end_row` was bounds-checked against `current_len`.
        unsafe { mark_fn(self.raw, term_index, start_row, row_count) }
    }

    /// Marks a single row `row` in writable column `term_index` as changed.
    #[inline]
    pub fn mark_changed(&mut self, term_index: usize, row: usize) -> FfiResult {
        self.mark_changed_range(term_index, row, 1)
    }

    /// Returns a [`ValidatedSlice`] stamped with [`BamColumnInfo`] for dense read column `term_index`.
    #[inline]
    pub fn readonly_slice(&self, term_index: usize) -> ValidatedSlice<'_> {
        let Ok(info) = self.column_info(term_index) else {
            return ValidatedSlice::invalid();
        };
        if !info.mode.has_entity_column_data() || info.is_resource() || info.is_sparse_set() {
            return ValidatedSlice::invalid();
        }
        let ptr = self.column_readonly_ptr(term_index);
        // SAFETY: `ptr` comes from `iter_column_readonly` for `self.current_len` rows matching `info`.
        unsafe { ValidatedSlice::from_raw_parts_with_info(ptr, self.current_len, info) }
            .with_manifest_name(self.host.manifest_name())
    }

    /// Returns a [`ValidatedSliceMut`] (marking chunk ticks changed) stamped with [`BamColumnInfo`].
    #[inline]
    pub fn mutable_slice(&mut self, term_index: usize) -> ValidatedSliceMut<'_> {
        let Ok(info) = self.column_info(term_index) else {
            return ValidatedSliceMut::invalid();
        };
        if !info.mode.is_mutable()
            || !info.mode.has_entity_column_data()
            || info.is_immutable()
            || info.is_resource()
            || info.is_sparse_set()
        {
            return ValidatedSliceMut::invalid();
        }
        let len = self.current_len;
        let ptr = self.column_mutable_ptr(term_index);
        let manifest_name = self.host.manifest_name();
        // SAFETY: `ptr` comes from `iter_column_mutable` for `len` rows under `&mut self`.
        unsafe { ValidatedSliceMut::from_raw_parts_with_info(ptr, len, info) }
            .with_manifest_name(manifest_name)
    }

    /// Returns an untracked [`ValidatedSliceMut`] (without marking chunk ticks changed) stamped
    /// with [`BamColumnInfo`].
    #[inline]
    pub fn mutable_slice_untracked(&mut self, term_index: usize) -> ValidatedSliceMut<'_> {
        let Ok(info) = self.column_info(term_index) else {
            return ValidatedSliceMut::invalid();
        };
        if !info.mode.is_mutable()
            || !info.mode.has_entity_column_data()
            || info.is_immutable()
            || info.is_resource()
            || info.is_sparse_set()
        {
            return ValidatedSliceMut::invalid();
        }
        let len = self.current_len;
        let ptr = self.column_mutable_untracked_ptr(term_index);
        let manifest_name = self.host.manifest_name();
        // SAFETY: `ptr` comes from `iter_column_mutable_untracked` for `len` rows under `&mut self`.
        unsafe { ValidatedSliceMut::from_raw_parts_with_info(ptr, len, info) }
            .with_manifest_name(manifest_name)
    }

    /// Borrows dense column `term_index` as `&[T]` after validating [`BamColumnInfo`] and optional
    /// `expected_uuid`.
    #[inline]
    pub fn column_checked<T: AbiPod>(
        &self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&[T]> {
        let info_mode = if let Some(entry) = self.cached_term_entry::<T>(
            term_index,
            CachedTermKind::DenseReadAny,
            AccessMode::READ,
            expected_uuid,
        ) {
            entry.info_mode
        } else {
            let info = self.column_info(term_index).ok()?;
            if !validate_dense_read_info::<T>(&info, expected_uuid) {
                return None;
            }
            self.store_cached_term(
                term_index,
                CachedTermEntry {
                    type_id: core::any::TypeId::of::<T>(),
                    expected_uuid,
                    expected_mode: AccessMode::READ,
                    info_mode: info.mode,
                    component: info.component,
                    kind: CachedTermKind::DenseReadAny,
                },
            );
            info.mode
        };
        if self.current_len == 0 {
            if info_mode.is_optional() {
                return None;
            }
            return Some(&[]);
        }
        let ptr = self.column_readonly_ptr(term_index);
        // SAFETY: `validate_dense_read_info` confirmed layout/UUID/mode and `ptr` covers `current_len` rows.
        unsafe { raw_dense_slice::<T>(ptr, self.current_len) }
    }

    /// Borrows dense column `term_index` as `&mut [T]` (marking chunk ticks changed) after
    /// validating [`BamColumnInfo`] and optional `expected_uuid`.
    #[inline]
    pub fn column_mut_checked<T: AbiPod>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&mut [T]> {
        let info_mode = if let Some(entry) = self.cached_term_entry::<T>(
            term_index,
            CachedTermKind::DenseWriteAny,
            AccessMode::WRITE,
            expected_uuid,
        ) {
            entry.info_mode
        } else {
            let info = self.column_info(term_index).ok()?;
            if !validate_dense_write_info::<T>(&info, expected_uuid) {
                return None;
            }
            self.store_cached_term(
                term_index,
                CachedTermEntry {
                    type_id: core::any::TypeId::of::<T>(),
                    expected_uuid,
                    expected_mode: AccessMode::WRITE,
                    info_mode: info.mode,
                    component: info.component,
                    kind: CachedTermKind::DenseWriteAny,
                },
            );
            info.mode
        };
        let len = self.current_len;
        if len == 0 {
            if info_mode.is_optional() {
                return None;
            }
            return Some(&mut []);
        }
        let ptr = self.column_mutable_ptr(term_index);
        // SAFETY: `validate_dense_write_info` confirmed writable dense `T` column under `&mut self`.
        unsafe { raw_dense_slice_mut::<T>(ptr, len) }
    }

    /// Borrows dense column `term_index` as `&mut [T]` **without** automatically marking change ticks,
    /// after validating [`BamColumnInfo`] and optional `expected_uuid`.
    #[inline]
    pub fn column_mut_untracked_checked<T: AbiPod>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&mut [T]> {
        let info_mode = if let Some(entry) = self.cached_term_entry::<T>(
            term_index,
            CachedTermKind::DenseWriteAny,
            AccessMode::WRITE,
            expected_uuid,
        ) {
            entry.info_mode
        } else {
            let info = self.column_info(term_index).ok()?;
            if !validate_dense_write_info::<T>(&info, expected_uuid) {
                return None;
            }
            self.store_cached_term(
                term_index,
                CachedTermEntry {
                    type_id: core::any::TypeId::of::<T>(),
                    expected_uuid,
                    expected_mode: AccessMode::WRITE,
                    info_mode: info.mode,
                    component: info.component,
                    kind: CachedTermKind::DenseWriteAny,
                },
            );
            info.mode
        };
        let len = self.current_len;
        if len == 0 {
            if info_mode.is_optional() {
                return None;
            }
            return Some(&mut []);
        }
        let ptr = self.column_mutable_untracked_ptr(term_index);
        // SAFETY: `validate_dense_write_info` confirmed writable dense `T` column under `&mut self`.
        unsafe { raw_dense_slice_mut::<T>(ptr, len) }
    }

    /// Simultaneously borrows one read-only dense column (`read_col`) and one mutable dense column
    /// (`write_col`), rejecting identical term indices, identical component handles, or overlapping
    /// byte ranges.
    #[inline]
    pub fn read_write_checked<R: AbiPod, W: AbiPod>(
        &mut self,
        read_col: usize,
        read_uuid: Option<UuidFilter>,
        write_col: usize,
        write_uuid: Option<UuidFilter>,
    ) -> Option<(&[R], &mut [W])> {
        if read_col == write_col {
            return None;
        }
        let r_info = self.column_info(read_col).ok()?;
        let w_info = self.column_info(write_col).ok()?;
        if r_info.component == w_info.component {
            return None;
        }
        if !validate_dense_read_info::<R>(&r_info, read_uuid)
            || !validate_dense_write_info::<W>(&w_info, write_uuid)
        {
            return None;
        }
        let len = self.current_len;
        if len == 0 {
            if r_info.mode.is_optional() || w_info.mode.is_optional() {
                return None;
            }
            return Some((&[], &mut []));
        }
        let r_ptr = self.column_readonly_ptr(read_col);
        let w_ptr = self.column_mutable_ptr(write_col);
        let r_size = core::mem::size_of::<R>();
        let w_size = core::mem::size_of::<W>();
        if r_size > 0 && w_size > 0 {
            if r_ptr.is_null() || w_ptr.is_null() {
                return None;
            }
            let r_bytes = len.checked_mul(r_size)?;
            let w_bytes = len.checked_mul(w_size)?;
            let r_start = r_ptr as usize;
            let w_start = w_ptr as usize;
            let r_end = r_start.checked_add(r_bytes)?;
            let w_end = w_start.checked_add(w_bytes)?;
            if r_start < w_end && w_start < r_end {
                return None;
            }
        }
        // SAFETY: Verified distinct columns, distinct `BamComponent` handles, non-overlapping byte
        // ranges, and matching `BamColumnInfo` layouts under `&mut self`.
        let r_slice = unsafe { raw_dense_slice::<R>(r_ptr, len) }?;
        // SAFETY: Disjoint from `r_slice` and exclusively borrowed for `&mut self`.
        let w_slice = unsafe { raw_dense_slice_mut::<W>(w_ptr, len) }?;
        Some((r_slice, w_slice))
    }

    /// Simultaneously borrows two disjoint mutable dense columns (`col_a` and `col_b`).
    #[inline]
    pub fn write_2_checked<W1: AbiPod, W2: AbiPod>(
        &mut self,
        col_a: usize,
        uuid_a: Option<UuidFilter>,
        col_b: usize,
        uuid_b: Option<UuidFilter>,
    ) -> Option<(&mut [W1], &mut [W2])> {
        if col_a == col_b {
            return None;
        }
        let info_a = self.column_info(col_a).ok()?;
        let info_b = self.column_info(col_b).ok()?;
        if info_a.component == info_b.component {
            return None;
        }
        if !validate_dense_write_info::<W1>(&info_a, uuid_a)
            || !validate_dense_write_info::<W2>(&info_b, uuid_b)
        {
            return None;
        }
        let len = self.current_len;
        if len == 0 {
            if info_a.mode.is_optional() || info_b.mode.is_optional() {
                return None;
            }
            return Some((&mut [], &mut []));
        }
        let ptr_a = self.column_mutable_ptr(col_a);
        let ptr_b = self.column_mutable_ptr(col_b);
        let size_a = core::mem::size_of::<W1>();
        let size_b = core::mem::size_of::<W2>();
        if size_a > 0 && size_b > 0 {
            if ptr_a.is_null() || ptr_b.is_null() {
                return None;
            }
            let bytes_a = len.checked_mul(size_a)?;
            let bytes_b = len.checked_mul(size_b)?;
            let start_a = ptr_a as usize;
            let start_b = ptr_b as usize;
            let end_a = start_a.checked_add(bytes_a)?;
            let end_b = start_b.checked_add(bytes_b)?;
            if start_a < end_b && start_b < end_a {
                return None;
            }
        }
        // SAFETY: Verified distinct columns, distinct `BamComponent` handles, and non-overlapping
        // byte ranges under `&mut self`.
        let slice_a = unsafe { raw_dense_slice_mut::<W1>(ptr_a, len) }?;
        // SAFETY: Disjoint from `slice_a` and exclusively borrowed for `&mut self`.
        let slice_b = unsafe { raw_dense_slice_mut::<W2>(ptr_b, len) }?;
        Some((slice_a, slice_b))
    }

    /// Simultaneously borrows `[FfiEntity; current_len]` and mutable dense column `write_col`,
    /// verifying disjointness between the entity array and component column bytes.
    #[inline]
    pub fn entities_and_column_mut_checked<W: AbiPod>(
        &mut self,
        write_col: usize,
        write_uuid: Option<UuidFilter>,
    ) -> Option<(&[FfiEntity], &mut [W])> {
        let w_info = self.column_info(write_col).ok()?;
        if !validate_dense_write_info::<W>(&w_info, write_uuid) {
            return None;
        }
        let len = self.current_len;
        if len == 0 {
            if w_info.mode.is_optional() {
                return None;
            }
            return Some((&[], &mut []));
        }
        let entities_fn = self.host.api().iter_entities?;
        // SAFETY: `self.raw` is active at a chunk of `len` rows.
        let e_ptr = unsafe { entities_fn(self.raw.cast_const()) };
        if e_ptr.is_null() || !(e_ptr as usize).is_multiple_of(core::mem::align_of::<FfiEntity>()) {
            return None;
        }
        let e_bytes = len.checked_mul(core::mem::size_of::<FfiEntity>())?;
        if e_bytes > isize::MAX as usize {
            return None;
        }
        let w_ptr = self.column_mutable_ptr(write_col);
        let w_size = core::mem::size_of::<W>();
        if w_size > 0 {
            if w_ptr.is_null() {
                return None;
            }
            let w_bytes = len.checked_mul(w_size)?;
            let e_start = e_ptr as usize;
            let w_start = w_ptr as usize;
            let e_end = e_start.checked_add(e_bytes)?;
            let w_end = w_start.checked_add(w_bytes)?;
            if e_start < w_end && w_start < e_end {
                return None;
            }
        }
        // SAFETY: Entity slice and mutable component column were verified non-overlapping.
        let entities = unsafe { core::slice::from_raw_parts(e_ptr, len) };
        // SAFETY: Disjoint from `entities` and exclusively borrowed under `&mut self`.
        let w_slice = unsafe { raw_dense_slice_mut::<W>(w_ptr, len) }?;
        Some((entities, w_slice))
    }

    /// Simultaneously borrows `[FfiEntity; current_len]`, read-only dense column `read_col`, and
    /// disjoint mutable dense column `write_col`.
    #[inline]
    pub fn entities_and_read_write_checked<R: AbiPod, W: AbiPod>(
        &mut self,
        read_col: usize,
        read_uuid: Option<UuidFilter>,
        write_col: usize,
        write_uuid: Option<UuidFilter>,
    ) -> Option<(&[FfiEntity], &[R], &mut [W])> {
        if read_col == write_col {
            return None;
        }
        let r_info = self.column_info(read_col).ok()?;
        let w_info = self.column_info(write_col).ok()?;
        if r_info.component == w_info.component {
            return None;
        }
        if !validate_dense_read_info::<R>(&r_info, read_uuid)
            || !validate_dense_write_info::<W>(&w_info, write_uuid)
        {
            return None;
        }
        let len = self.current_len;
        if len == 0 {
            if r_info.mode.is_optional() || w_info.mode.is_optional() {
                return None;
            }
            return Some((&[], &[], &mut []));
        }
        let entities_fn = self.host.api().iter_entities?;
        // SAFETY: `self.raw` is active at a chunk of `len` rows.
        let e_ptr = unsafe { entities_fn(self.raw.cast_const()) };
        if e_ptr.is_null() || !(e_ptr as usize).is_multiple_of(core::mem::align_of::<FfiEntity>()) {
            return None;
        }
        let e_bytes = len.checked_mul(core::mem::size_of::<FfiEntity>())?;
        if e_bytes > isize::MAX as usize {
            return None;
        }
        let r_ptr = self.column_readonly_ptr(read_col);
        let w_ptr = self.column_mutable_ptr(write_col);
        let r_size = core::mem::size_of::<R>();
        let w_size = core::mem::size_of::<W>();
        if w_size > 0 {
            if w_ptr.is_null() {
                return None;
            }
            let w_bytes = len.checked_mul(w_size)?;
            let w_start = w_ptr as usize;
            let w_end = w_start.checked_add(w_bytes)?;
            let e_start = e_ptr as usize;
            let e_end = e_start.checked_add(e_bytes)?;
            if e_start < w_end && w_start < e_end {
                return None;
            }
            if r_size > 0 {
                if r_ptr.is_null() {
                    return None;
                }
                let r_bytes = len.checked_mul(r_size)?;
                let r_start = r_ptr as usize;
                let r_end = r_start.checked_add(r_bytes)?;
                if r_start < w_end && w_start < r_end {
                    return None;
                }
            }
        }
        // SAFETY: Verified non-overlapping with `w_ptr` and aligned for `FfiEntity`.
        let entities = unsafe { core::slice::from_raw_parts(e_ptr, len) };
        // SAFETY: Verified non-overlapping with `w_ptr` and matching `BamColumnInfo`.
        let r_slice = unsafe { raw_dense_slice::<R>(r_ptr, len) }?;
        // SAFETY: Disjoint from `entities` and `r_slice` under `&mut self`.
        let w_slice = unsafe { raw_dense_slice_mut::<W>(w_ptr, len) }?;
        Some((entities, r_slice, w_slice))
    }

    /// Materializes indirect read-only sparse-set row pointers `[ *const u8; current_len ]` and
    /// wraps them in a validated [`SparseRows`] view.
    #[inline]
    pub fn rows_checked<T: AbiPod>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<SparseRows<'_, T>> {
        let info = self.column_info(term_index).ok()?;
        if !validate_rows_read_info::<T>(&info, expected_uuid) {
            return None;
        }
        let len = self.current_len;
        if len == 0 {
            // SAFETY: Zero-length `SparseRows` does not dereference its pointer.
            return unsafe { SparseRows::from_raw_parts(core::ptr::null(), 0, true) };
        }
        let rows_fn = self.host.api().iter_column_rows_readonly?;
        // SAFETY: `self.raw` is active at a chunk of `len` rows; `&mut self` keeps the host's
        // scratch row-pointer array alive and unaliased for the returned `SparseRows<'_, T>`.
        let ptrs = unsafe { rows_fn(self.raw, term_index) };
        if ptrs.is_null() {
            return None;
        }
        // SAFETY: `ptrs` covers `len` row pointers valid until `self` is next used.
        unsafe { SparseRows::from_raw_parts(ptrs, len, info.mode.is_optional()) }
    }

    /// Materializes indirect mutable sparse-set row pointers `[ *mut u8; current_len ]` and wraps
    /// them in a validated [`SparseRowsMut`] view.
    #[inline]
    pub fn rows_mut_checked<T: AbiPod>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<SparseRowsMut<'_, T>> {
        let info = self.column_info(term_index).ok()?;
        if !validate_rows_write_info::<T>(&info, expected_uuid) {
            return None;
        }
        let len = self.current_len;
        if len == 0 {
            // SAFETY: Zero-length `SparseRowsMut` does not dereference its pointer.
            return unsafe { SparseRowsMut::from_raw_parts(core::ptr::null(), 0, true) };
        }
        let rows_fn = self.host.api().iter_column_rows_mutable?;
        // SAFETY: `self.raw` is active at a chunk of `len` rows; `&mut self` keeps the host's
        // scratch row-pointer array and component borrows exclusive for `'self`.
        let ptrs = unsafe { rows_fn(self.raw, term_index) };
        if ptrs.is_null() {
            return None;
        }
        // SAFETY: `ptrs` covers `len` writable row pointers valid until `self` is next used.
        unsafe { SparseRowsMut::from_raw_parts(ptrs, len, info.mode.is_optional()) }
    }

    /// Returns the raw read-only singleton resource pointer at `term_index` (or null).
    #[inline]
    pub fn res_ptr(&self, term_index: usize) -> *const u8 {
        if self.raw.is_null() {
            return core::ptr::null();
        }
        let Some(res_fn) = self.host.api().res_ptr else {
            return core::ptr::null();
        };
        // SAFETY: `self.raw` is a valid active `BamIter` for `'a`.
        unsafe { res_fn(self.raw.cast_const(), term_index) }
    }

    /// Returns the raw mutable singleton resource pointer at `term_index` (or null).
    #[inline]
    pub fn res_mut_ptr(&mut self, term_index: usize) -> *mut u8 {
        if self.raw.is_null() {
            return core::ptr::null_mut();
        }
        let Some(res_mut_fn) = self.host.api().res_mut_ptr else {
            return core::ptr::null_mut();
        };
        // SAFETY: `self.raw` is an active `BamIter` and `&mut self` prevents concurrent access.
        unsafe { res_mut_fn(self.raw, term_index) }
    }

    /// Borrows singleton resource `R` declared at `term_index` after validating [`BamColumnInfo`]
    /// and optional `expected_uuid` (using the persistent per-system term cache on steady-state calls).
    #[inline]
    pub fn resource_checked<R: AbiPod>(
        &self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&R> {
        if self
            .cached_term_entry::<R>(
                term_index,
                CachedTermKind::ResourceRead,
                AccessMode::RESOURCE_READ,
                expected_uuid,
            )
            .is_none()
        {
            let info = self.column_info(term_index).ok()?;
            if !validate_resource_read_info::<R>(&info, expected_uuid) {
                return None;
            }
            self.store_cached_term(
                term_index,
                CachedTermEntry {
                    type_id: core::any::TypeId::of::<R>(),
                    expected_uuid,
                    expected_mode: AccessMode::RESOURCE_READ,
                    info_mode: info.mode,
                    component: info.component,
                    kind: CachedTermKind::ResourceRead,
                },
            );
        }
        let ptr = self.res_ptr(term_index);
        if ptr.is_null() {
            return None;
        }
        if core::mem::size_of::<R>() == 0 {
            // SAFETY: Zero-sized `R: AbiPod` may be referenced via `NonNull::dangling()`.
            return Some(unsafe { core::ptr::NonNull::<R>::dangling().as_ref() });
        }
        if !(ptr as usize).is_multiple_of(core::mem::align_of::<R>()) {
            return None;
        }
        // SAFETY: `validate_resource_read_info` confirmed `R`'s size, alignment, resource mode, and
        // optional UUID; `ptr` is non-null, aligned, and valid for `&self`.
        Some(unsafe { &*ptr.cast::<R>() })
    }

    /// Mutably borrows singleton resource `R` declared with `RESOURCE_WRITE` at `term_index` after
    /// validating [`BamColumnInfo`] and optional `expected_uuid` (using the persistent per-system
    /// term cache on steady-state calls).
    #[inline]
    pub fn resource_mut_checked<R: AbiPod>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<&mut R> {
        if self
            .cached_term_entry::<R>(
                term_index,
                CachedTermKind::ResourceWrite,
                AccessMode::RESOURCE_WRITE,
                expected_uuid,
            )
            .is_none()
        {
            let info = self.column_info(term_index).ok()?;
            if !validate_resource_write_info::<R>(&info, expected_uuid) {
                return None;
            }
            self.store_cached_term(
                term_index,
                CachedTermEntry {
                    type_id: core::any::TypeId::of::<R>(),
                    expected_uuid,
                    expected_mode: AccessMode::RESOURCE_WRITE,
                    info_mode: info.mode,
                    component: info.component,
                    kind: CachedTermKind::ResourceWrite,
                },
            );
        }
        let ptr = self.res_mut_ptr(term_index);
        if ptr.is_null() {
            return None;
        }
        if core::mem::size_of::<R>() == 0 {
            // SAFETY: Zero-sized `R: AbiPod` may be mutably referenced via `NonNull::dangling()`.
            return Some(unsafe { core::ptr::NonNull::<R>::dangling().as_mut() });
        }
        if !(ptr as usize).is_multiple_of(core::mem::align_of::<R>()) {
            return None;
        }
        // SAFETY: `validate_resource_write_info` confirmed `RESOURCE_WRITE`, non-immutable, layout,
        // and optional UUID; `&mut self` guarantees exclusive access.
        Some(unsafe { &mut *ptr.cast::<R>() })
    }

    /// Reads Bevy change-detection ticks for the singleton `Resource` declared at `term_index`.
    #[inline]
    pub fn res_ticks(&self, term_index: usize) -> Result<BamChangeTicks, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let ticks_fn = host_slot!(self.host.api(), res_ticks).ok_or(FfiResult::UNSUPPORTED)?;
        let mut out = BamChangeTicks::default();
        // SAFETY: `self.raw` is active and `&mut out` is aligned writable `BamChangeTicks` storage.
        let status = unsafe { ticks_fn(self.raw.cast_const(), term_index, &mut out) };
        if status.is_ok() { Ok(out) } else { Err(status) }
    }

    /// Borrows singleton resource `R` along with its Bevy change-detection ticks (`BamChangeTicks`).
    #[inline]
    pub fn resource_with_ticks_checked<R: AbiPod>(
        &self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<(&R, BamChangeTicks)> {
        let ticks = self.res_ticks(term_index).ok()?;
        let r = self.resource_checked::<R>(term_index, expected_uuid)?;
        Some((r, ticks))
    }

    /// Mutably borrows singleton resource `R` along with its prior Bevy change-detection ticks.
    #[inline]
    pub fn resource_mut_with_ticks_checked<R: AbiPod>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Option<(&mut R, BamChangeTicks)> {
        let ticks = self.res_ticks(term_index).ok()?;
        let r = self.resource_mut_checked::<R>(term_index, expected_uuid)?;
        Some((r, ticks))
    }

    /// Executes `f` in parallel across disjoint sub-range chunks on the host `ComputeTaskPool`
    ///, catching panics per task so unwinding never crosses `extern "C"`.
    pub fn par_for_each_chunk<F>(&mut self, f: F) -> FfiResult
    where
        F: Fn(&mut ValidatedIter<'_>) -> FfiResult + Sync,
    {
        if self.raw.is_null() {
            return FfiResult::OK;
        }
        let Some(par_fn) = host_slot!(self.host.api(), par_for_each_chunk) else {
            return FfiResult::UNSUPPORTED;
        };
        self.bump_batch_epoch();
        self.batch_entities = core::ptr::null();
        self.current_len = 0;
        let frozen_cache = self.slot_cache.and_then(|c| match c.as_mutable() {
            Some(s) => s.snapshot_frozen(),
            None => c.as_frozen().copied(),
        });
        let task_ctx = ParChunkTaskContext {
            host: self.host(),
            frozen_cache: frozen_cache.as_ref(),
            func: f,
        };
        // SAFETY: `self.raw` is an active top-level `BamIter`, `task_ctx` stays live on the stack
        // across the synchronous `par_fn` scope, `F: Sync` permits concurrent calls across worker
        // threads, and `par_chunk_task_trampoline` catches any panic on the guest heap.
        unsafe {
            par_fn(
                self.raw,
                Some(par_chunk_task_trampoline::<F>),
                (&raw const task_ctx).cast_mut().cast::<c_void>(),
            )
        }
    }

    /// Executes `f` in parallel across disjoint read-only sub-range chunks via
    /// `BamHostApi::par_for_each_chunk_readonly` (slot 59), catching panics per worker task.
    ///
    /// Hosts reject iterators containing writable terms with [`FfiResult::PERMISSION_DENIED`],
    /// and older ABI `0.2` hosts return [`FfiResult::UNSUPPORTED`].
    pub fn par_for_each_chunk_readonly<F>(&mut self, f: F) -> FfiResult
    where
        F: Fn(&mut ValidatedIter<'_>) -> FfiResult + Sync,
    {
        if self.raw.is_null() {
            return FfiResult::OK;
        }
        let Some(par_fn) = host_slot!(self.host.api(), par_for_each_chunk_readonly) else {
            return FfiResult::UNSUPPORTED;
        };
        self.bump_batch_epoch();
        self.batch_entities = core::ptr::null();
        self.current_len = 0;
        let frozen_cache = self.slot_cache.and_then(|c| match c.as_mutable() {
            Some(s) => s.snapshot_frozen(),
            None => c.as_frozen().copied(),
        });
        let task_ctx = ParChunkTaskContext {
            host: self.host(),
            frozen_cache: frozen_cache.as_ref(),
            func: f,
        };
        // SAFETY: `self.raw` is an active top-level `BamIter`, `task_ctx` lives on the stack across
        // the synchronous `par_fn` scope, `F: Sync` permits concurrent calls across worker threads,
        // and `par_chunk_task_trampoline` catches any panic on the guest heap.
        unsafe {
            par_fn(
                self.raw,
                Some(par_chunk_task_trampoline::<F>),
                (&raw const task_ctx).cast_mut().cast::<c_void>(),
            )
        }
    }

    /// Non-destructively counts all matching entities in this query via `BamHostApi::iter_count`
    /// (slot 57) without advancing or resetting the chunk cursor.
    ///
    /// Returns [`FfiResult::UNSUPPORTED`] when connected to an ABI `0.2` host.
    #[inline]
    pub fn count(&mut self) -> Result<usize, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let count_fn = host_slot!(self.host.api(), iter_count).ok_or(FfiResult::UNSUPPORTED)?;
        let mut out = 0usize;
        // SAFETY: `self.raw` is an active `BamIter` for `'a` and `&mut out` is aligned writable storage.
        let status = unsafe { count_fn(self.raw, &mut out) };
        if status.is_ok() { Ok(out) } else { Err(status) }
    }

    /// Walks an ancestor relationship chain upward along `relation` (`BamComponent::INVALID` selects
    /// the host's default `ChildOf` hierarchy when supported) starting from `entity` via
    /// `BamHostApi::iter_up_cascade` (slot 58) until locating an ancestor carrying the read-only
    /// component declared at `term_index`.
    ///
    /// Validates `(mode, size, align)` and optional `expected_uuid` via [`Self::column_info`],
    /// rejecting writable terms with [`FfiResult::PERMISSION_DENIED`] and mismatched types with
    /// [`FfiResult::TYPE_MISMATCH`]. Returns `Ok(None)` when no ancestor carries the component
    /// (whether reported by [`FfiResult::NOT_FOUND`] or a null output pointer/owner).
    pub fn up_cascade_checked<T: AbiPod>(
        &mut self,
        relation: BamComponent,
        entity: FfiEntity,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<Option<(FfiEntity, &T)>, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        if entity.is_null() {
            return Err(FfiResult::INVALID_ENTITY);
        }
        let cascade_fn =
            host_slot!(self.host.api(), iter_up_cascade).ok_or(FfiResult::UNSUPPORTED)?;
        let info = self.column_info(term_index)?;
        if info.mode.is_mutable() {
            return Err(FfiResult::PERMISSION_DENIED);
        }
        if !info.mode.has_entity_column_data() || info.is_resource() || info.is_message() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if info.size != core::mem::size_of::<T>() || info.align != core::mem::align_of::<T>() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if let Some(filter) = expected_uuid
            && !filter.matches(info.uuid)
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        let mut ptr: *const c_void = core::ptr::null();
        let mut owner = FfiEntity::NULL;
        // SAFETY: `self.raw` is an active `BamIter` under `&'b mut self` (preventing concurrent
        // iterator advancement or mutable column aliasing for `'b`), and `ptr`/`owner` are aligned
        // writable stack slots.
        let status =
            unsafe { cascade_fn(self.raw, relation, entity, term_index, &mut ptr, &mut owner) };
        if status == FfiResult::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_ok() {
            return Err(status);
        }
        if owner.is_null() {
            return Ok(None);
        }
        if core::mem::size_of::<T>() == 0 {
            // SAFETY: Zero-sized `T: AbiPod` may be referenced via `NonNull::dangling()`.
            return Ok(Some((owner, unsafe {
                core::ptr::NonNull::<T>::dangling().as_ref()
            })));
        }
        if ptr.is_null() {
            return Ok(None);
        }
        if !(ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        // SAFETY: `column_info` verified `T`'s layout, read-only mode, and optional UUID; `ptr` is
        // non-null, aligned for `T`, and valid for shared reads throughout `'b`.
        Ok(Some((owner, unsafe { &*ptr.cast::<T>() })))
    }

    /// Reads up to `max_count` unread POD messages from `term_index` (`MESSAGE_READ` or
    /// `MESSAGE_WRITE`), advancing the running system's per-term message cursor.
    pub fn read_messages_batch_checked<M: AbiPod>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
        max_count: usize,
    ) -> Result<Vec<M>, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let read_fn = host_slot!(self.host.api(), msg_read_batch).ok_or(FfiResult::UNSUPPORTED)?;
        let info = self.column_info(term_index)?;
        if !validate_message_read_info::<M>(&info, expected_uuid) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if max_count == 0 {
            return Ok(Vec::new());
        }
        let mut scratch: Vec<MaybeUninit<M>> = Vec::with_capacity(max_count);
        let out_ptr = if core::mem::size_of::<M>() == 0 {
            core::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            scratch.as_mut_ptr().cast::<u8>()
        };
        let mut read_count = 0usize;
        // SAFETY: `self.raw` is active, `validate_message_read_info` confirmed element size and
        // alignment match `M`, and `scratch` has capacity for `max_count` elements of `M`.
        let status = unsafe { read_fn(self.raw, term_index, out_ptr, max_count, &mut read_count) };
        if !status.is_ok() {
            return Err(status);
        }
        let actual = read_count.min(max_count);
        // SAFETY: Host copied `actual` initialized elements of `M` into `scratch[..actual]`.
        unsafe {
            scratch.set_len(actual);
        }
        let mut out = Vec::with_capacity(actual);
        for slot in scratch {
            // SAFETY: Every element in `scratch[..actual]` was initialized by `msg_read_batch`.
            out.push(unsafe { slot.assume_init() });
        }
        Ok(out)
    }

    /// Drains all currently unread POD messages from `term_index`.
    pub fn drain_messages_checked<M: AbiPod>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<Vec<M>, FfiResult> {
        const BATCH_SIZE: usize = 64;
        let mut all = Vec::new();
        loop {
            let batch =
                self.read_messages_batch_checked::<M>(term_index, expected_uuid, BATCH_SIZE)?;
            let count = batch.len();
            all.extend(batch);
            if count < BATCH_SIZE {
                break;
            }
        }
        Ok(all)
    }

    /// Appends a POD message `msg` to the Bevy `Messages<M>` stream declared with `MESSAGE_WRITE`
    /// at `term_index`.
    pub fn send_message_checked<M: AbiPod>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
        msg: &M,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let Some(send_fn) = host_slot!(self.host.api(), msg_send) else {
            return FfiResult::UNSUPPORTED;
        };
        let info = match self.column_info(term_index) {
            Ok(i) => i,
            Err(err) => return err,
        };
        if !validate_message_write_info::<M>(&info, expected_uuid) {
            return FfiResult::TYPE_MISMATCH;
        }
        let data_ptr = if core::mem::size_of::<M>() == 0 {
            core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const()
        } else {
            core::ptr::from_ref(msg).cast::<u8>()
        };
        // SAFETY: `self.raw` is active, `validate_message_write_info` verified `M`'s exact layout
        // and `MESSAGE_WRITE` permission, and `data_ptr` points to `size_of::<M>()` readable bytes.
        unsafe { send_fn(self.raw, term_index, data_ptr, core::mem::size_of::<M>()) }
    }

    /// Batch-appends `messages` (`M: AbiPod`) into `Messages<M>` declared with `MESSAGE_WRITE` at
    /// `term_index`.
    pub fn send_messages_batch_checked<M: AbiPod>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
        messages: &[M],
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let Some(write_fn) = host_slot!(self.host.api(), msg_write_batch) else {
            return FfiResult::UNSUPPORTED;
        };
        let info = match self.column_info(term_index) {
            Ok(i) => i,
            Err(err) => return err,
        };
        if !validate_message_write_info::<M>(&info, expected_uuid) {
            return FfiResult::TYPE_MISMATCH;
        }
        if messages.is_empty() {
            return FfiResult::OK;
        }
        let data_ptr = if core::mem::size_of::<M>() == 0 {
            core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const()
        } else {
            messages.as_ptr().cast::<u8>()
        };
        // SAFETY: `self.raw` is active, `validate_message_write_info` confirmed layout and
        // `MESSAGE_WRITE`, and `data_ptr` covers `messages.len() * size_of::<M>()` initialized bytes.
        unsafe { write_fn(self.raw, term_index, data_ptr, messages.len()) }
    }

    /// Reads up to `out_ptrs.len()` unread non-POD message pointers (`*const c_void` referencing
    /// host `M` items inside `Messages<M>`) from `term_index`, advancing this system's cursor.
    ///
    /// Returned pointers remain valid only for the synchronous duration of the active system tick.
    pub fn read_non_pod_message_ptrs_checked(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
        out_ptrs: &mut [*const c_void],
    ) -> Result<usize, FfiResult> {
        const _: () = {
            assert!(
                core::mem::size_of::<*const c_void>() == core::mem::size_of::<usize>()
                    && core::mem::align_of::<*const c_void>() == core::mem::align_of::<usize>()
            );
        };
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let read_fn = host_slot!(self.host.api(), msg_read_batch).ok_or(FfiResult::UNSUPPORTED)?;
        let info = self.column_info(term_index)?;
        if !validate_non_pod_message_info(&info, expected_uuid, false) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if out_ptrs.is_empty() {
            return Ok(0);
        }
        let mut read_count = 0usize;
        // SAFETY: `self.raw` is active, `out_ptrs` provides `out_ptrs.len()` writable
        // `usize`/`*const c_void` slots, and `validate_non_pod_message_info` verified `term_index`.
        let status = unsafe {
            read_fn(
                self.raw,
                term_index,
                out_ptrs.as_mut_ptr().cast::<u8>(),
                out_ptrs.len(),
                &mut read_count,
            )
        };
        if !status.is_ok() {
            return Err(status);
        }
        Ok(read_count.min(out_ptrs.len()))
    }

    /// Verifies that the host registered the `"bam.v0.<M>.non_pod_message"` custom symbol for `M`,
    /// confirming non-POD message wire compatibility before marshaling.
    fn verify_non_pod_message_marker<M: ModComponent>(&self) -> Result<(), FfiResult> {
        let host = self.host();
        let Ok(ptr) = host.cached_symbol_lookup(
            (
                core::any::TypeId::of::<M>(),
                crate::host::symbol_cache_kind::NON_POD_MESSAGE_MARKER,
            ),
            || {
                let marker_sym =
                    CString::new(format!("bam.v0.{}.non_pod_message", M::CANONICAL_NAME))
                        .map_err(|_| FfiResult::TYPE_MISMATCH)?;
                host.resolve_custom_symbol(&marker_sym)
                    .map_err(|_| FfiResult::TYPE_MISMATCH)
            },
        ) else {
            return Err(FfiResult::TYPE_MISMATCH);
        };
        if ptr.is_null() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        Ok(())
    }

    /// Reads up to `max_count` unread VTable-backed non-POD messages (`M: ModComponent + Default`)
    /// from `term_index`, reconstructing each host `M` into guest memory via the host's
    /// `"bam.v0.<M>.export_fields"` witness symbol and guest's `"bam.v0.<M>.import_field"`.
    pub fn read_non_pod_messages_checked<M: ModComponent + Default>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
        max_count: usize,
    ) -> Result<Vec<M>, FfiResult> {
        let info = self.column_info(term_index)?;
        let effective_uuid = expected_uuid.unwrap_or_else(|| UuidFilter::exact(M::UUID));
        if !validate_non_pod_message_info_for::<M>(
            &info,
            self.host.manifest_name(),
            Some(effective_uuid),
            false,
        ) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        self.verify_non_pod_message_marker::<M>()?;
        if max_count == 0 {
            return Ok(Vec::new());
        }

        let mut ptrs = vec![core::ptr::null::<c_void>(); max_count];
        let actual =
            self.read_non_pod_message_ptrs_checked(term_index, Some(effective_uuid), &mut ptrs)?;
        if actual == 0 {
            return Ok(Vec::new());
        }

        type ExportFieldsFn = unsafe extern "C" fn(
            src_ptr: *const c_void,
            sink: Option<
                unsafe extern "C" fn(
                    sink_ctx: *mut c_void,
                    field_name: StrView,
                    type_tag: StrView,
                    payload: FfiSlice<u8>,
                ) -> FfiResult,
            >,
            sink_ctx: *mut c_void,
        ) -> FfiResult;

        let host = self.host();
        let host_export_ptr = host.cached_symbol_lookup(
            (
                core::any::TypeId::of::<M>(),
                crate::host::symbol_cache_kind::HOST_EXPORT_FIELDS,
            ),
            || {
                let export_sym_name =
                    CString::new(format!("bam.v0.{}.export_fields", M::CANONICAL_NAME))
                        .map_err(|_| FfiResult::INVALID_ARGUMENT)?;
                host.resolve_custom_symbol(&export_sym_name)
            },
        )?;
        if host_export_ptr.is_null() {
            return Err(FfiResult::UNSUPPORTED);
        }
        // SAFETY: Host registered `"bam.v0.<M>.export_fields"` with the exact witness `ExportFieldsFn` ABI.
        let host_export_fn: ExportFieldsFn = unsafe { core::mem::transmute(host_export_ptr) };

        let guest_import_ptr = host.cached_symbol_lookup(
            (
                core::any::TypeId::of::<M>(),
                crate::host::symbol_cache_kind::GUEST_IMPORT_FIELD,
            ),
            || {
                let guest_import_sym = format!("bam.v0.{}.import_field", M::CANONICAL_NAME);
                M::resolve_symbol(&guest_import_sym).ok_or(FfiResult::UNSUPPORTED)
            },
        )?;
        if guest_import_ptr.is_null() {
            return Err(FfiResult::UNSUPPORTED);
        }
        // SAFETY: `M::symbols()` exports `"bam.v0.<M>.import_field"` with `WitnessImportFieldFn` ABI.
        let guest_import_fn: WitnessImportFieldFn =
            unsafe { core::mem::transmute(guest_import_ptr) };

        let mut out = Vec::with_capacity(actual);
        for &host_msg_ptr in &ptrs[..actual] {
            if host_msg_ptr.is_null() {
                return Err(FfiResult::NULL_POINTER);
            }
            let mut guest_msg = M::default();
            let mut sink_ctx = ImportSinkCtx {
                dst_ptr: (&raw mut guest_msg).cast::<c_void>(),
                import_fn: guest_import_fn,
            };
            // SAFETY: `host_msg_ptr` points to a live host `&M` inside `Messages<M>` for this tick;
            // `sink_ctx` lives on the stack across `host_export_fn`.
            let status = unsafe {
                host_export_fn(
                    host_msg_ptr,
                    Some(forward_field),
                    (&raw mut sink_ctx).cast::<c_void>(),
                )
            };
            if !status.is_ok() {
                return Err(status);
            }
            out.push(guest_msg);
        }
        Ok(out)
    }

    /// Drains all currently unread VTable-backed non-POD messages (`M: ModComponent + Default`)
    /// from `term_index`.
    pub fn drain_non_pod_messages_checked<M: ModComponent + Default>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<Vec<M>, FfiResult> {
        const BATCH_SIZE: usize = 64;
        let mut all = Vec::new();
        loop {
            let batch =
                self.read_non_pod_messages_checked::<M>(term_index, expected_uuid, BATCH_SIZE)?;
            let count = batch.len();
            all.extend(batch);
            if count < BATCH_SIZE {
                break;
            }
        }
        Ok(all)
    }

    /// Encodes a non-POD message `msg` (`M: ModComponent`) via `"bam.v0.<M>.export_fields"` into
    /// a witness field packet `[field_count: u32; (name, tag, payload)*]` and sends it through
    /// `BamHostApi::msg_send` on `term_index`.
    pub fn send_non_pod_message_checked<M: ModComponent>(
        &mut self,
        term_index: usize,
        expected_uuid: Option<UuidFilter>,
        msg: &M,
    ) -> FfiResult {
        if self.raw.is_null() {
            return FfiResult::NULL_POINTER;
        }
        let Some(send_fn) = host_slot!(self.host.api(), msg_send) else {
            return FfiResult::UNSUPPORTED;
        };
        let info = match self.column_info(term_index) {
            Ok(i) => i,
            Err(err) => return err,
        };
        let effective_uuid = expected_uuid.unwrap_or_else(|| UuidFilter::exact(M::UUID));
        if !validate_non_pod_message_info_for::<M>(
            &info,
            self.host.manifest_name(),
            Some(effective_uuid),
            true,
        ) {
            return FfiResult::TYPE_MISMATCH;
        }
        if let Err(err) = self.verify_non_pod_message_marker::<M>() {
            return err;
        }

        type ExportFieldsFn = unsafe extern "C" fn(
            src_ptr: *const c_void,
            sink: Option<
                unsafe extern "C" fn(
                    sink_ctx: *mut c_void,
                    field_name: StrView,
                    type_tag: StrView,
                    payload: FfiSlice<u8>,
                ) -> FfiResult,
            >,
            sink_ctx: *mut c_void,
        ) -> FfiResult;

        let host = self.host();
        let export_ptr = match host.cached_symbol_lookup(
            (
                core::any::TypeId::of::<M>(),
                crate::host::symbol_cache_kind::GUEST_EXPORT_FIELDS,
            ),
            || {
                let export_sym = format!("bam.v0.{}.export_fields", M::CANONICAL_NAME);
                M::resolve_symbol(&export_sym).ok_or(FfiResult::UNSUPPORTED)
            },
        ) {
            Ok(ptr) if !ptr.is_null() => ptr,
            Ok(_) => return FfiResult::UNSUPPORTED,
            Err(err) => return err,
        };
        // SAFETY: `M::symbols()` exports `"bam.v0.<M>.export_fields"` with `ExportFieldsFn` ABI.
        let export_fn: ExportFieldsFn = unsafe { core::mem::transmute(export_ptr) };

        let mut builder = PacketBuilder::new();
        // SAFETY: `msg` is a valid `&M` and `builder` stays pinned on the stack across `export_fn`.
        let status = unsafe {
            export_fn(
                core::ptr::from_ref(msg).cast::<c_void>(),
                Some(append_field),
                (&raw mut builder).cast::<c_void>(),
            )
        };
        if !status.is_ok() {
            return status;
        }
        builder.buf.as_mut_slice()[0..4].copy_from_slice(&builder.field_count.to_le_bytes());
        let packet = builder.buf.as_slice();
        // SAFETY: `self.raw` is active, `validate_non_pod_message_info` verified `MESSAGE_WRITE`,
        // and `packet` holds the serialized witness packet of `packet.len()` bytes.
        unsafe { send_fn(self.raw, term_index, packet.as_ptr(), packet.len()) }
    }

    /// Reads the synthesized per-chunk [`AccessMode::HAS`] boolean presence indicator for
    /// `term_index` without re-validating `BamColumnInfo` (caller must have validated `HAS`).
    #[inline]
    fn has_column_raw(&self, term_index: usize) -> Option<bool> {
        let ptr = self.column_readonly_ptr(term_index);
        if ptr.is_null() {
            return None;
        }
        // SAFETY: For `AccessMode::HAS` on an active chunk, `iter_column_readonly` returns a
        // non-null pointer to a 1-byte `FfiBool` (`0` or `1`).
        Some(unsafe { (*ptr.cast::<FfiBool>()).get() })
    }

    /// Reads the synthesized per-chunk [`AccessMode::HAS`] boolean presence indicator for
    /// `term_index` (`Some(true)` if present in the active chunk's archetype, `Some(false)` if
    /// absent, or `None` if `term_index` was not declared with `AccessMode::HAS` or no chunk is active).
    #[inline]
    pub fn has_column(&self, term_index: usize) -> Option<bool> {
        if self.raw.is_null() || self.current_len == 0 {
            return None;
        }
        self.validate_has_untyped_term(term_index, None).ok()?;
        self.has_column_raw(term_index)
    }

    /// Reads up to `out_entities.len()` entity IDs from a `RemovedComponents<T>` stream declared
    /// with [`AccessMode::REMOVED_READ`] at `term_index` via `BamHostApi::iter_read_removed`.
    pub fn read_removed_batch(
        &mut self,
        term_index: usize,
        out_entities: &mut [FfiEntity],
    ) -> Result<usize, FfiResult> {
        if self.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let read_fn =
            host_slot!(self.host.api(), iter_read_removed).ok_or(FfiResult::UNSUPPORTED)?;
        let info = self.column_info(term_index)?;
        if info.mode != AccessMode::REMOVED_READ || info.is_resource() || info.is_message() {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        if out_entities.is_empty() {
            return Ok(0);
        }
        let mut read_count = 0usize;
        // SAFETY: `self.raw` is active, `out_entities` provides `out_entities.len()` writable
        // `FfiEntity` slots, and `read_count` is aligned writable stack storage.
        let status = unsafe {
            read_fn(
                self.raw,
                term_index,
                out_entities.as_mut_ptr(),
                out_entities.len(),
                &mut read_count,
            )
        };
        if status.is_ok() {
            Ok(read_count.min(out_entities.len()))
        } else {
            Err(status)
        }
    }

    /// Drains all unread `RemovedComponents<T>` entity IDs from `term_index` (`REMOVED_READ`).
    pub fn drain_removed(&mut self, term_index: usize) -> Result<Vec<FfiEntity>, FfiResult> {
        const BATCH_SIZE: usize = 64;
        let mut all = Vec::new();
        let mut batch = [FfiEntity::NULL; BATCH_SIZE];
        loop {
            let count = self.read_removed_batch(term_index, &mut batch)?;
            all.extend_from_slice(&batch[..count]);
            if count < BATCH_SIZE {
                break;
            }
        }
        Ok(all)
    }

    /// Validates that `term_index` points to a readable (or writable when `require_write` is true)
    /// non-immutable bridged column matching `expected_uuid`.
    #[inline]
    fn validate_bridged_row_info(
        &self,
        term_index: usize,
        row: usize,
        expected_uuid: ComponentUuid,
        host_size: usize,
        host_align: usize,
        require_mutable: bool,
    ) -> Result<BamColumnInfo, FfiResult> {
        if row >= self.current_len {
            return Err(FfiResult::OUT_OF_RANGE);
        }
        let info = self.column_info(term_index)?;
        if !info.mode.has_entity_column_data()
            || info.is_resource()
            || info.is_message()
            || (require_mutable && (!info.mode.is_mutable() || info.is_immutable()))
            || info.uuid != expected_uuid
            || info.size != host_size
            || info.align != host_align
            || (host_size > 0
                && (!host_align.is_power_of_two() || !host_size.is_multiple_of(host_align)))
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        Ok(info)
    }

    /// Fetches the read-only raw host pointer for row `row_index` of bridged column `term_index`,
    /// verifying column metadata, bounds, and `Optional` presence.
    #[inline]
    fn raw_bridged_row_ptr_ro(
        &mut self,
        term_index: usize,
        row: usize,
        expected_uuid: ComponentUuid,
        host_size: usize,
        host_align: usize,
    ) -> Result<Option<*const c_void>, FfiResult> {
        let info = self.validate_bridged_row_info(
            term_index,
            row,
            expected_uuid,
            host_size,
            host_align,
            false,
        )?;
        if info.is_sparse_set() {
            let rows_fn = self
                .host
                .api()
                .iter_column_rows_readonly
                .ok_or(FfiResult::NULL_POINTER)?;
            // SAFETY: `self.raw` is active at `self.current_len` rows.
            let ptrs = unsafe { rows_fn(self.raw, term_index) };
            if ptrs.is_null() {
                return if info.mode.is_optional() {
                    Ok(None)
                } else {
                    Err(FfiResult::NULL_POINTER)
                };
            }
            if !ptrs.is_aligned() {
                return Err(FfiResult::TYPE_MISMATCH);
            }
            if self
                .current_len
                .checked_mul(core::mem::size_of::<*const u8>())
                .is_none_or(|b| b > isize::MAX as usize)
            {
                return Err(FfiResult::OUT_OF_RANGE);
            }
            // SAFETY: `ptrs` is non-null, aligned, `current_len * size_of::<*const u8>() <= isize::MAX`,
            // and `row < self.current_len`.
            let row_ptr = unsafe { *ptrs.add(row) };
            if row_ptr.is_null() {
                return if info.mode.is_optional() {
                    Ok(None)
                } else {
                    Err(FfiResult::NULL_POINTER)
                };
            }
            if host_size > 0 && !(row_ptr as usize).is_multiple_of(host_align) {
                return Err(FfiResult::TYPE_MISMATCH);
            }
            Ok(Some(row_ptr.cast::<c_void>()))
        } else {
            let base = self.column_readonly_ptr(term_index);
            if base.is_null() {
                return if info.mode.is_optional() {
                    Ok(None)
                } else {
                    Err(FfiResult::NULL_POINTER)
                };
            }
            if host_size == 0 {
                return Ok(Some(base.cast::<c_void>()));
            }
            if !(base as usize).is_multiple_of(host_align) {
                return Err(FfiResult::TYPE_MISMATCH);
            }
            if self
                .current_len
                .checked_mul(host_size)
                .is_none_or(|total| total > isize::MAX as usize)
            {
                return Err(FfiResult::OUT_OF_RANGE);
            }
            let offset = row.checked_mul(host_size).ok_or(FfiResult::OUT_OF_RANGE)?;
            // SAFETY: `row < self.current_len`, `current_len * host_size <= isize::MAX` (so
            // `offset + host_size <= isize::MAX`), and `base` covers the column.
            Ok(Some(unsafe { base.add(offset) }.cast::<c_void>()))
        }
    }

    /// Fetches the writable raw host pointer for row `row_index` of bridged column `term_index`,
    /// verifying mutable access, bounds, and `Optional` presence while stamping host change ticks.
    #[inline]
    fn raw_bridged_row_ptr_mut(
        &mut self,
        term_index: usize,
        row: usize,
        expected_uuid: ComponentUuid,
        host_size: usize,
        host_align: usize,
    ) -> Result<Option<*mut c_void>, FfiResult> {
        let info = self.validate_bridged_row_info(
            term_index,
            row,
            expected_uuid,
            host_size,
            host_align,
            true,
        )?;
        if info.is_sparse_set() {
            let rows_fn = self
                .host
                .api()
                .iter_column_rows_mutable
                .ok_or(FfiResult::NULL_POINTER)?;
            // SAFETY: `self.raw` is active at `self.current_len` rows under `&mut self`.
            let ptrs = unsafe { rows_fn(self.raw, term_index) };
            if ptrs.is_null() {
                return if info.mode.is_optional() {
                    Ok(None)
                } else {
                    Err(FfiResult::NULL_POINTER)
                };
            }
            if !ptrs.is_aligned() {
                return Err(FfiResult::TYPE_MISMATCH);
            }
            if self
                .current_len
                .checked_mul(core::mem::size_of::<*mut u8>())
                .is_none_or(|b| b > isize::MAX as usize)
            {
                return Err(FfiResult::OUT_OF_RANGE);
            }
            // SAFETY: `ptrs` is non-null, aligned, `current_len * size_of::<*mut u8>() <= isize::MAX`,
            // and `row < self.current_len`.
            let row_ptr = unsafe { *ptrs.add(row) };
            if row_ptr.is_null() {
                return if info.mode.is_optional() {
                    Ok(None)
                } else {
                    Err(FfiResult::NULL_POINTER)
                };
            }
            if host_size > 0 && !(row_ptr as usize).is_multiple_of(host_align) {
                return Err(FfiResult::TYPE_MISMATCH);
            }
            Ok(Some(row_ptr.cast::<c_void>()))
        } else {
            let base = self.column_mutable_ptr(term_index);
            if base.is_null() {
                return if info.mode.is_optional() {
                    Ok(None)
                } else {
                    Err(FfiResult::NULL_POINTER)
                };
            }
            if host_size == 0 {
                return Ok(Some(base.cast::<c_void>()));
            }
            if !(base as usize).is_multiple_of(host_align) {
                return Err(FfiResult::TYPE_MISMATCH);
            }
            if self
                .current_len
                .checked_mul(host_size)
                .is_none_or(|total| total > isize::MAX as usize)
            {
                return Err(FfiResult::OUT_OF_RANGE);
            }
            let offset = row.checked_mul(host_size).ok_or(FfiResult::OUT_OF_RANGE)?;
            // SAFETY: `row < self.current_len`, `current_len * host_size <= isize::MAX` (so
            // `offset + host_size <= isize::MAX`), and `base` covers the column.
            Ok(Some(unsafe { base.add(offset) }.cast::<c_void>()))
        }
    }

    /// Reads row `row` of an opaque host-bridged component at `term_index` into wire `W: AbiPod`
    /// (`Ok(None)` when absent on an optional column).
    pub fn read_mapped_wire_row<W: AbiPod>(
        &mut self,
        term_index: usize,
        row: usize,
        vtable: &ValidatedWireVTable<W>,
    ) -> Result<Option<W>, FfiResult> {
        let Some(vt_get) = vtable.raw.get else {
            return Err(FfiResult::UNSUPPORTED);
        };
        let Some(ptr) = self.raw_bridged_row_ptr_ro(
            term_index,
            row,
            vtable.uuid(),
            vtable.host_size(),
            vtable.host_align(),
        )?
        else {
            return Ok(None);
        };
        let mut out = MaybeUninit::<W>::zeroed();
        // SAFETY: `ptr` was layout- and UUID-verified against `vtable`, and `out` is aligned writable `W`.
        let status = unsafe { vt_get(ptr, out.as_mut_ptr()) };
        if status.is_ok() {
            // SAFETY: `vt_get` returned `OK`, initializing `out`.
            Ok(Some(unsafe { out.assume_init() }))
        } else {
            Err(status)
        }
    }

    /// Writes `value` (`W: AbiPod`) into row `row` of an opaque host-bridged mutable component at
    /// `term_index`.
    pub fn write_mapped_wire_row<W: AbiPod>(
        &mut self,
        term_index: usize,
        row: usize,
        vtable: &ValidatedWireVTable<W>,
        value: &W,
    ) -> FfiResult {
        let Some(vt_set) = vtable.raw.set else {
            return FfiResult::PERMISSION_DENIED;
        };
        let ptr_opt = match self.raw_bridged_row_ptr_mut(
            term_index,
            row,
            vtable.uuid(),
            vtable.host_size(),
            vtable.host_align(),
        ) {
            Ok(p) => p,
            Err(err) => return err,
        };
        let Some(ptr) = ptr_opt else {
            return FfiResult::NOT_FOUND;
        };
        // SAFETY: `ptr` is exclusively borrowed and layout/UUID verified against `vtable`.
        unsafe { vt_set(ptr, value) }
    }

    /// Reads an opaque host-bridged singleton resource declared at `term_index` into wire `W: AbiPod`.
    pub fn read_mapped_wire_resource<W: AbiPod>(
        &self,
        term_index: usize,
        vtable: &ValidatedWireVTable<W>,
    ) -> Result<W, FfiResult> {
        let Some(vt_get) = vtable.raw.get else {
            return Err(FfiResult::UNSUPPORTED);
        };
        let info = self.column_info(term_index)?;
        if !info.mode.is_resource()
            || info.uuid != vtable.uuid()
            || info.size != vtable.host_size()
            || info.align != vtable.host_align()
            || (vtable.host_size() > 0
                && (!vtable.host_align().is_power_of_two()
                    || !vtable.host_size().is_multiple_of(vtable.host_align())))
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        let ptr = self.res_ptr(term_index);
        if ptr.is_null() {
            return Err(FfiResult::NOT_FOUND);
        }
        if vtable.host_size() > 0 && !(ptr as usize).is_multiple_of(vtable.host_align()) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        let mut out = MaybeUninit::<W>::zeroed();
        // SAFETY: `ptr` points to the verified host resource for `&self` and `out` is writable `W`.
        let status = unsafe { vt_get(ptr.cast::<c_void>(), out.as_mut_ptr()) };
        if status.is_ok() {
            // SAFETY: `vt_get` returned `OK`, initializing `out`.
            Ok(unsafe { out.assume_init() })
        } else {
            Err(status)
        }
    }

    /// Writes `value` (`W: AbiPod`) into an opaque host-bridged singleton resource declared with
    /// `RESOURCE_WRITE` at `term_index`.
    pub fn write_mapped_wire_resource<W: AbiPod>(
        &mut self,
        term_index: usize,
        vtable: &ValidatedWireVTable<W>,
        value: &W,
    ) -> FfiResult {
        let Some(vt_set) = vtable.raw.set else {
            return FfiResult::PERMISSION_DENIED;
        };
        let info = match self.column_info(term_index) {
            Ok(i) => i,
            Err(err) => return err,
        };
        if info.mode != AccessMode::RESOURCE_WRITE
            || info.is_immutable()
            || info.uuid != vtable.uuid()
            || info.size != vtable.host_size()
            || info.align != vtable.host_align()
            || (vtable.host_size() > 0
                && (!vtable.host_align().is_power_of_two()
                    || !vtable.host_size().is_multiple_of(vtable.host_align())))
        {
            return FfiResult::TYPE_MISMATCH;
        }
        let ptr = self.res_mut_ptr(term_index);
        if ptr.is_null() {
            return FfiResult::NOT_FOUND;
        }
        if vtable.host_size() > 0 && !(ptr as usize).is_multiple_of(vtable.host_align()) {
            return FfiResult::TYPE_MISMATCH;
        }
        // SAFETY: `ptr` is exclusively borrowed under `&mut self` and verified against `vtable`.
        unsafe { vt_set(ptr.cast::<c_void>(), value) }
    }

    /// Reads row `row` of an opaque host-bridged UTF-8 string component (such as `Name`) at
    /// `term_index` into a guest-owned `String` (`Ok(None)` when absent on an optional column).
    pub fn read_mapped_string_row(
        &mut self,
        term_index: usize,
        row: usize,
        vtable: &ValidatedStringVTable,
    ) -> Result<Option<String>, FfiResult> {
        let Some(vt_get) = vtable.raw.get else {
            return Err(FfiResult::UNSUPPORTED);
        };
        let Some(ptr) = self.raw_bridged_row_ptr_ro(
            term_index,
            row,
            vtable.uuid(),
            vtable.host_size(),
            vtable.host_align(),
        )?
        else {
            return Ok(None);
        };
        // SAFETY: `ptr` is valid for the borrow window and `vt_get` returns a borrowed `StrView`.
        let view: StrView = unsafe { vt_get(ptr) };
        // SAFETY: `try_as_str` validates pointer alignment, non-nullness, and UTF-8 before copying.
        let s = unsafe { view.try_as_str() }?;
        Ok(Some(String::from(s)))
    }

    /// Writes `text` into row `row` of an opaque host-bridged mutable UTF-8 string component at
    /// `term_index`.
    pub fn write_mapped_string_row(
        &mut self,
        term_index: usize,
        row: usize,
        vtable: &ValidatedStringVTable,
        text: &str,
    ) -> FfiResult {
        let Some(vt_set) = vtable.raw.set else {
            return FfiResult::PERMISSION_DENIED;
        };
        let ptr_opt = match self.raw_bridged_row_ptr_mut(
            term_index,
            row,
            vtable.uuid(),
            vtable.host_size(),
            vtable.host_align(),
        ) {
            Ok(p) => p,
            Err(err) => return err,
        };
        let Some(ptr) = ptr_opt else {
            return FfiResult::NOT_FOUND;
        };
        // SAFETY: `ptr` is exclusively borrowed and `StrView::from_str(text)` is valid for this call.
        unsafe { vt_set(ptr, StrView::from_str(text)) }
    }
}

/// C-ABI `import_field` callback signature used by [`ImportSinkCtx`] and [`forward_field`].
pub(crate) type WitnessImportFieldFn = unsafe extern "C" fn(
    dst_ptr: *mut c_void,
    field_name: StrView,
    type_tag: StrView,
    payload: FfiSlice<u8>,
) -> FfiResult;

/// Trampoline context forwarding each host-decoded wire field into `M`'s `import_field`.
pub(crate) struct ImportSinkCtx {
    pub(crate) dst_ptr: *mut c_void,
    pub(crate) import_fn: WitnessImportFieldFn,
}

/// C-ABI field-sink callback passed to host wire decoding that invokes `import_fn` on `dst_ptr`.
pub(crate) unsafe extern "C" fn forward_field(
    ctx: *mut c_void,
    field_name: StrView,
    type_tag: StrView,
    payload: FfiSlice<u8>,
) -> FfiResult {
    catch_guest_result(|| {
        if ctx.is_null() || !(ctx as usize).is_multiple_of(core::mem::align_of::<ImportSinkCtx>()) {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `ctx` is non-null, aligned, and points to a stack-pinned `ImportSinkCtx`
        // for the duration of `host_export_fn`.
        let sink_ctx = unsafe { &*ctx.cast::<ImportSinkCtx>() };
        // SAFETY: `sink_ctx.dst_ptr` points to a valid initialized `M`, and `field_name`,
        // `type_tag`, `payload` are valid slices for this synchronous callback.
        unsafe { (sink_ctx.import_fn)(sink_ctx.dst_ptr, field_name, type_tag, payload) }
    })
}

/// Hybrid stack/heap byte buffer for witness field packets, avoiding heap allocation for small
/// messages (up to 256 bytes) before spilling seamlessly into a `Vec<u8>`.
pub(crate) struct PacketBuf {
    stack: [u8; 256],
    stack_len: usize,
    heap: Vec<u8>,
}

impl PacketBuf {
    /// Creates a packet buffer initialized with 4 zeroed header bytes for `field_count`.
    #[inline]
    pub(crate) const fn with_header() -> Self {
        Self {
            stack: [0u8; 256],
            stack_len: 4,
            heap: Vec::new(),
        }
    }

    /// Appends `bytes` to the stack buffer while it fits, spilling into `heap` on overflow.
    #[inline]
    pub(crate) fn extend_from_slice(&mut self, bytes: &[u8]) {
        if self.heap.is_empty() {
            if let Some(new_len) = self.stack_len.checked_add(bytes.len())
                && new_len <= self.stack.len()
            {
                self.stack[self.stack_len..new_len].copy_from_slice(bytes);
                self.stack_len = new_len;
                return;
            }
            self.heap
                .reserve(self.stack_len.saturating_add(bytes.len()));
            self.heap.extend_from_slice(&self.stack[..self.stack_len]);
            self.heap.extend_from_slice(bytes);
        } else {
            self.heap.extend_from_slice(bytes);
        }
    }

    /// Returns the written packet bytes as a contiguous slice.
    #[inline]
    pub(crate) fn as_slice(&self) -> &[u8] {
        if self.heap.is_empty() {
            &self.stack[..self.stack_len]
        } else {
            self.heap.as_slice()
        }
    }

    /// Returns the written packet bytes as a mutable contiguous slice.
    #[inline]
    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        if self.heap.is_empty() {
            &mut self.stack[..self.stack_len]
        } else {
            self.heap.as_mut_slice()
        }
    }

    /// Returns the number of bytes currently written to the packet buffer.
    #[cfg_attr(not(kani), allow(dead_code))]
    #[inline]
    pub(crate) fn len(&self) -> usize {
        if self.heap.is_empty() {
            self.stack_len
        } else {
            self.heap.len()
        }
    }
}

/// Accumulator serializing exported `ModComponent` fields into a length-prefixed wire packet.
pub(crate) struct PacketBuilder {
    pub(crate) buf: PacketBuf,
    pub(crate) field_count: u32,
}

impl PacketBuilder {
    /// Creates a new [`PacketBuilder`] initialized with a 4-byte zeroed `field_count` header.
    #[inline]
    pub(crate) const fn new() -> Self {
        Self {
            buf: PacketBuf::with_header(),
            field_count: 0,
        }
    }
}

/// C-ABI field-sink callback invoked by `M::export_fields` to append one encoded field to `PacketBuilder`.
pub(crate) unsafe extern "C" fn append_field(
    ctx: *mut c_void,
    field_name: StrView,
    type_tag: StrView,
    payload: FfiSlice<u8>,
) -> FfiResult {
    catch_guest_result(|| {
        if ctx.is_null() || !(ctx as usize).is_multiple_of(core::mem::align_of::<PacketBuilder>()) {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `ctx` is non-null, aligned, and points to an active `PacketBuilder` on
        // the stack for this call.
        let builder = unsafe { &mut *ctx.cast::<PacketBuilder>() };
        // SAFETY: `field_name`, `type_tag`, and `payload` come directly from `export_fields`
        // and borrow live slices for this synchronous callback.
        let Ok(name_bytes) = (unsafe { field_name.try_as_slice() }) else {
            return FfiResult::NULL_POINTER;
        };
        // SAFETY: Same as above for `type_tag`.
        let Ok(tag_bytes) = (unsafe { type_tag.try_as_slice() }) else {
            return FfiResult::NULL_POINTER;
        };
        // SAFETY: Same as above for `payload`.
        let Ok(val_bytes) = (unsafe { payload.try_as_slice() }) else {
            return FfiResult::NULL_POINTER;
        };
        let Ok(name_len) = u32::try_from(name_bytes.len()) else {
            return FfiResult::CAPACITY_EXCEEDED;
        };
        let Ok(tag_len) = u32::try_from(tag_bytes.len()) else {
            return FfiResult::CAPACITY_EXCEEDED;
        };
        let Ok(val_len) = u32::try_from(val_bytes.len()) else {
            return FfiResult::CAPACITY_EXCEEDED;
        };
        let Some(next_count) = builder.field_count.checked_add(1) else {
            return FfiResult::CAPACITY_EXCEEDED;
        };
        builder.field_count = next_count;
        builder.buf.extend_from_slice(&name_len.to_le_bytes());
        builder.buf.extend_from_slice(name_bytes);
        builder.buf.extend_from_slice(&tag_len.to_le_bytes());
        builder.buf.extend_from_slice(tag_bytes);
        builder.buf.extend_from_slice(&val_len.to_le_bytes());
        builder.buf.extend_from_slice(val_bytes);
        FfiResult::OK
    })
}

/// Context passed through `user_data` to [`par_chunk_task_trampoline`] during parallel chunk dispatch.
struct ParChunkTaskContext<'h, F> {
    host: &'h Host,
    frozen_cache: Option<&'h FrozenSlotCache>,
    func: F,
}

/// Panic-isolated C-ABI trampoline invoked by the host `ComputeTaskPool` for each disjoint sub-chunk
/// iterator during `par_for_each_chunk` and `par_for_each_chunk_readonly`.
unsafe extern "C" fn par_chunk_task_trampoline<F>(
    sub_iter: *mut BamIter,
    user_data: *mut c_void,
) -> FfiResult
where
    F: Fn(&mut ValidatedIter<'_>) -> FfiResult + Sync,
{
    catch_guest_result(|| {
        if sub_iter.is_null() || user_data.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if !(user_data as usize).is_multiple_of(core::mem::align_of::<ParChunkTaskContext<'_, F>>())
        {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `user_data` is non-null, aligned, and points to `ParChunkTaskContext<'_, F>` live
        // on the caller's stack throughout `par_for_each_chunk`, and `F: Sync` allows concurrent shared reads.
        let ctx = unsafe { &*user_data.cast::<ParChunkTaskContext<'_, F>>() };
        // SAFETY: `sub_iter` is an isolated sub-range `BamIter` valid on this worker thread for
        // the duration of this task callback, and `ctx.frozen_cache` outlives this call.
        let mut task_iter = unsafe {
            ValidatedIter::from_raw_with_frozen_cache(sub_iter, ctx.host, ctx.frozen_cache)
        };
        while task_iter.next_chunk().is_some() {
            let status = (ctx.func)(&mut task_iter);
            if !status.is_ok() {
                return status;
            }
        }
        FfiResult::OK
    })
}

/// Validates that `info` describes a `MESSAGE_READ` stream matching `M: AbiPod` (size, alignment,
/// and optional UUID filter).
#[inline]
fn validate_message_read_info<M: AbiPod>(
    info: &BamColumnInfo,
    expected_uuid: Option<UuidFilter>,
) -> bool {
    if !matches!(
        info.mode,
        AccessMode::MESSAGE_READ | AccessMode::MESSAGE_WRITE
    ) || !info.is_message()
        || info.is_resource()
    {
        return false;
    }
    if info.size != core::mem::size_of::<M>() || info.align != core::mem::align_of::<M>() {
        return false;
    }
    if let Some(filter) = expected_uuid
        && !filter.matches(info.uuid)
    {
        return false;
    }
    true
}

/// Validates that `info` describes a `MESSAGE_WRITE` stream matching `M: AbiPod` (size, alignment,
/// and optional UUID filter).
#[inline]
fn validate_message_write_info<M: AbiPod>(
    info: &BamColumnInfo,
    expected_uuid: Option<UuidFilter>,
) -> bool {
    if info.mode != AccessMode::MESSAGE_WRITE
        || info.is_immutable()
        || !info.is_message()
        || info.is_resource()
    {
        return false;
    }
    if info.size != core::mem::size_of::<M>() || info.align != core::mem::align_of::<M>() {
        return false;
    }
    if let Some(filter) = expected_uuid
        && !filter.matches(info.uuid)
    {
        return false;
    }
    true
}

/// Validates that `info` describes a non-POD message stream matching `expected_access`, non-zero
/// power-of-two host alignment, and optional `expected_uuid`.
#[inline]
fn validate_non_pod_message_info(
    info: &BamColumnInfo,
    expected_uuid: Option<UuidFilter>,
    writable: bool,
) -> bool {
    if writable {
        if info.mode != AccessMode::MESSAGE_WRITE || info.is_immutable() {
            return false;
        }
    } else if !matches!(
        info.mode,
        AccessMode::MESSAGE_READ | AccessMode::MESSAGE_WRITE
    ) {
        return false;
    }
    if !info.is_message() || info.is_resource() {
        return false;
    }
    if info.size != core::mem::size_of::<*const c_void>()
        || info.align != core::mem::align_of::<*const c_void>()
    {
        return false;
    }
    if let Some(filter) = expected_uuid
        && !filter.matches(info.uuid)
    {
        return false;
    }
    true
}

/// Computes `M`'s manifest-qualified [`UuidFilter`] and validates `info` as a non-POD message
/// stream for `expected_access`.
#[inline]
fn validate_non_pod_message_info_for<M: ModComponent>(
    info: &BamColumnInfo,
    manifest_name: &str,
    expected_uuid: Option<UuidFilter>,
    writable: bool,
) -> bool {
    let filter = expected_uuid.unwrap_or_else(|| UuidFilter::exact(M::UUID));
    if !validate_non_pod_message_info(info, Some(filter), writable) {
        return false;
    }
    if info.uuid == M::UUID {
        return true;
    }
    qualify_mod_component_identity::<M>(manifest_name)
        .is_ok_and(|(_, qualified_uuid)| info.uuid == qualified_uuid)
}

/// Record of a single column borrow dispensed from [`ChunkColumnDispenser`], used to enforce
/// Rust's aliasing rules (no overlapping `&mut` with any other borrow of the same component or
/// address range) within a chunk.
#[derive(Clone, Copy)]
struct DispensedBorrow {
    component: BamComponent,
    start: usize,
    end: usize,
    is_mut: bool,
}

/// Sequential per-chunk column dispenser verifying disjoint borrows across up to
/// [`MAX_TERMS_PER_QUERY`] tuple terms so safe `bamabi_guest` tuple queries require
/// zero `unsafe` code.
pub struct ChunkColumnDispenser<'chunk, 'ctx> {
    iter: &'chunk mut ValidatedIter<'ctx>,
    next_term_index: usize,
    borrowed: [MaybeUninit<DispensedBorrow>; MAX_TERMS_PER_QUERY],
    borrowed_count: usize,
    entities_range: Option<(usize, usize)>,
    batch_ptrs: *const MaybeUninit<*mut u8>,
    batch_col_count: usize,
}

impl<'chunk, 'ctx> ChunkColumnDispenser<'chunk, 'ctx> {
    /// Creates a column dispenser over the currently active chunk of `iter`.
    #[inline]
    pub fn new(iter: &'chunk mut ValidatedIter<'ctx>) -> Self {
        let (batch_ptrs, batch_col_count) = match iter.slot_cache.and_then(SlotCacheRef::as_mutable)
        {
            Some(scratch)
                if iter.iter_id != 0
                    && iter.batch_epoch != 0
                    && iter.scratch_batch_epoch == iter.batch_epoch =>
            {
                let count = (scratch.batch_col_count.get() as usize).min(INLINE_BATCH_COL_CAP);
                if count > 0 {
                    // SAFETY: `scratch.batch_col_ptrs` holds `INLINE_BATCH_COL_CAP` elements and
                    // `count` leading entries were initialized by `advance_batch` for the exact
                    // active `(iter_id, batch_epoch)`. `iter` is borrowed `&'chunk mut`, preventing
                    // mutation while `self` is alive.
                    (unsafe { (*scratch.batch_col_ptrs.get()).0.as_ptr() }, count)
                } else {
                    (core::ptr::null(), 0)
                }
            }
            _ => (core::ptr::null(), 0),
        };
        Self {
            iter,
            next_term_index: 0,
            borrowed: [const { MaybeUninit::uninit() }; MAX_TERMS_PER_QUERY],
            borrowed_count: 0,
            entities_range: None,
            batch_ptrs,
            batch_col_count,
        }
    }

    /// Creates a column dispenser over the currently active chunk of `iter` backed by an explicit
    /// caller-owned [`BatchColCache`] populated via [`ValidatedIter::advance_batch_into`].
    ///
    /// If `cache` does not match `iter`'s current `(iter_id, batch_epoch)` (e.g. from an earlier
    /// chunk, another iterator, or an unpopulated cache), falls back cleanly to legacy per-column
    /// FFI lookups (`batch_ptrs = null`, `batch_col_count = 0`).
    #[inline]
    pub fn new_with_batch(
        iter: &'chunk mut ValidatedIter<'ctx>,
        cache: &'chunk BatchColCache<'ctx>,
    ) -> Self {
        let valid = iter.iter_id != 0
            && iter.batch_epoch != 0
            && cache.iter_id == iter.iter_id
            && cache.batch_epoch == u64::from(iter.batch_epoch);
        let count = if valid {
            (cache.len as usize).min(INLINE_BATCH_COL_CAP)
        } else {
            0
        };
        let batch_ptrs = if count > 0 {
            cache.ptrs.as_ptr()
        } else {
            core::ptr::null()
        };
        Self {
            iter,
            next_term_index: 0,
            borrowed: [const { MaybeUninit::uninit() }; MAX_TERMS_PER_QUERY],
            borrowed_count: 0,
            entities_range: None,
            batch_ptrs,
            batch_col_count: count,
        }
    }

    /// Reads cached slot-68 column pointer `term_index` if `term_index < self.batch_col_count`.
    #[inline]
    fn batch_col_ptr(&self, term_index: usize) -> Option<*mut u8> {
        if term_index < self.batch_col_count && !self.batch_ptrs.is_null() {
            // SAFETY: `self.batch_ptrs` points to at least `self.batch_col_count` initialized
            // `MaybeUninit<*mut u8>` elements populated by `advance_batch` / `advance_batch_into`,
            // and `self.batch_col_count <= INLINE_BATCH_COL_CAP`.
            Some(unsafe { (*self.batch_ptrs.add(term_index)).assume_init() })
        } else {
            None
        }
    }

    /// Returns the init-time base [`Host`] table without invoking `BamHostApi::sys_host_ctx`.
    #[inline]
    pub const fn base_host(&self) -> &'ctx Host {
        self.iter.base_host()
    }

    /// Returns the active [`Host`] table for this iterator.
    #[inline]
    pub fn host(&self) -> &'ctx Host {
        self.iter.host()
    }

    /// Returns the number of entity rows in the active chunk.
    #[inline]
    pub const fn len(&self) -> usize {
        self.iter.current_len()
    }

    /// Returns `true` if `len() == 0`.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the next term index that will be consumed by the next `next_*` dispenser method.
    #[inline]
    pub const fn next_term_index(&self) -> usize {
        self.next_term_index
    }

    /// Retrieves per-row `added` and `changed` tick slices (`&'chunk [u32]`) alongside the
    /// enclosing system's `(last_run, this_run)` tick window for dense column `term_index` in the
    /// active chunk via `BamHostApi::iter_column_ticks` (slot 69).
    ///
    /// Rejects hostile tick pointers that overlap any previously dispensed mutable column borrow
    /// with [`FfiResult::ACCESS_CONFLICT`].
    #[inline]
    pub fn column_ticks(
        &self,
        term_index: usize,
    ) -> Result<(&'chunk [u32], &'chunk [u32], u32, u32), FfiResult> {
        if self.iter.raw.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        let ticks_fn =
            host_slot!(self.iter.host.api(), iter_column_ticks).ok_or(FfiResult::UNSUPPORTED)?;
        let mut added_ptr: *const u32 = core::ptr::null();
        let mut changed_ptr: *const u32 = core::ptr::null();
        let mut last_run: u32 = 0;
        let mut this_run: u32 = 0;
        // SAFETY: `self.iter.raw` is a valid active `BamIter` for `'chunk` and all four
        // out-pointers point to aligned writable stack storage.
        let status = unsafe {
            ticks_fn(
                self.iter.raw.cast_const(),
                term_index,
                &mut added_ptr,
                &mut changed_ptr,
                &mut last_run,
                &mut this_run,
            )
        };
        if !status.is_ok() {
            return Err(status);
        }
        let len = self.iter.current_len();
        if len == 0 {
            return Ok((&[], &[], last_run, this_run));
        }
        if added_ptr.is_null() || changed_ptr.is_null() {
            return Err(FfiResult::NULL_POINTER);
        }
        if !(added_ptr as usize).is_multiple_of(core::mem::align_of::<u32>())
            || !(changed_ptr as usize).is_multiple_of(core::mem::align_of::<u32>())
        {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        let byte_len = len
            .checked_mul(core::mem::size_of::<u32>())
            .ok_or(FfiResult::OUT_OF_RANGE)?;
        if byte_len > isize::MAX as usize {
            return Err(FfiResult::OUT_OF_RANGE);
        }
        let added_start = added_ptr.addr();
        let added_end = added_start
            .checked_add(byte_len)
            .ok_or(FfiResult::OUT_OF_RANGE)?;
        let changed_start = changed_ptr.addr();
        let changed_end = changed_start
            .checked_add(byte_len)
            .ok_or(FfiResult::OUT_OF_RANGE)?;
        for slot in &self.borrowed[..self.borrowed_count] {
            // SAFETY: Entries `0..self.borrowed_count` are initialized by `check_and_record_borrow`.
            let prev = unsafe { slot.assume_init_ref() };
            if prev.is_mut
                && prev.end > prev.start
                && ((added_start < prev.end && prev.start < added_end)
                    || (changed_start < prev.end && prev.start < changed_end))
            {
                return Err(FfiResult::ACCESS_CONFLICT);
            }
        }
        // SAFETY: Host reported `OK`, both tick pointers are non-null, aligned for `u32`,
        // `byte_len <= isize::MAX`, disjoint from active mutable column borrows, and valid for
        // shared reads throughout `'chunk`.
        let added = unsafe { core::slice::from_raw_parts(added_ptr, len) };
        // SAFETY: Same invariants as `added` above.
        let changed = unsafe { core::slice::from_raw_parts(changed_ptr, len) };
        Ok((added, changed, last_run, this_run))
    }

    #[cfg(kani)]
    pub(crate) const fn borrowed_count(&self) -> usize {
        self.borrowed_count
    }

    #[cfg(kani)]
    pub(crate) fn set_entities_range(&mut self, range: Option<(usize, usize)>) {
        self.entities_range = range;
    }

    /// Verifies that borrowing `(component, start..end, mutable)` does not conflict with any
    /// previously dispensed borrow in this chunk (checking both component identity and raw byte
    /// address overlap), recording the borrow on success or returning `ACCESS_CONFLICT`.
    #[inline]
    pub(crate) fn check_and_record_borrow(
        &mut self,
        component: BamComponent,
        start: usize,
        byte_len: usize,
        is_mut: bool,
    ) -> Result<(), FfiResult> {
        let end = start.checked_add(byte_len).ok_or(FfiResult::OUT_OF_RANGE)?;
        if is_mut
            && byte_len > 0
            && let Some((ent_start, ent_end)) = self.entities_range
            && ent_end > ent_start
            && start < ent_end
            && ent_start < end
        {
            return Err(FfiResult::ACCESS_CONFLICT);
        }
        for slot in &self.borrowed[..self.borrowed_count] {
            // SAFETY: Entries `0..self.borrowed_count` are initialized by `check_and_record_borrow`.
            let prev = unsafe { slot.assume_init_ref() };
            if is_mut || prev.is_mut {
                if prev.component == component {
                    return Err(FfiResult::ACCESS_CONFLICT);
                }
                let prev_non_empty = prev.end > prev.start;
                if byte_len > 0 && prev_non_empty && start < prev.end && prev.start < end {
                    return Err(FfiResult::ACCESS_CONFLICT);
                }
            }
        }
        if self.borrowed_count >= MAX_TERMS_PER_QUERY {
            return Err(FfiResult::CAPACITY_EXCEEDED);
        }
        self.borrowed[self.borrowed_count].write(DispensedBorrow {
            component,
            start,
            end,
            is_mut,
        });
        self.borrowed_count += 1;
        Ok(())
    }

    /// Dispenses the contiguous `[FfiEntity; len]` slice for the active chunk without consuming a
    /// `BamTermDesc` index.
    #[inline]
    pub fn entities(&mut self) -> Result<&'chunk [FfiEntity], FfiResult> {
        let len = self.iter.current_len();
        if len == 0 {
            return Ok(&[]);
        }
        let ptr = if !self.iter.batch_entities.is_null() {
            self.iter.batch_entities
        } else {
            let ent_fn = self
                .iter
                .host
                .api()
                .iter_entities
                .ok_or(FfiResult::NULL_POINTER)?;
            // SAFETY: `self.iter.raw` is active at a chunk of `len` entities for `'chunk`.
            unsafe { ent_fn(self.iter.raw.cast_const()) }
        };
        if ptr.is_null() || !(ptr as usize).is_multiple_of(core::mem::align_of::<FfiEntity>()) {
            return Err(FfiResult::NULL_POINTER);
        }
        let byte_len = len
            .checked_mul(core::mem::size_of::<FfiEntity>())
            .ok_or(FfiResult::OUT_OF_RANGE)?;
        if byte_len > isize::MAX as usize {
            return Err(FfiResult::OUT_OF_RANGE);
        }
        let start = ptr.addr();
        let end = start.checked_add(byte_len).ok_or(FfiResult::OUT_OF_RANGE)?;
        if let Some(prev) = self.entities_range
            && prev != (start, end)
        {
            return Err(FfiResult::ACCESS_CONFLICT);
        }
        for slot in &self.borrowed[..self.borrowed_count] {
            // SAFETY: Entries `0..self.borrowed_count` are initialized by `check_and_record_borrow`.
            let prev = unsafe { slot.assume_init_ref() };
            if prev.is_mut && prev.end > prev.start && start < prev.end && prev.start < end {
                return Err(FfiResult::ACCESS_CONFLICT);
            }
        }
        self.entities_range = Some((start, end));
        // SAFETY: `ptr` is non-null, aligned, `byte_len <= isize::MAX`, disjoint from all mutable
        // column borrows, and valid for `len` `FfiEntity` elements throughout `'chunk`.
        Ok(unsafe { core::slice::from_raw_parts(ptr, len) })
    }

    /// Validates pointer alignment/bounds, records a shared borrow in `self`, and dispenses
    /// `&'chunk [T]` for a dense column without repeating bounds/alignment checks.
    ///
    /// # Safety
    /// `ptr` must be valid for `len` initialized `T` elements for lifetime `'chunk`.
    #[inline]
    unsafe fn dispense_dense_read<T: AbiPod>(
        &mut self,
        component: BamComponent,
        ptr: *const u8,
        len: usize,
    ) -> Result<&'chunk [T], FfiResult> {
        let elem_size = core::mem::size_of::<T>();
        if len == 0 || elem_size == 0 {
            if ptr.is_null() && (elem_size > 0 || len > 0) {
                return Err(FfiResult::TYPE_MISMATCH);
            }
            self.check_and_record_borrow(component, 0, 0, false)?;
            // SAFETY: `len == 0` or ZST `T: AbiPod`, and `ptr` is non-null when `len > 0`.
            return Ok(unsafe { raw_dense_slice_unchecked::<T>(ptr, len) });
        }
        let byte_len = len.checked_mul(elem_size).ok_or(FfiResult::OUT_OF_RANGE)?;
        if byte_len > isize::MAX as usize {
            return Err(FfiResult::OUT_OF_RANGE);
        }
        if ptr.is_null() || !(ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        // Check and record disjointness BEFORE forming `&'chunk [T]` so an overlapping hostile
        // pointer can never alias an already-dispensed `&'chunk mut [...]` under Tree Borrows.
        self.check_and_record_borrow(component, ptr.addr(), byte_len, false)?;
        // SAFETY: Verified non-null, aligned, `byte_len <= isize::MAX`, and strict disjointness
        // from all active mutable borrows for `'chunk`.
        Ok(unsafe { raw_dense_slice_unchecked::<T>(ptr, len) })
    }

    /// Validates pointer alignment/bounds, records an exclusive borrow in `self`, and dispenses
    /// `&'chunk mut [T]` for a writable dense column without repeating bounds/alignment checks.
    ///
    /// # Safety
    /// `ptr` must be exclusively valid for `len` initialized `T` elements for lifetime `'chunk`.
    #[inline]
    unsafe fn dispense_dense_write<T: AbiPod>(
        &mut self,
        component: BamComponent,
        ptr: *mut u8,
        len: usize,
    ) -> Result<&'chunk mut [T], FfiResult> {
        let elem_size = core::mem::size_of::<T>();
        if len == 0 || elem_size == 0 {
            if ptr.is_null() && (elem_size > 0 || len > 0) {
                return Err(FfiResult::TYPE_MISMATCH);
            }
            self.check_and_record_borrow(component, 0, 0, true)?;
            // SAFETY: `len == 0` or ZST `T: AbiPod`; `check_and_record_borrow` verified unique component.
            return Ok(unsafe { raw_dense_slice_mut_unchecked::<T>(ptr, len) });
        }
        let byte_len = len.checked_mul(elem_size).ok_or(FfiResult::OUT_OF_RANGE)?;
        if byte_len > isize::MAX as usize {
            return Err(FfiResult::OUT_OF_RANGE);
        }
        if ptr.is_null() || !(ptr as usize).is_multiple_of(core::mem::align_of::<T>()) {
            return Err(FfiResult::TYPE_MISMATCH);
        }
        self.check_and_record_borrow(component, ptr.addr(), byte_len, true)?;
        // SAFETY: Verified non-null, aligned, `byte_len <= isize::MAX`, and strict disjointness for `'chunk`.
        Ok(unsafe { raw_dense_slice_mut_unchecked::<T>(ptr, len) })
    }

    /// Dispenses the next dense read-only `&'chunk [T]` column (`READ`, `CHANGED`, or `ADDED`).
    #[inline]
    pub fn next_read<T: AbiPod>(
        &mut self,
        expected_mode: AccessMode,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<&'chunk [T], FfiResult> {
        let term_index = self.next_term_index;
        self.next_term_index += 1;
        let component =
            self.iter
                .validate_dense_read_term::<T>(term_index, expected_mode, expected_uuid)?;
        let len = self.iter.current_len();
        let ptr = if len == 0 || core::mem::size_of::<T>() == 0 {
            core::ptr::NonNull::<u8>::dangling().as_ptr().cast_const()
        } else if let Some(batch_ptr) = self.batch_col_ptr(term_index) {
            batch_ptr.cast_const()
        } else {
            let col_fn = self
                .iter
                .host
                .api()
                .iter_column_readonly
                .ok_or(FfiResult::NULL_POINTER)?;
            // SAFETY: `self.iter.raw` is active at a chunk of `len` rows for `'chunk`.
            unsafe { col_fn(self.iter.raw.cast_const(), term_index) }
        };
        // SAFETY: `validate_dense_read_term` verified `T`'s layout, mode, and UUID for `'chunk`.
        unsafe { self.dispense_dense_read::<T>(component, ptr, len) }
    }

    /// Dispenses the next dense mutable `&'chunk mut [T]` column (`WRITE`).
    #[inline]
    pub fn next_write<T: AbiPod>(
        &mut self,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<&'chunk mut [T], FfiResult> {
        let term_index = self.next_term_index;
        self.next_term_index += 1;
        let component = self
            .iter
            .validate_dense_write_term::<T>(term_index, expected_uuid)?;
        let len = self.iter.current_len();
        let ptr = if len == 0 || core::mem::size_of::<T>() == 0 {
            if len > 0 && term_index >= self.batch_col_count {
                if let Some(col_fn) = self.iter.host.api().iter_column_mutable {
                    // Mark ZST change ticks via host `iter_column_mutable` when not already marked by
                    // `iter_next_chunk_batch`.
                    // SAFETY: `self.iter.raw` is active at a valid chunk.
                    let _ = unsafe { col_fn(self.iter.raw, term_index) };
                }
            }
            core::ptr::NonNull::<u8>::dangling().as_ptr()
        } else if let Some(batch_ptr) = self.batch_col_ptr(term_index) {
            batch_ptr
        } else {
            let col_fn = self
                .iter
                .host
                .api()
                .iter_column_mutable
                .ok_or(FfiResult::NULL_POINTER)?;
            // SAFETY: `self.iter.raw` is active at a chunk of `len` rows for `'chunk`.
            unsafe { col_fn(self.iter.raw, term_index) }
        };
        // SAFETY: `validate_dense_write_term` verified `WRITE`, non-immutable, layout, and UUID.
        unsafe { self.dispense_dense_write::<T>(component, ptr, len) }
    }

    /// Dispenses the next optional dense read-only `Option<&'chunk [T]>` column (`OPTIONAL_READ`).
    #[inline]
    pub fn next_optional_read<T: AbiPod>(
        &mut self,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<Option<&'chunk [T]>, FfiResult> {
        let term_index = self.next_term_index;
        self.next_term_index += 1;
        let component = self
            .iter
            .validate_optional_read_term::<T>(term_index, expected_uuid)?;
        let len = self.iter.current_len();
        let ptr = if let Some(batch_ptr) = self.batch_col_ptr(term_index) {
            batch_ptr.cast_const()
        } else {
            let col_fn = self
                .iter
                .host
                .api()
                .iter_column_readonly
                .ok_or(FfiResult::NULL_POINTER)?;
            // SAFETY: `self.iter.raw` is active at a chunk of `len` rows for `'chunk`.
            unsafe { col_fn(self.iter.raw.cast_const(), term_index) }
        };
        if ptr.is_null() {
            return Ok(None);
        }
        // SAFETY: Present optional dense column verified for layout, UUID, alignment, and non-aliasing.
        unsafe { self.dispense_dense_read::<T>(component, ptr, len) }.map(Some)
    }

    /// Dispenses the next optional dense mutable `Option<&'chunk mut [T]>` column (`OPTIONAL_WRITE`).
    #[inline]
    pub fn next_optional_write<T: AbiPod>(
        &mut self,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<Option<&'chunk mut [T]>, FfiResult> {
        let term_index = self.next_term_index;
        self.next_term_index += 1;
        let component = self
            .iter
            .validate_optional_write_term::<T>(term_index, expected_uuid)?;
        let len = self.iter.current_len();
        let ptr = if let Some(batch_ptr) = self.batch_col_ptr(term_index) {
            batch_ptr
        } else {
            let col_fn = self
                .iter
                .host
                .api()
                .iter_column_mutable
                .ok_or(FfiResult::NULL_POINTER)?;
            // SAFETY: `self.iter.raw` is active at a chunk of `len` rows for `'chunk`.
            unsafe { col_fn(self.iter.raw, term_index) }
        };
        if ptr.is_null() {
            return Ok(None);
        }
        // SAFETY: Present optional dense mutable column verified for layout, UUID, alignment, and disjointness.
        unsafe { self.dispense_dense_write::<T>(component, ptr, len) }.map(Some)
    }

    /// Validates and advances past a pure filter term (`FILTER_ONLY` / `WITHOUT` / `ALLOW`).
    #[inline]
    pub fn next_filter<T: AbiPod>(
        &mut self,
        expected_mode: AccessMode,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<(), FfiResult> {
        let term_index = self.next_term_index;
        self.next_term_index += 1;
        self.iter
            .validate_filter_term::<T>(term_index, expected_mode, expected_uuid)
    }

    /// Validates and advances past an untyped/bridged filter term (`FILTER_ONLY` / `WITHOUT` / `ALLOW`)
    /// without comparing `T`'s guest layout against opaque host `size`/`align`.
    #[inline]
    pub fn next_filter_untyped(
        &mut self,
        expected_mode: AccessMode,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<(), FfiResult> {
        let term_index = self.next_term_index;
        self.next_term_index += 1;
        self.iter
            .validate_filter_untyped_term(term_index, expected_mode, expected_uuid)
    }

    /// Dispenses the next `AccessMode::HAS` presence indicator for `T: AbiPod`.
    #[inline]
    pub fn next_has<T: AbiPod>(
        &mut self,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<bool, FfiResult> {
        let term_index = self.next_term_index;
        self.next_term_index += 1;
        self.iter
            .validate_has_term::<T>(term_index, expected_uuid)?;
        if self.iter.raw.is_null() || self.iter.current_len() == 0 {
            return Err(FfiResult::NULL_POINTER);
        }
        if let Some(batch_ptr) = self.batch_col_ptr(term_index) {
            let ptr = batch_ptr.cast_const();
            if ptr.is_null() {
                return Err(FfiResult::NULL_POINTER);
            }
            // SAFETY: For `AccessMode::HAS` on an active chunk, `iter_next_chunk_batch` writes a
            // non-null pointer to a 1-byte `FfiBool` (`0` or `1`).
            return Ok(unsafe { (*ptr.cast::<FfiBool>()).get() });
        }
        self.iter
            .has_column_raw(term_index)
            .ok_or(FfiResult::NULL_POINTER)
    }

    /// Dispenses the next `AccessMode::HAS` presence indicator for an opaque/bridged component.
    #[inline]
    pub fn next_has_untyped(
        &mut self,
        expected_uuid: Option<UuidFilter>,
    ) -> Result<bool, FfiResult> {
        let term_index = self.next_term_index;
        self.next_term_index += 1;
        self.iter
            .validate_has_untyped_term(term_index, expected_uuid)?;
        if self.iter.raw.is_null() || self.iter.current_len() == 0 {
            return Err(FfiResult::NULL_POINTER);
        }
        if let Some(batch_ptr) = self.batch_col_ptr(term_index) {
            let ptr = batch_ptr.cast_const();
            if ptr.is_null() {
                return Err(FfiResult::NULL_POINTER);
            }
            // SAFETY: For `AccessMode::HAS` on an active chunk, `iter_next_chunk_batch` writes a
            // non-null pointer to a 1-byte `FfiBool` (`0` or `1`).
            return Ok(unsafe { (*ptr.cast::<FfiBool>()).get() });
        }
        self.iter
            .has_column_raw(term_index)
            .ok_or(FfiResult::NULL_POINTER)
    }
}

#[cfg(all(test, not(kani)))]
mod tests {
    use core::ffi::{c_char, c_void};
    use core::mem::MaybeUninit;

    use bamabi_sys::{
        ABI_VERSION, BamCommands, BamHostApi, BamHostCtx, BamLoadCtx, BamSystemCtx, BamSystemDesc,
        ComponentDesc,
    };

    use super::*;

    struct EpochTestIter {
        col_ro: [u32; 2],
        col_rw: [u32; 2],
        legacy_ro_calls: usize,
        legacy_rw_calls: usize,
    }

    unsafe extern "C" fn stub_get_proc(
        _ctx: *const BamHostCtx,
        _name: *const c_char,
    ) -> *const c_void {
        core::ptr::null()
    }

    unsafe extern "C" fn stub_last_error(
        _ctx: *const BamHostCtx,
        _buf: *mut c_char,
        _cap: usize,
    ) -> usize {
        0
    }

    unsafe extern "C" fn stub_load_host_ctx(load_ctx: *const BamLoadCtx) -> *const BamHostCtx {
        load_ctx.cast()
    }

    unsafe extern "C" fn stub_register_comp(
        _ctx: *mut BamLoadCtx,
        _desc: *const ComponentDesc,
        _out: *mut BamComponent,
    ) -> FfiResult {
        FfiResult::OK
    }

    unsafe extern "C" fn stub_resolve_comp(
        _ctx: *const BamHostCtx,
        _name: *const c_char,
        _uuid: *const ComponentUuid,
        _out: *mut BamComponent,
    ) -> FfiResult {
        FfiResult::OK
    }

    unsafe extern "C" fn stub_register_sys(
        _ctx: *mut BamLoadCtx,
        _desc: *const BamSystemDesc,
    ) -> FfiResult {
        FfiResult::OK
    }

    unsafe extern "C" fn stub_sys_host_ctx(sys_ctx: *const BamSystemCtx) -> *const BamHostCtx {
        sys_ctx.cast()
    }

    unsafe extern "C" fn stub_sys_query(
        _sys_ctx: *mut BamSystemCtx,
        _idx: usize,
        out_iter: *mut *mut BamIter,
    ) -> FfiResult {
        // SAFETY: Test-only null initializer when `out_iter` is non-null.
        if !out_iter.is_null() {
            // SAFETY: Caller passed writable `*mut *mut BamIter`.
            unsafe { *out_iter = core::ptr::null_mut() };
        }
        FfiResult::OK
    }

    unsafe extern "C" fn stub_sys_commands(
        _sys_ctx: *mut BamSystemCtx,
        out_cmds: *mut *mut BamCommands,
    ) -> FfiResult {
        if !out_cmds.is_null() {
            // SAFETY: Caller passed writable `*mut *mut BamCommands`.
            unsafe { *out_cmds = core::ptr::null_mut() };
        }
        FfiResult::OK
    }

    unsafe extern "C" fn stub_iter_entities(_iter: *const BamIter) -> *const FfiEntity {
        core::ptr::null()
    }

    unsafe extern "C" fn stub_iter_mark_changed(
        _iter: *mut BamIter,
        _idx: usize,
        _start: usize,
        _count: usize,
    ) -> FfiResult {
        FfiResult::OK
    }

    unsafe extern "C" fn stub_iter_rows_ro(_iter: *mut BamIter, _idx: usize) -> *const *const u8 {
        core::ptr::null()
    }

    unsafe extern "C" fn stub_iter_rows_mut(_iter: *mut BamIter, _idx: usize) -> *const *mut u8 {
        core::ptr::null()
    }

    unsafe extern "C" fn stub_res_ptr(_iter: *const BamIter, _idx: usize) -> *const u8 {
        core::ptr::null()
    }

    unsafe extern "C" fn stub_res_mut_ptr(_iter: *mut BamIter, _idx: usize) -> *mut u8 {
        core::ptr::null_mut()
    }

    unsafe extern "C" fn stub_cmd_spawn_empty(
        _cmds: *mut BamCommands,
        _out: *mut FfiEntity,
    ) -> FfiResult {
        FfiResult::OK
    }

    unsafe extern "C" fn stub_cmd_spawn(
        _cmds: *mut BamCommands,
        _comps: *const BamComponent,
        _ptrs: *const *const u8,
        _count: usize,
        _out: *mut FfiEntity,
    ) -> FfiResult {
        FfiResult::OK
    }

    unsafe extern "C" fn stub_cmd_despawn(_cmds: *mut BamCommands, _e: FfiEntity) -> FfiResult {
        FfiResult::OK
    }

    unsafe extern "C" fn stub_cmd_insert(
        _cmds: *mut BamCommands,
        _e: FfiEntity,
        _c: BamComponent,
        _ptr: *const u8,
        _len: usize,
    ) -> FfiResult {
        FfiResult::OK
    }

    unsafe extern "C" fn stub_cmd_insert_default(
        _cmds: *mut BamCommands,
        _e: FfiEntity,
        _c: BamComponent,
    ) -> FfiResult {
        FfiResult::OK
    }

    unsafe extern "C" fn stub_cmd_remove(
        _cmds: *mut BamCommands,
        _e: FfiEntity,
        _c: BamComponent,
    ) -> FfiResult {
        FfiResult::OK
    }

    unsafe extern "C" fn epoch_col_info(
        _: *const BamIter,
        term_index: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        let info = match term_index {
            0 => BamColumnInfo {
                component: BamComponent::from_raw(1),
                uuid: ComponentUuid::from_parts("iter_epoch::ReadU32", 4, 4, 0),
                size: 4,
                align: 4,
                mode: AccessMode::READ,
                storage_flags: 0,
                reserved0: 0,
            },
            1 => BamColumnInfo {
                component: BamComponent::from_raw(2),
                uuid: ComponentUuid::from_parts("iter_epoch::WriteU32", 4, 4, 0),
                size: 4,
                align: 4,
                mode: AccessMode::WRITE,
                storage_flags: 0,
                reserved0: 0,
            },
            _ => return FfiResult::OUT_OF_RANGE,
        };
        // SAFETY: `out` is writable stack storage provided by `column_info_uncached`.
        unsafe { *out = info };
        FfiResult::OK
    }

    unsafe extern "C" fn epoch_next_chunk(_iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: `out_len` is writable stack storage provided by `ValidatedIter::next_chunk`.
        unsafe { *out_len = 2 };
        FfiBool::TRUE
    }

    unsafe extern "C" fn epoch_next_chunk_batch(
        iter: *mut BamIter,
        out_len: *mut usize,
        _out_entities: *mut *const FfiEntity,
        out_col_ptrs: *mut *mut u8,
        col_count: usize,
        _mark_changed_mask: u32,
    ) -> FfiBool {
        // SAFETY: `iter` points to a valid `EpochTestIter` for the test lifetime.
        let st = unsafe { &mut *iter.cast::<EpochTestIter>() };
        // SAFETY: `out_len` is writable stack storage and `out_col_ptrs` has `col_count` slots.
        unsafe {
            *out_len = st.col_ro.len();
            if col_count > 0 {
                *out_col_ptrs.add(0) = st.col_ro.as_mut_ptr().cast::<u8>();
            }
            if col_count > 1 {
                *out_col_ptrs.add(1) = st.col_rw.as_mut_ptr().cast::<u8>();
            }
        }
        FfiBool::TRUE
    }

    unsafe extern "C" fn epoch_col_ro(iter: *const BamIter, term_index: usize) -> *const u8 {
        // SAFETY: `iter` points to `EpochTestIter` with interior test counters updated sequentially.
        let st = unsafe { &mut *iter.cast::<EpochTestIter>().cast_mut() };
        st.legacy_ro_calls += 1;
        if term_index == 0 {
            st.col_ro.as_ptr().cast::<u8>()
        } else {
            core::ptr::null()
        }
    }

    unsafe extern "C" fn epoch_col_rw(iter: *mut BamIter, term_index: usize) -> *mut u8 {
        // SAFETY: `iter` points to `EpochTestIter`.
        let st = unsafe { &mut *iter.cast::<EpochTestIter>() };
        st.legacy_rw_calls += 1;
        if term_index == 1 {
            st.col_rw.as_mut_ptr().cast::<u8>()
        } else {
            core::ptr::null_mut()
        }
    }

    unsafe extern "C" fn epoch_reset(_iter: *mut BamIter) -> FfiResult {
        FfiResult::OK
    }

    fn make_epoch_api() -> BamHostApi {
        // SAFETY: `BamHostApi` is `#[repr(C)]` of `u32` and `Option<extern "C" fn>`.
        let mut api: BamHostApi = unsafe { MaybeUninit::zeroed().assume_init() };
        api.struct_size = core::mem::size_of::<BamHostApi>() as u32;
        api.abi_version = ABI_VERSION;
        api.get_proc_address = Some(stub_get_proc);
        api.last_error = Some(stub_last_error);
        api.load_host_ctx = Some(stub_load_host_ctx);
        api.register_component = Some(stub_register_comp);
        api.resolve_component = Some(stub_resolve_comp);
        api.register_system = Some(stub_register_sys);
        api.sys_host_ctx = Some(stub_sys_host_ctx);
        api.sys_query = Some(stub_sys_query);
        api.sys_commands = Some(stub_sys_commands);
        api.iter_next_chunk = Some(epoch_next_chunk);
        api.iter_reset = Some(epoch_reset);
        api.iter_entities = Some(stub_iter_entities);
        api.iter_column_readonly = Some(epoch_col_ro);
        api.iter_column_mutable = Some(epoch_col_rw);
        api.iter_column_mutable_untracked = Some(epoch_col_rw);
        api.iter_mark_changed_range = Some(stub_iter_mark_changed);
        api.iter_column_rows_readonly = Some(stub_iter_rows_ro);
        api.iter_column_rows_mutable = Some(stub_iter_rows_mut);
        api.iter_column_info = Some(epoch_col_info);
        api.res_ptr = Some(stub_res_ptr);
        api.res_mut_ptr = Some(stub_res_mut_ptr);
        api.cmd_spawn_empty = Some(stub_cmd_spawn_empty);
        api.cmd_spawn = Some(stub_cmd_spawn);
        api.cmd_despawn = Some(stub_cmd_despawn);
        api.cmd_insert = Some(stub_cmd_insert);
        api.cmd_insert_default = Some(stub_cmd_insert_default);
        api.cmd_remove = Some(stub_cmd_remove);
        api.iter_next_chunk_batch = Some(epoch_next_chunk_batch);
        api
    }

    #[test]
    fn bump_batch_epoch_saturates_sticky_zero_forever() {
        let api = make_epoch_api();
        let dummy_ctx = 0u64;
        // SAFETY: `api` and `dummy_ctx` outlive `host`.
        let host = unsafe { Host::from_raw(&api, (&raw const dummy_ctx).cast()) }.unwrap();

        let mut st = EpochTestIter {
            col_ro: [10, 20],
            col_rw: [30, 40],
            legacy_ro_calls: 0,
            legacy_rw_calls: 0,
        };
        let mut scratch = QuerySlotScratch::new();
        // SAFETY: `st` and `scratch` outlive `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw_with_scratch(
                (&raw mut st).cast::<BamIter>(),
                &host,
                &mut scratch,
            )
        };

        // Populate `BatchColCache` at epoch 2 and `QuerySlotScratch` at epoch 3.
        let mut cache_epoch2 = BatchColCache::new();
        assert_eq!(
            iter.advance_batch_into(&mut cache_epoch2, 2, false, 0),
            Some(2)
        );
        assert_eq!(iter.batch_epoch, 2);
        assert_eq!(cache_epoch2.batch_epoch, 2);
        assert_eq!(cache_epoch2.len, 2);

        assert_eq!(iter.advance_batch(2, false, 0), Some(2));
        assert_eq!(iter.batch_epoch, 3);
        assert_eq!(iter.scratch_batch_epoch, 3);

        // Also populate a `BatchColCache` and `QuerySlotScratch` right at `u32::MAX`.
        iter.batch_epoch = u32::MAX - 1;
        let mut cache_epoch_max = BatchColCache::new();
        assert_eq!(
            iter.advance_batch_into(&mut cache_epoch_max, 2, false, 0),
            Some(2)
        );
        assert_eq!(iter.batch_epoch, u32::MAX);
        assert_eq!(cache_epoch_max.batch_epoch, u64::from(u32::MAX));

        iter.batch_epoch = u32::MAX - 1;
        assert_eq!(iter.advance_batch(2, false, 0), Some(2));
        assert_eq!(iter.batch_epoch, u32::MAX);
        assert_eq!(iter.scratch_batch_epoch, u32::MAX);

        // Overflow `u32::MAX -> 0` and verify `0` is permanently sticky across repeated transitions.
        iter.batch_epoch = u32::MAX;
        iter.bump_batch_epoch();
        assert_eq!(iter.batch_epoch, 0);

        for _ in 0..8 {
            iter.bump_batch_epoch();
            assert_eq!(iter.batch_epoch, 0);
        }

        // Neither dispenser may match stale epoch-2 or epoch-u32::MAX caches once saturated at `0`.
        {
            let disp = ChunkColumnDispenser::new(&mut iter);
            assert_eq!(disp.batch_col_count, 0);
        }
        {
            let disp = ChunkColumnDispenser::new_with_batch(&mut iter, &cache_epoch2);
            assert_eq!(disp.batch_col_count, 0);
        }
        {
            let disp = ChunkColumnDispenser::new_with_batch(&mut iter, &cache_epoch_max);
            assert_eq!(disp.batch_col_count, 0);
        }

        // Verify `next_chunk()`, `advance_batch()`, `advance_batch_into()`, and `reset()` keep
        // `batch_epoch == 0` permanently and leave both dispensers with `batch_col_count == 0`.
        assert_eq!(iter.next_chunk(), Some(2));
        assert_eq!(iter.batch_epoch, 0);
        assert_eq!(ChunkColumnDispenser::new(&mut iter).batch_col_count, 0);
        assert_eq!(
            ChunkColumnDispenser::new_with_batch(&mut iter, &cache_epoch2).batch_col_count,
            0
        );

        assert_eq!(iter.advance_batch(2, false, 0), Some(2));
        assert_eq!(iter.batch_epoch, 0);
        assert_eq!(ChunkColumnDispenser::new(&mut iter).batch_col_count, 0);
        assert_eq!(
            ChunkColumnDispenser::new_with_batch(&mut iter, &cache_epoch_max).batch_col_count,
            0
        );

        let mut cache_saturated = BatchColCache::new();
        assert_eq!(
            iter.advance_batch_into(&mut cache_saturated, 2, false, 0),
            Some(2)
        );
        assert_eq!(iter.batch_epoch, 0);
        assert_eq!(cache_saturated.batch_epoch, 0);
        assert_eq!(cache_saturated.len, 0);
        assert_eq!(ChunkColumnDispenser::new(&mut iter).batch_col_count, 0);
        assert_eq!(
            ChunkColumnDispenser::new_with_batch(&mut iter, &cache_saturated).batch_col_count,
            0
        );
        assert_eq!(
            ChunkColumnDispenser::new_with_batch(&mut iter, &cache_epoch2).batch_col_count,
            0
        );

        assert_eq!(iter.reset(), FfiResult::OK);
        assert_eq!(iter.batch_epoch, 0);
        assert_eq!(iter.next_chunk(), Some(2));
        assert_eq!(iter.batch_epoch, 0);
        {
            let mut disp = ChunkColumnDispenser::new_with_batch(&mut iter, &cache_epoch2);
            assert_eq!(disp.batch_col_count, 0);
            assert_eq!(
                disp.next_read::<u32>(AccessMode::READ, None),
                Ok(&[10, 20][..])
            );
            assert_eq!(disp.next_write::<u32>(None), Ok(&mut [30, 40][..]));
        }
        assert_eq!(st.legacy_ro_calls, 1);
        assert_eq!(st.legacy_rw_calls, 1);
    }
}
