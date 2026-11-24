//! ABI-safe `#[repr(C)]` optional value wrapper (`FfiOption<T>`).

use core::{
    fmt,
    hash::{Hash, Hasher},
    mem::MaybeUninit,
};

use crate::primitives::FfiBool;

/// Cross-language `#[repr(C)]` optional value (`is_some: FfiBool` + `value: T`).
///
/// Rust's `Option<T>` does not have a guaranteed cross-compiler layout unless `T` has a specific
/// pointer niche, and never matches a C99 tagged struct. `FfiOption<T>` provides a deterministic
/// `#[repr(C)]` wire representation across Rust, C99, C++17, and Zig.
///
/// # Example
///
/// ```
/// use bamabi_sys::FfiOption;
///
/// let some_val = FfiOption::some(42u32);
/// assert!(some_val.is_some());
/// assert_eq!(some_val.as_ref(), Some(&42));
/// assert_eq!(some_val.into_option(), Some(42));
///
/// let none_val = FfiOption::<u32>::none();
/// assert!(none_val.is_none());
/// assert_eq!(none_val.as_ref(), None);
/// assert_eq!(none_val.into_option(), None);
/// ```
#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiOption<T> {
    /// Presence tag (`0` = `None`, any non-zero `u8` = `Some(value)`).
    pub is_some: FfiBool,
    /// Payload slot (initialized when `is_some` is non-zero; zero-filled by [`Self::none`]).
    pub value: T,
}

impl<T> FfiOption<T> {
    /// `true` iff `#[repr(C)]` layout inserts implicit alignment padding between `is_some` and `value`.
    pub const HAS_IMPLICIT_PADDING: bool = core::mem::size_of::<Self>()
        != (core::mem::size_of::<FfiBool>() + core::mem::size_of::<T>());

    /// Constructs an engaged `FfiOption::Some(value)` (`is_some = FfiBool::TRUE`).
    #[inline]
    pub const fn some(value: T) -> Self {
        Self {
            is_some: FfiBool::TRUE,
            value,
        }
    }

    /// Writes a disengaged `FfiOption::None` into `dst` with every byte—including any implicit
    /// alignment padding between `is_some` and `value`—explicitly zero-initialized.
    ///
    /// Because moving `FfiOption<T>` by value in Rust can leave alignment padding bytes
    /// uninitialized, callers that require deterministic zeroed padding in memory (for raw byte
    /// hashing or C-ABI wire buffers) must initialize in place via [`Self::write_none_zeroed`]
    /// or serialize via [`Self::write_zeroed_bytes`], rather than passing `Self` by value.
    #[inline]
    pub fn write_none_zeroed(dst: &mut MaybeUninit<Self>) -> *mut Self
    where
        T: crate::traits::RawInsertable,
    {
        // SAFETY: `dst.as_mut_ptr()` points to `size_of::<Self>()` writable bytes of
        // `MaybeUninit<Self>`. Filling all bytes with `0` sets `is_some = FfiBool::FALSE`,
        // zero-initializes `value: T` (valid for `T: RawInsertable`), and initializes every
        // alignment padding byte to `0`.
        unsafe {
            core::ptr::write_bytes(
                dst.as_mut_ptr().cast::<u8>(),
                0,
                core::mem::size_of::<Self>(),
            );
        }
        dst.as_mut_ptr()
    }

    /// Serializes `self` into `dst` with every implicit alignment padding byte explicitly zero-filled,
    /// returning `Some(size_of::<Self>())` on success or `None` if `dst.len() < size_of::<Self>()`.
    #[inline]
    pub fn write_zeroed_bytes(&self, dst: &mut [u8]) -> Option<usize>
    where
        T: crate::traits::AbiPod,
    {
        let total = core::mem::size_of::<Self>();
        if dst.len() < total {
            return None;
        }
        let target = dst.get_mut(..total)?;
        target.fill(0);
        target[0] = self.is_some.0;
        if self.is_some.get() {
            let val_off = core::mem::offset_of!(Self, value);
            let val_bytes = self.value.as_bytes();
            let val_dst = target.get_mut(val_off..val_off + val_bytes.len())?;
            val_dst.copy_from_slice(val_bytes);
        }
        Some(total)
    }

