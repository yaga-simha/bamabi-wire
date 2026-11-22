//! Zero-dependency dynamic library loader (`dlopen`/`dlsym`/`dlclose` on Unix,
//! `LoadLibraryW`/`GetProcAddress`/`FreeLibrary` on Windows) for polyglot `bamabi` guests.

use core::ffi::{CStr, c_void};
use std::path::Path;

use bamabi_sys::{
    ABI_VERSION, BamInitFn, BamInstance, BamManifest, BamManifestFn, BamShutdownFn, FfiResult,
    ScheduleId, read_descriptor,
};

use crate::host::OwnedHostWorld;

/// Required benchmark system names registered by every guest in [`ScheduleId::UPDATE`].
pub(crate) const REQUIRED_SYSTEMS: [&str; 4] = [
    "bench_noop",
    "bench_dense_update",
    "bench_command_burst",
    "bench_host_api_roundtrip",
];

#[cfg(unix)]
mod os {
    use core::ffi::{CStr, c_char, c_int, c_void};
    use std::{ffi::CString, os::unix::ffi::OsStrExt, path::Path, sync::Mutex};

    const RTLD_NOW: c_int = 2;
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos"
    ))]
    const RTLD_LOCAL: c_int = 4;
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos"
    )))]
    const RTLD_LOCAL: c_int = 0;

    static DL_LOCK: Mutex<()> = Mutex::new(());

    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
        fn dlerror() -> *const c_char;
    }

    pub(super) struct OsLib {
        handle: *mut c_void,
    }

    impl OsLib {
        pub(super) fn open(path: &Path) -> Result<Self, String> {
            let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
                format!(
                    "library path contains interior NUL byte: {}",
                    path.display()
                )
            })?;
            let _guard = DL_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // SAFETY: Clearing any stale thread-local `dlerror` state prior to `dlopen`.
            unsafe {
                let _ = dlerror();
            }
            // SAFETY: `c_path` is a valid NUL-terminated path C string.
            let handle = unsafe { dlopen(c_path.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
            if handle.is_null() {
                // SAFETY: `dlerror` returns a static/thread-local C string or null after failure.
                let err_ptr = unsafe { dlerror() };
                let detail = if err_ptr.is_null() {
                    "unknown dlopen error".to_owned()
                } else {
                    // SAFETY: `err_ptr` is non-null NUL-terminated string from `dlerror`.
                    unsafe { CStr::from_ptr(err_ptr) }
                        .to_string_lossy()
                        .into_owned()
                };
                return Err(format!("dlopen({}) failed: {detail}", path.display()));
            }
            Ok(Self { handle })
        }

        pub(super) fn symbol(&self, name: &CStr) -> Result<*mut c_void, String> {
            let _guard = DL_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // SAFETY: Clear stale `dlerror` state before `dlsym` lookup.
            unsafe {
                let _ = dlerror();
            }
            // SAFETY: `self.handle` is an active `dlopen` handle and `name` is NUL-terminated.
            let sym = unsafe { dlsym(self.handle, name.as_ptr()) };
            if sym.is_null() {
                // SAFETY: Check `dlerror` for missing symbol diagnostic.
                let err_ptr = unsafe { dlerror() };
                let detail = if err_ptr.is_null() {
                    "symbol resolved to null".to_owned()
                } else {
                    // SAFETY: `err_ptr` is non-null NUL-terminated string from `dlerror`.
                    unsafe { CStr::from_ptr(err_ptr) }
                        .to_string_lossy()
                        .into_owned()
                };
                return Err(format!(
                    "dlsym({}) failed: {detail}",
                    name.to_string_lossy()
                ));
            }
            Ok(sym)
        }
    }

    impl Drop for OsLib {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                let _guard = DL_LOCK
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // SAFETY: `self.handle` was returned by `dlopen` and is closed exactly once on drop.
                unsafe {
                    let _ = dlclose(self.handle);
                }
                self.handle = core::ptr::null_mut();
            }
        }
    }
}