    /// Validates that `is_some` holds a well-formed `FfiBool` (`0` = `None`, non-zero = `Some`).
    #[inline]
    pub const fn validate(&self) -> crate::FfiResult {
        let _ = self.is_some.0;
        crate::FfiResult::OK
    }

    /// Constructs a disengaged `FfiOption::None` for `T: RawInsertable` by zero-filling `value`.
    ///
    /// Note: returning `FfiOption<T>` by value does not guarantee initialized alignment padding
    /// bytes across moves; use [`Self::write_none_zeroed`] or [`Self::write_zeroed_bytes`] when
    /// reading raw struct bytes.
    #[inline]
    pub const fn none() -> Self
    where
        T: crate::traits::RawInsertable,
    {
        // SAFETY: `T: RawInsertable` guarantees zero drop glue and valid all-bits-zero representation.
        let value = unsafe { MaybeUninit::<T>::zeroed().assume_init() };
        Self {
            is_some: FfiBool::FALSE,
            value,
        }
    }

    /// Constructs a disengaged `FfiOption::None` using `T::default()` as the placeholder payload.
    #[inline]
    pub fn none_default() -> Self
    where
        T: Default,
    {
        Self {
            is_some: FfiBool::FALSE,
            value: T::default(),
        }
    }

    /// Returns `true` if `self.is_some` is non-zero.
    #[inline]
    pub const fn is_some(&self) -> bool {
        self.is_some.get()
    }

    /// Returns `true` if `self.is_some` is zero (`FfiBool::FALSE`).
    #[inline]
    pub const fn is_none(&self) -> bool {
        !self.is_some.get()
    }

    /// Converts this `FfiOption<T>` into a standard Rust `Option<T>`.
    #[inline]
    pub fn into_option(self) -> Option<T> {
        if self.is_some.get() {
            Some(self.value)
        } else {
            None
        }
    }

    /// Converts a standard Rust `Option<T>` (`T: RawInsertable`) into an `FfiOption<T>`.
    #[inline]
    pub fn from_option(opt: Option<T>) -> Self
    where
        T: crate::traits::RawInsertable,
    {
        match opt {
            Some(v) => Self::some(v),
            None => Self::none(),
        }
    }

    /// Converts a standard Rust `Option<T>` (`T: Default`) into an `FfiOption<T>`.
    #[inline]
    pub fn from_option_default(opt: Option<T>) -> Self
    where
        T: Default,
    {
        match opt {
            Some(v) => Self::some(v),
            None => Self::none_default(),
        }
    }

    /// Borrows the inner payload as `Option<&T>`.
    #[inline]
    pub const fn as_ref(&self) -> Option<&T> {
        if self.is_some.get() {
            Some(&self.value)
        } else {
            None
        }
    }

    /// Mutably borrows the inner payload as `Option<&mut T>`.
    #[inline]
    pub const fn as_mut(&mut self) -> Option<&mut T> {
        if self.is_some.get() {
            Some(&mut self.value)
        } else {
            None
        }
    }
}

impl<T: Default> Default for FfiOption<T> {
    #[inline]
    fn default() -> Self {
        Self::none_default()
    }
}

impl<T: crate::traits::RawInsertable> From<Option<T>> for FfiOption<T> {
    #[inline]
    fn from(opt: Option<T>) -> Self {
        Self::from_option(opt)
    }
}

impl<T> From<FfiOption<T>> for Option<T> {
    #[inline]
    fn from(opt: FfiOption<T>) -> Self {
        opt.into_option()
    }
}

impl<T: fmt::Debug> fmt::Debug for FfiOption<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_ref() {
            Some(val) => f.debug_tuple("Some").field(val).finish(),
            None => f.write_str("None"),
        }
    }
}

impl<T: PartialEq> PartialEq for FfiOption<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl<T: Eq> Eq for FfiOption<T> {}

impl<T: Hash> Hash for FfiOption<T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}

const _: () = {
    assert!(core::mem::size_of::<FfiOption<u8>>() == 2);
    assert!(core::mem::align_of::<FfiOption<u8>>() == 1);
    assert!(core::mem::offset_of!(FfiOption<u32>, is_some) == 0);
    assert!(core::mem::offset_of!(FfiOption<u32>, value) == 4);
    assert!(core::mem::size_of::<FfiOption<u32>>() == 8);
    assert!(core::mem::align_of::<FfiOption<u32>>() == 4);
};