#[cfg(windows)]
mod os {
    use core::ffi::{CStr, c_void};
    use std::{os::windows::ffi::OsStrExt, path::Path};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *mut c_void;
        fn FreeLibrary(h_module: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    pub(super) struct OsLib {
        handle: *mut c_void,
    }

    impl OsLib {
        pub(super) fn open(path: &Path) -> Result<Self, String> {
            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(core::iter::once(0))
                .collect();
            // SAFETY: `wide` is a NUL-terminated UTF-16 path string.
            let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
            if handle.is_null() {
                // SAFETY: `GetLastError` retrieves the Win32 error code for the failed call.
                let code = unsafe { GetLastError() };
                return Err(format!(
                    "LoadLibraryW({}) failed with Win32 error {code}",
                    path.display()
                ));
            }
            Ok(Self { handle })
        }

        pub(super) fn symbol(&self, name: &CStr) -> Result<*mut c_void, String> {
            // SAFETY: `self.handle` is a valid HMODULE and `name` is NUL-terminated ASCII.
            let sym = unsafe { GetProcAddress(self.handle, name.as_ptr().cast::<u8>()) };
            if sym.is_null() {
                // SAFETY: `GetLastError` retrieves the Win32 error code for the failed call.
                let code = unsafe { GetLastError() };
                return Err(format!(
                    "GetProcAddress({}) failed with Win32 error {code}",
                    name.to_string_lossy()
                ));
            }
            Ok(sym)
        }
    }

    impl Drop for OsLib {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` was returned by `LoadLibraryW` and is freed once here.
                unsafe {
                    let _ = FreeLibrary(self.handle);
                }
                self.handle = core::ptr::null_mut();
            }
        }
    }
}

/// Platform-agnostic handle to a dynamically loaded shared library.
pub(crate) struct DynamicLibrary {
    inner: os::OsLib,
}

impl DynamicLibrary {
    /// Opens a shared library (`.dylib`, `.so`, `.dll`) at `path`.
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        Ok(Self {
            inner: os::OsLib::open(path)?,
        })
    }

    /// Resolves a raw symbol address by C string name.
    pub(crate) fn symbol_raw(&self, name: &CStr) -> Result<*mut c_void, String> {
        self.inner.symbol(name)
    }
}

/// Optional message benchmark system names recognized if exported by a guest in
/// [`ScheduleId::UPDATE`].
pub(crate) const OPTIONAL_MESSAGE_SYSTEMS: [&str; 3] = [
    "bench_message_idle_poll",
    "bench_message_burst_256",
    "bench_message_burst_4096",
];

/// Resolved system indices registered by a loaded guest module.
#[derive(Debug, Copy, Clone)]
pub(crate) struct GuestSystems {
    pub(crate) noop: usize,
    pub(crate) dense_update: usize,
    pub(crate) command_burst: usize,
    pub(crate) host_api_roundtrip: usize,
    pub(crate) message_idle_poll: Option<usize>,
    pub(crate) message_burst_256: Option<usize>,
    pub(crate) message_burst_4096: Option<usize>,
}

/// Metadata extracted from a guest's [`BamManifest`].
#[derive(Debug, Clone)]
pub(crate) struct ManifestInfo {
    pub(crate) _abi_version: u32,
    pub(crate) name: String,
    pub(crate) version: String,
}

/// Loaded and initialized polyglot guest mod paired with its lifecycle entry points.
pub(crate) struct LoadedGuest {
    pub(crate) manifest: ManifestInfo,
    pub(crate) systems: GuestSystems,
    instance: *mut BamInstance,
    shutdown_fn: BamShutdownFn,
    shut_down: bool,
    // Declared last so `dlclose` / `FreeLibrary` runs strictly after `bamabi_shutdown`.
    _lib: DynamicLibrary,
}

impl Drop for LoadedGuest {
    fn drop(&mut self) {
        if !self.shut_down {
            // SAFETY: Guest library is still mapped (`_lib` drops after `self` body); passing the
            // session `instance` pointer returned by `bamabi_init` obeys `BamShutdownFn` contract.
            unsafe {
                (self.shutdown_fn)(self.instance);
            }
            self.shut_down = true;
            self.instance = core::ptr::null_mut();
        }
    }
}

impl LoadedGuest {
    /// Dynamically loads `path`, verifies its [`BamManifest`], initializes it against `world`,
    /// and confirms all 4 benchmark systems are registered in [`ScheduleId::UPDATE`].
    pub(crate) fn load_and_init(path: &Path, world: &mut OwnedHostWorld) -> Result<Self, String> {
        let lib = DynamicLibrary::open(path)?;

        let manifest_ptr = lib.symbol_raw(c"bamabi_manifest")?;
        let init_ptr = lib.symbol_raw(c"bamabi_init")?;
        let shutdown_ptr = lib.symbol_raw(c"bamabi_shutdown")?;

        // SAFETY: POSIX `dlsym` / Win32 `GetProcAddress` returned a valid non-null code pointer for
        // `bamabi_manifest`, whose C ABI signature is `BamManifestFn`.
        let manifest_fn: BamManifestFn = unsafe { core::mem::transmute(manifest_ptr) };
        // SAFETY: Valid symbol pointer for `bamabi_init` matching `BamInitFn`.
        let init_fn: BamInitFn = unsafe { core::mem::transmute(init_ptr) };
        // SAFETY: Valid symbol pointer for `bamabi_shutdown` matching `BamShutdownFn`.
        let shutdown_fn: BamShutdownFn = unsafe { core::mem::transmute(shutdown_ptr) };

        // SAFETY: `bamabi_manifest` takes no arguments and returns a static `*const BamManifest`.
        let raw_manifest = unsafe { manifest_fn() };
        if raw_manifest.is_null() {
            return Err(format!(
                "{}: bamabi_manifest() returned null",
                path.display()
            ));
        }
        if !(raw_manifest as usize).is_multiple_of(core::mem::align_of::<BamManifest>()) {
            return Err(format!(
                "{}: bamabi_manifest() returned unaligned pointer",
                path.display()
            ));
        }

        // SAFETY: `raw_manifest` is non-null, aligned, and points to static `BamManifest` data.
        let manifest = unsafe { read_descriptor::<BamManifest>(raw_manifest.cast::<c_void>()) }
            .map_err(|e| format!("{}: invalid BamManifest header: {e}", path.display()))?;
        let valid = manifest.validate_for_host(ABI_VERSION);
        if !valid.is_ok() {
            return Err(format!(
                "{}: manifest validation failed: {valid}",
                path.display()
            ));
        }
        if manifest.abi_version != ABI_VERSION {
            return Err(format!(
                "{}: manifest ABI version 0x{:08x} != host 0x{:08x}",
                path.display(),
                manifest.abi_version,
                ABI_VERSION
            ));
        }
        // SAFETY: `manifest.validate_for_host` checked `manifest.name` is non-null.
        let mod_name = unsafe { CStr::from_ptr(manifest.name) }
            .to_str()
            .map_err(|_| format!("{}: manifest name is not valid UTF-8", path.display()))?
            .to_owned();
        if mod_name.is_empty() {
            return Err(format!(
                "{}: manifest name must be non-empty",
                path.display()
            ));
        }
        // SAFETY: `manifest.validate_for_host` checked `manifest.version` is non-null.
        let mod_version = unsafe { CStr::from_ptr(manifest.version) }
            .to_str()
            .map_err(|_| format!("{}: manifest version is not valid UTF-8", path.display()))?
            .to_owned();
        if mod_version.is_empty() {
            return Err(format!(
                "{}: manifest version must be non-empty",
                path.display()
            ));
        }

        let mut instance: *mut BamInstance = core::ptr::null_mut();
        // SAFETY: `world.api_ptr()` points to a valid 73-slot `BamHostApi`, `world.load_ctx_ptr()`
        // points to `HostWorld` for the duration of `bamabi_init`, and `instance` is writable.
        let init_status = unsafe { init_fn(world.api_ptr(), world.load_ctx_ptr(), &mut instance) };
        if init_status != FfiResult::OK {
            let err_msg = world.last_error_string();
            let faults = world.faults();
            return Err(format!(
                "{} (`{mod_name}`): bamabi_init failed with {init_status} (last_error={err_msg:?}, faults={faults:?})",
                path.display()
            ));
        }
        if world.has_fault() {
            return Err(format!(
                "{} (`{mod_name}`): guest reported fault during bamabi_init: {:?}",
                path.display(),
                world.faults()
            ));
        }

        let systems = Self::resolve_required_systems(world, path, &mod_name)?;

        Ok(Self {
            manifest: ManifestInfo {
                _abi_version: manifest.abi_version,
                name: mod_name,
                version: mod_version,
            },
            systems,
            instance,
            shutdown_fn,
            shut_down: false,
            _lib: lib,
        })
    }

    fn resolve_required_systems(
        world: &OwnedHostWorld,
        path: &Path,
        mod_name: &str,
    ) -> Result<GuestSystems, String> {
        let mut indices = [0usize; 4];
        for (slot, sys_name) in REQUIRED_SYSTEMS.iter().enumerate() {
            let idx = world.find_system(sys_name).ok_or_else(|| {
                format!(
                    "{} (`{mod_name}`): missing required system `{sys_name}`",
                    path.display()
                )
            })?;
            let sched = world.systems[idx].schedule;
            if sched != ScheduleId::UPDATE {
                return Err(format!(
                    "{} (`{mod_name}`): system `{sys_name}` registered in schedule {sched}, expected UPDATE",
                    path.display()
                ));
            }
            indices[slot] = idx;
        }
        let mut opt_indices = [None; 3];
        for (slot, sys_name) in OPTIONAL_MESSAGE_SYSTEMS.iter().enumerate() {
            if let Some(idx) = world.find_system(sys_name) {
                let sched = world.systems[idx].schedule;
                if sched != ScheduleId::UPDATE {
                    return Err(format!(
                        "{} (`{mod_name}`): system `{sys_name}` registered in schedule {sched}, expected UPDATE",
                        path.display()
                    ));
                }
                opt_indices[slot] = Some(idx);
            }
        }
        Ok(GuestSystems {
            noop: indices[0],
            dense_update: indices[1],
            command_burst: indices[2],
            host_api_roundtrip: indices[3],
            message_idle_poll: opt_indices[0],
            message_burst_256: opt_indices[1],
            message_burst_4096: opt_indices[2],
        })
    }

    /// Shuts down the guest cleanly before unloading the underlying dynamic library.
    pub(crate) fn shutdown(mut self, world: &mut OwnedHostWorld) -> Result<(), String> {
        world.clear_systems_for_unload();
        if !self.shut_down {
            // SAFETY: `self._lib` is still mapped and `self.instance` came from `bamabi_init`.
            unsafe {
                (self.shutdown_fn)(self.instance);
            }
            self.shut_down = true;
            self.instance = core::ptr::null_mut();
        }
        if world.has_fault() {
            return Err(format!(
                "guest `{}` reported fault during shutdown: {:?}",
                self.manifest.name,
                world.faults()
            ));
        }
        Ok(())
    }
}
