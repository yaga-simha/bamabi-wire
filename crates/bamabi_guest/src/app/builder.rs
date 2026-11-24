//! `GuestApp` initialization session and unified `SystemBuilder` / `QueryBuilder`.

use core::ffi::{CStr, c_void};

use bamabi_guest_sys::{
    GuestInstance, Host, PreparedRunIf, SystemRegistrationConfig, TypedComponent, ValidatedLoadCtx,
    ValidatedSystemInvocation, ValidatedSystemSlice, allocate_run_if_ctx_query,
    allocate_run_if_ctx_trampoline, resolve_component, resolve_mod_component,
};
use bamabi_sys::{
    AbiPod, AccessMode, BAM_COMPONENT_FLAG_IMMUTABLE, BAM_COMPONENT_FLAG_SPARSE_SET,
    BAM_COMPONENT_FLAG_SUMMARY_TICK, BAM_SYSTEM_FLAG_AMBIGUOUS_WITH_ALL,
    BAM_SYSTEM_FLAG_MAIN_THREAD, BAM_TERM_KNOWN_FLAGS, BamComponent, BamFieldDesc, BamHostApi,
    BamLoadCtx, BamRequiredComponent, BamSystem, BamTermDesc, ComponentDropFn, ComponentUuid,
    FfiDisabled, FfiEntity, FfiResult, MAX_QUERIES_PER_SYSTEM, MAX_TERMS_PER_QUERY, ModComponent,
    ScheduleId,
};

use crate::{
    app::{
        conditions::EcsRunCondition,
        context::{RunConditionContext, RunConditionSystemContext, SystemContext},
        observers::{RunIfPredicate, with_guest_commands},
    },
    commands::GuestCommands,
    component::{
        GuestComponent, GuestComponentHooks, qualify_component_identity, resolve_guest_component,
        resolve_guest_component_uncached,
    },
    iter::{ChunkIter, typed_query::QueryTuple},
};

/// Boxed ECS-aware run-condition predicate paired with its base term offset in `run_if_query`.
type RunIfCtxPredicate =
    Box<dyn FnMut(&mut RunConditionSystemContext<'_>, usize) -> bool + Send + 'static>;

mod private {
    /// Private sealing trait preventing downstream crates from implementing [`super::IntoSystemResult`].
    pub trait Sealed {}
}

/// Return-type conversion trait permitting [`SystemBuilder::build`] closures to return
/// [`FfiResult`], `()`, or `Result<(), FfiResult>`.
pub trait IntoSystemResult: private::Sealed {
    /// Converts the callback return value into an [`FfiResult`].
    fn into_ffi_result(self) -> FfiResult;
}

impl private::Sealed for FfiResult {}
impl IntoSystemResult for FfiResult {
    /// Returns the [`FfiResult`] status code unchanged.
    #[inline]
    fn into_ffi_result(self) -> FfiResult {
        self
    }
}

impl private::Sealed for () {}
impl IntoSystemResult for () {
    /// Converts an infallible `()` system return into [`FfiResult::OK`].
    #[inline]
    fn into_ffi_result(self) -> FfiResult {
        FfiResult::OK
    }
}

impl private::Sealed for Result<(), FfiResult> {}
impl IntoSystemResult for Result<(), FfiResult> {
    /// Maps `Ok(())` to [`FfiResult::OK`] and forwards `Err(err)` unchanged.
    #[inline]
    fn into_ffi_result(self) -> FfiResult {
        match self {
            Ok(()) => FfiResult::OK,
            Err(err) => err,
        }
    }
}

/// Fluent registration options for [`GuestApp::register_component_with_options`], supporting
/// storage flags (`summary_tick`, `sparse_set`, `immutable`), `EntityCloner` support (`clone`),
/// full lifecycle hooks (`on_add`, `on_insert`, `on_discard`, `on_remove`, `on_despawn`), and
/// transitive required components (`require::<R>()`).
pub struct GuestComponentOptions<T: AbiPod> {
    flags: u32,
    hooks: GuestComponentHooks<T>,
    required_resolvers: Vec<fn(&Host, &str) -> Result<BamComponent, FfiResult>>,
}

impl<T: AbiPod> Default for GuestComponentOptions<T> {
    /// Delegates to [`GuestComponentOptions::new`] with zero flags, no hooks, and no requirements.
    fn default() -> Self {
        Self::new()
    }
}

impl<T: AbiPod> GuestComponentOptions<T> {
    /// Creates default registration options (`flags = 0`, no clone/hooks/requirements).
    #[inline]
    pub fn new() -> Self {
        Self {
            flags: 0,
            hooks: GuestComponentHooks::new(),
            required_resolvers: Vec::new(),
        }
    }

    /// Adds explicit `BAM_COMPONENT_FLAG_*` flags (merged with `T::FLAGS`).
    #[inline]
    pub const fn flags(mut self, flags: u32) -> Self {
        self.flags |= flags;
        self
    }

    /// Configures `StorageType::SparseSet` (`BAM_COMPONENT_FLAG_SPARSE_SET`).
    #[inline]
    pub const fn sparse_set(mut self) -> Self {
        self.flags |= BAM_COMPONENT_FLAG_SPARSE_SET;
        self
    }

    /// Marks the component as immutable (`BAM_COMPONENT_FLAG_IMMUTABLE`).
    #[inline]
    pub const fn immutable(mut self) -> Self {
        self.flags |= BAM_COMPONENT_FLAG_IMMUTABLE;
        self
    }

    /// Enables Bevy 0.20 per-table `summary_tick` acceleration (`BAM_COMPONENT_FLAG_SUMMARY_TICK`).
    #[inline]
    pub const fn summary_tick(mut self) -> Self {
        self.flags |= BAM_COMPONENT_FLAG_SUMMARY_TICK;
        self
    }

    /// Enables bitwise `EntityCloner` (`clone_fn`) support for `T: AbiPod`.
    #[inline]
    pub fn clone(mut self) -> Self {
        self.hooks = self.hooks.with_clone(true);
        self
    }

    /// Attaches a pre-built [`GuestComponentHooks<T>`] configuration, preserving any `clone`,
    /// ABI 0.3 metadata, or required handles already set on `self` or `hooks`.
    pub fn hooks(mut self, hooks: GuestComponentHooks<T>) -> Self {
        let enable_clone = self.hooks.inner.enable_clone || hooks.inner.enable_clone;
        let mut reqs = self.hooks.inner.required_components;
        reqs.extend_from_slice(&hooks.inner.required_components);
        let schema_fingerprint = if hooks.inner.schema_fingerprint != 0 {
            hooks.inner.schema_fingerprint
        } else {
            self.hooks.inner.schema_fingerprint
        };
        let fields = if !hooks.inner.fields.is_empty() {
            hooks.inner.fields
        } else {
            self.hooks.inner.fields
        };
        let mut req_inits = self.hooks.inner.required_init_descs;
        req_inits.extend_from_slice(&hooks.inner.required_init_descs);
        let mut req_bytes = self.hooks.inner.required_custom_bytes;
        req_bytes.extend(hooks.inner.required_custom_bytes);
        self.hooks = GuestComponentHooks {
            inner: bamabi_guest_sys::PodComponentHooks {
                on_add: hooks.inner.on_add,
                on_insert: hooks.inner.on_insert,
                on_discard: hooks.inner.on_discard,
                on_remove: hooks.inner.on_remove,
                on_despawn: hooks.inner.on_despawn,
                enable_clone,
                required_components: reqs,
                schema_fingerprint,
                fields,
                required_init_descs: req_inits,
                required_custom_bytes: req_bytes,
            },
        };
        self
    }

    /// Attaches an `on_add` lifecycle hook.
    #[inline]
    pub fn on_add<F: FnMut(FfiEntity, &T) + Send + 'static>(mut self, callback: F) -> Self {
        self.hooks = self.hooks.on_add(callback);
        self
    }

    /// Attaches an `on_insert` lifecycle hook.
    #[inline]
    pub fn on_insert<F: FnMut(FfiEntity, &T) + Send + 'static>(mut self, callback: F) -> Self {
        self.hooks = self.hooks.on_insert(callback);
        self
    }

    /// Attaches an `on_discard` lifecycle hook.
    #[inline]
    pub fn on_discard<F: FnMut(FfiEntity, &T) + Send + 'static>(mut self, callback: F) -> Self {
        self.hooks = self.hooks.on_discard(callback);
        self
    }

    /// Attaches an `on_remove` lifecycle hook.
    #[inline]
    pub fn on_remove<F: FnMut(FfiEntity, &T) + Send + 'static>(mut self, callback: F) -> Self {
        self.hooks = self.hooks.on_remove(callback);
        self
    }

    /// Attaches an `on_despawn` lifecycle hook.
    #[inline]
    pub fn on_despawn<F: FnMut(FfiEntity, &T) + Send + 'static>(mut self, callback: F) -> Self {
        self.hooks = self.hooks.on_despawn(callback);
        self
    }

    /// Declares a required [`GuestComponent`] `R` resolved on the host when registering `T`.
    #[inline]
    pub fn require<R: GuestComponent>(mut self) -> Self {
        self.required_resolvers.push(resolve_guest_component::<R>);
        self
    }

    /// Declares a pre-resolved required [`BamComponent`] handle.
    #[inline]
    pub fn require_handle(mut self, component: BamComponent) -> Self {
        self.hooks = self.hooks.with_required_handle(component);
        self
    }

    /// Sets the ABI `0.3` 64-bit FNV-1a structural schema fingerprint.
    #[inline]
    pub fn with_schema_fingerprint(mut self, fingerprint: u64) -> Self {
        self.hooks = self.hooks.with_schema_fingerprint(fingerprint);
        self
    }

    /// Sets static ABI `0.3` field reflection metadata (`ModComponent::BAM_FIELDS`).
    #[inline]
    pub fn with_fields(mut self, fields: &'static [BamFieldDesc]) -> Self {
        self.hooks = self.hooks.with_fields(fields);
        self
    }

    /// Appends an extended ABI `0.3` required-component initialization descriptor.
    #[inline]
    pub fn require_init_desc(mut self, desc: BamRequiredComponent) -> Self {
        self.hooks = self.hooks.with_required_init_desc(desc);
        self
    }

    /// Declares a pre-resolved required component initialized from POD `value` bytes
    /// (analogous to Bevy's `#[require(Comp = value)]`).
    #[inline]
    pub fn require_handle_with_value<R: AbiPod>(
        mut self,
        component: BamComponent,
        value: &R,
    ) -> Self {
        self.hooks = self.hooks.with_required_pod_value(component, value);
        self
    }

    /// Creates options pre-populated with ABI `0.3` `COMPONENT_FLAGS`, `SCHEMA_FINGERPRINT`,
    /// `BAM_FIELDS`, and `REQUIRED_COMPONENTS` from `T: ModComponent`.
    #[inline]
    pub fn from_mod_component() -> Self
    where
        T: ModComponent,
    {
        Self {
            flags: T::COMPONENT_FLAGS,
            hooks: GuestComponentHooks::<T>::from_mod_component(),
            required_resolvers: Vec::new(),
        }
    }
}

/// Safe per-World mod initialization builder passed to [`GuestPlugin::build`](crate::GuestPlugin::build)
/// and [`export_mod!`](crate::export_mod).
///
/// Constructing `GuestApp` directly from raw C pointers in safe Rust is rejected at compile time:
///
/// ```compile_fail
/// use bamabi_guest::GuestApp;
///
/// let _app = GuestApp::from_raw(core::ptr::null(), core::ptr::null_mut());
/// ```
///
/// Registering non-`AbiPod` types (such as `String` or `Vec<u8>`) via `register_component` is
/// rejected at compile time:
///
/// ```compile_fail
/// use bamabi_guest::GuestApp;
///
/// fn register_bad(app: &mut GuestApp<'_>) {
///     let _ = app.register_component::<String>();
/// }
/// ```
pub struct GuestApp<'a> {
    pub(crate) instance: &'a mut GuestInstance,
    pub(crate) load_ctx: ValidatedLoadCtx<'a>,
}

impl<'a> GuestApp<'a> {
    /// Wraps a validated [`GuestInstance`] and [`ValidatedLoadCtx`] for the duration of
    /// `bamabi_init`.
    #[inline]
    pub fn from_validated(instance: &'a mut GuestInstance, load_ctx: ValidatedLoadCtx<'a>) -> Self {
        Self { instance, load_ctx }
    }

    /// Returns the canonical manifest name of this mod instance.
    #[inline]
    pub fn manifest_name(&self) -> &str {
        self.instance.manifest_name()
    }

    /// Returns the cached [`Host`] table for this mod instance.
    #[inline]
    pub const fn host(&self) -> &Host {
        self.instance.host()
    }

    /// Returns the validated [`BamHostApi`] function table.
    #[inline]
    pub const fn host_api(&self) -> &BamHostApi {
        self.instance.host().api()
    }

    /// Returns the raw `*mut BamLoadCtx` pointer for low-level registration helpers.
    #[inline]
    pub const fn as_raw_load_ctx(&self) -> *mut BamLoadCtx {
        self.load_ctx.as_raw()
    }

    /// Borrows the underlying [`GuestInstance`] mutably (used by low-level `raw` helpers).
    #[inline]
    pub fn instance_mut(&mut self) -> &mut GuestInstance {
        self.instance
    }

    /// Reads the most recent human-readable UTF-8 diagnostic recorded on this world's host context.
    #[inline]
    pub fn last_error(&self) -> String {
        self.instance.host().last_error()
    }

    /// Resolves a custom `"bam.v0.<name>"` host symbol pointer.
    #[inline]
    pub fn resolve_custom_symbol(&self, symbol: &CStr) -> Result<*const c_void, FfiResult> {
        self.instance.host().resolve_custom_symbol(symbol)
    }

    /// Resolves a [`BamComponent`] handle for `T: GuestComponent`, caching validated handles in
    /// the owning [`GuestInstance`].
    ///
    /// Tries the manifest-qualified name first when `T::NAME` is unqualified, falling back to
    /// `(T::NAME, T::UUID)`.
    #[inline]
    pub fn resolve_component<T: GuestComponent>(&self) -> Result<BamComponent, FfiResult> {
        resolve_guest_component::<T>(self.instance.host(), self.instance.manifest_name())
    }

    /// Resolves `T: GuestComponent` and validates its registered layout against `T` into a
    /// [`TypedComponent<T>`], caching the layout-verified handle in the owning [`GuestInstance`].
    #[inline]
    pub fn resolve_component_typed<T: GuestComponent>(
        &self,
    ) -> Result<TypedComponent<T>, FfiResult> {
        let (_, uuid) = qualify_component_identity::<T>(self.instance.manifest_name())?;
        self.instance.host().cached_typed_component::<T>(uuid, || {
            resolve_guest_component_uncached::<T>(
                self.instance.host(),
                self.instance.manifest_name(),
            )
        })
    }

    /// Returns a cached [`TypedComponent<T>`] handle for `T: GuestComponent` without redundant
    /// per-call FFI round-trips after the first registration or resolution.
    #[inline]
    pub fn cached_component<T: GuestComponent>(&self) -> Result<TypedComponent<T>, FfiResult> {
        self.resolve_component_typed::<T>()
    }

    /// Resolves a [`BamComponent`] handle by explicit `name` and/or `uuid`.
    #[inline]
    pub fn resolve_component_named(
        &self,
        name: Option<&CStr>,
        uuid: Option<ComponentUuid>,
    ) -> Result<BamComponent, FfiResult> {
        resolve_component(self.instance.host(), name, uuid)
    }

    /// Registers a cross-language `T: GuestComponent` (`AbiPod`) dynamic component on the host
    /// `World`, returning its 1-based [`BamComponent`] handle.
    ///
    /// Unqualified `T::NAME` values (without `"::"`) are automatically prefixed with
    /// `"<manifest.name>::"`.
    #[inline]
    pub fn register_component<T: GuestComponent>(&mut self) -> Result<BamComponent, FfiResult> {
        self.register_component_with_flags::<T>(T::FLAGS)
    }

    /// Registers `T: GuestComponent` on the host `World` and validates its registered layout into
    /// a cached [`TypedComponent<T>`].
    #[inline]
    pub fn register_component_typed<T: GuestComponent>(
        &mut self,
    ) -> Result<TypedComponent<T>, FfiResult> {
        let _ = self.register_component::<T>()?;
        self.cached_component::<T>()
    }

    /// Registers `T: GuestComponent` with explicit `BAM_COMPONENT_FLAG_*` flags (e.g.
    /// `BAM_COMPONENT_FLAG_SPARSE_SET` or `BAM_COMPONENT_FLAG_IMMUTABLE`), propagating
    /// `T::SCHEMA_FINGERPRINT` and `T::BAM_FIELDS`.
    pub fn register_component_with_flags<T: GuestComponent>(
        &mut self,
        flags: u32,
    ) -> Result<BamComponent, FfiResult> {
        self.register_component_with_flags_and_hooks::<T>(flags, GuestComponentHooks::new())
    }

    /// Registers an `AbiPod` component under an explicit `name`, `uuid`, and `flags`.
    #[inline]
    pub fn register_pod_component_named<T: AbiPod>(
        &mut self,
        name: &CStr,
        uuid: ComponentUuid,
        flags: u32,
    ) -> Result<BamComponent, FfiResult> {
        let handle = self
            .load_ctx
            .register_pod_component::<T>(self.instance, name, uuid, flags)?;
        self.instance
            .host()
            .cache_component_handle::<T>(uuid, handle, true);
        Ok(handle)
    }

    /// Registers `T: GuestComponent` with optional `on_add` and `on_remove` hooks.
    #[inline]
    pub fn register_component_with_hooks<T: GuestComponent>(
        &mut self,
        hooks: GuestComponentHooks<T>,
    ) -> Result<BamComponent, FfiResult> {
        self.register_component_with_flags_and_hooks::<T>(T::FLAGS, hooks)
    }

    /// Registers `T: GuestComponent` with explicit `flags` and optional lifecycle hooks,
    /// propagating `T::SCHEMA_FINGERPRINT` / `T::BAM_FIELDS` and caching the validated handle.
    pub fn register_component_with_flags_and_hooks<T: GuestComponent>(
        &mut self,
        flags: u32,
        mut hooks: GuestComponentHooks<T>,
    ) -> Result<BamComponent, FfiResult> {
        let (qualified_name, uuid) =
            qualify_component_identity::<T>(self.instance.manifest_name())?;
        if hooks.inner.schema_fingerprint == 0 {
            hooks.inner.schema_fingerprint = T::SCHEMA_FINGERPRINT;
        }
        if hooks.inner.fields.is_empty() {
            hooks.inner.fields = T::BAM_FIELDS;
        }
        let handle = self.load_ctx.register_pod_component_with_hooks::<T>(
            self.instance,
            qualified_name.as_c_str(),
            uuid,
            flags,
            hooks.inner,
        )?;
        self.instance
            .host()
            .cache_component_handle::<T>(uuid, handle, true);
        Ok(handle)
    }

    /// Registers a cross-language `R: GuestComponent` (`AbiPod`) dynamic singleton resource via
    /// `BamHostApi::register_resource` (slot 73), caching its handle and propagating
    /// `R::SCHEMA_FINGERPRINT` and `R::BAM_FIELDS`.
    pub fn register_resource<R: GuestComponent>(&mut self) -> Result<BamComponent, FfiResult> {
        let (qualified_name, uuid) =
            qualify_component_identity::<R>(self.instance.manifest_name())?;
        let mut hooks = bamabi_guest_sys::PodComponentHooks::<R>::new();
        hooks.schema_fingerprint = R::SCHEMA_FINGERPRINT;
        hooks.fields = R::BAM_FIELDS;
        let handle = self.load_ctx.register_pod_resource::<R>(
            self.instance,
            qualified_name.as_c_str(),
            uuid,
            R::FLAGS,
            hooks,
        )?;
        self.instance
            .host()
            .cache_component_handle::<R>(uuid, handle, true);
        Ok(handle)
    }

    /// Registers `R: GuestComponent` via `BamHostApi::register_resource` (slot 73) and returns
    /// its cached [`TypedComponent<R>`].
    #[inline]
    pub fn register_resource_typed<R: GuestComponent>(
        &mut self,
    ) -> Result<TypedComponent<R>, FfiResult> {
        let _ = self.register_resource::<R>()?;
        self.cached_component::<R>()
    }

    /// Registers a cross-language `M: GuestComponent` (`AbiPod`) dynamic buffered message type via
    /// `BamHostApi::register_message` (slot 74), caching its handle and propagating
    /// `M::SCHEMA_FINGERPRINT` and `M::BAM_FIELDS`.
    pub fn register_message<M: GuestComponent>(&mut self) -> Result<BamComponent, FfiResult> {
        let (qualified_name, uuid) =
            qualify_component_identity::<M>(self.instance.manifest_name())?;
        let mut hooks = bamabi_guest_sys::PodComponentHooks::<M>::new();
        hooks.schema_fingerprint = M::SCHEMA_FINGERPRINT;
        hooks.fields = M::BAM_FIELDS;
        let handle = self.load_ctx.register_pod_message::<M>(
            self.instance,
            qualified_name.as_c_str(),
            uuid,
            M::FLAGS,
            hooks,
        )?;
        self.instance
            .host()
            .cache_component_handle::<M>(uuid, handle, true);
        Ok(handle)
    }

    /// Registers `M: GuestComponent` via `BamHostApi::register_message` (slot 74) and returns
    /// its cached [`TypedComponent<M>`].
    #[inline]
    pub fn register_message_typed<M: GuestComponent>(
        &mut self,
    ) -> Result<TypedComponent<M>, FfiResult> {
        let _ = self.register_message::<M>()?;
        self.cached_component::<M>()
    }

    /// Registers `T: GuestComponent` with [`GuestComponentOptions`] (`summary_tick`, `clone`,
    /// full lifecycle hooks, and transitive `require::<R>()`).
    pub fn register_component_with_options<T: GuestComponent>(
        &mut self,
        mut options: GuestComponentOptions<T>,
    ) -> Result<BamComponent, FfiResult> {
        let host = *self.instance.host();
        let manifest_name = self.instance.manifest_name();
        for resolver in options.required_resolvers {
            let handle = resolver(&host, manifest_name)?;
            if !options.hooks.inner.required_components.contains(&handle) {
                options.hooks.inner.required_components.push(handle);
            }
        }
        let combined_flags = T::FLAGS | options.flags;
        self.register_component_with_flags_and_hooks::<T>(combined_flags, options.hooks)
    }

    /// Registers a runtime layout-defined dynamic component (`name`, `size`, `align`, `drop_fn`)
    /// backed by `ComponentDescriptor::new_with_layout` on the host, allowing mods to attach private
    /// `#[repr(C)]` state or custom dynamic events without recompiling the host.
    pub fn register_component_with_layout(
        &mut self,
        name: &str,
        size: usize,
        align: usize,
        drop_fn: Option<ComponentDropFn>,
    ) -> Result<BamComponent, FfiResult> {
        self.load_ctx
            .register_component_with_layout(self.instance, name, size, align, drop_fn)
    }

    /// Registers a global dynamic observer for `E: GuestComponent` events triggered via
    /// `world.trigger(...)` or `commands.trigger(...)`.
    pub fn observe<E, F>(&mut self, name: &str, mut callback: F) -> Result<&mut Self, FfiResult>
    where
        E: GuestComponent,
        F: FnMut(&E, &mut GuestCommands<'_>) + Send + 'static,
    {
        let event = self.resolve_component::<E>()?;
        self.observe_handle::<E, _>(name, event, FfiEntity::NULL, move |_target, ev, cmds| {
            callback(ev, cmds);
        })
    }

    /// Registers an entity-scoped dynamic observer for `EntityEvent` `E: GuestComponent` on
    /// `target` (or `FfiEntity::NULL` to watch all targets while receiving the event target entity).
    pub fn observe_entity<E, F>(
        &mut self,
        name: &str,
        target: FfiEntity,
        mut callback: F,
    ) -> Result<&mut Self, FfiResult>
    where
        E: GuestComponent,
        F: FnMut(FfiEntity, &E, &mut GuestCommands<'_>) + Send + 'static,
    {
        let event = self.resolve_component::<E>()?;
        self.observe_handle::<E, _>(name, event, target, move |tgt, ev, cmds| {
            callback(tgt, ev, cmds);
        })
    }

    /// Registers a dynamic observer for a pre-resolved event handle `event` (`FfiEntity::NULL`
    /// for global, or a specific `target` entity).
    pub fn observe_handle<E, F>(
        &mut self,
        name: &str,
        event: BamComponent,
        target: FfiEntity,
        mut callback: F,
    ) -> Result<&mut Self, FfiResult>
    where
        E: AbiPod,
        F: FnMut(FfiEntity, &E, &mut GuestCommands<'_>) + Send + 'static,
    {
        let status = self.load_ctx.register_observer::<E, _>(
            self.instance,
            name,
            event,
            target,
            move |tgt, ev, validated_cmds| {
                with_guest_commands(validated_cmds, |cmds| callback(tgt, ev, cmds));
                FfiResult::OK
            },
        );
        if status.is_ok() {
            Ok(self)
        } else {
            Err(status)
        }
    }

    /// Registers a hot-reload state snapshot callback invoked prior to `bamabi_shutdown`.
    #[inline]
    pub fn on_save_state<F>(&mut self, hook: F)
    where
        F: FnMut() -> Result<Vec<u8>, FfiResult> + Send + 'static,
    {
        self.instance.set_save_state_hook(hook);
    }

    /// Registers a hot-reload state restoration callback invoked after `bamabi_init`.
    #[inline]
    pub fn on_restore_state<F>(&mut self, hook: F)
    where
        F: FnMut(&[u8]) -> FfiResult + Send + 'static,
    {
        self.instance.set_restore_state_hook(hook);
    }

    /// Allocates guest-owned data inside this per-World [`GuestInstance`], returning a stable
    /// `*mut T` valid until `bamabi_shutdown` (or freed immediately on init failure).
    #[inline]
    pub fn alloc_user_data<T: Send + 'static>(&mut self, value: T) -> *mut T {
        self.instance.track_allocation(Box::new(value))
    }

    /// Registers a per-instance teardown closure executed during `bamabi_shutdown` in LIFO
    /// order before closure allocations are freed.
    #[inline]
    pub fn on_shutdown<F>(&mut self, cleanup: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.instance.add_shutdown_hook(cleanup);
    }

    /// Starts building a unified guest ECS system named `name`.
    #[inline]
    pub fn system<'b>(&'b mut self, name: &'b CStr) -> SystemBuilder<'b, 'a> {
        SystemBuilder {
            app: self,
            name,
            queries: Vec::new(),
            query_param_set_groups: Vec::new(),
            next_param_set_group: 1,
            system_resources: Vec::new(),
            schedule: ScheduleId::UPDATE,
            flags: 0,
            after: Vec::new(),
            before: Vec::new(),
            in_sets: Vec::new(),
            after_weak: Vec::new(),
            before_weak: Vec::new(),
            ambiguous_with: Vec::new(),
            run_if_preds: Vec::new(),
            run_if_ctx_terms: Vec::new(),
            run_if_ctx_preds: Vec::new(),
            first_error: None,
        }
    }
}

/// Generates typed [`QueryBuilder`] helper methods that resolve a `GuestComponent` and append a
/// term with the corresponding [`AccessMode`].
macro_rules! define_query_guest_terms {
    ($( $(#[$meta:meta])* $method:ident <$ty:ident> => $mode:ident );* $(;)?) => {
        $(
            $(#[$meta])*
            #[inline]
            pub fn $method<$ty: GuestComponent>(mut self) -> Self {
                self.push_guest_term::<$ty>(AccessMode::$mode);
                self
            }
        )*
    };
}

/// Generates handle-based [`QueryBuilder`] helper methods that append a pre-resolved
/// [`BamComponent`] handle with the corresponding [`AccessMode`].
macro_rules! define_query_handle_terms {
    ($( $(#[$meta:meta])* $method:ident => $mode:ident );* $(;)?) => {
        $(
            $(#[$meta])*
            #[inline]
            pub fn $method(mut self, component: BamComponent) -> Self {
                self.push_term(component, AccessMode::$mode);
                self
            }
        )*
    };
}

/// Declarative query-clause builder passed to [`SystemBuilder::query`].
pub struct QueryBuilder<'b> {
    pub(crate) host: &'b Host,
    pub(crate) manifest_name: &'b str,
    pub(crate) terms: Vec<BamTermDesc>,
    pub(crate) first_error: Option<FfiResult>,
}

impl<'b> QueryBuilder<'b> {
    /// Creates an empty [`QueryBuilder`] bound to `host` and `manifest_name`.
    #[inline]
    pub(crate) fn new(host: &'b Host, manifest_name: &'b str) -> Self {
        Self {
            host,
            manifest_name,
            terms: Vec::new(),
            first_error: None,
        }
    }

    /// Finalizes this builder into its validated `Vec<BamTermDesc>` list.
    #[inline]
    pub(crate) fn into_terms(self) -> Result<Vec<BamTermDesc>, FfiResult> {
        if let Some(err) = self.first_error {
            Err(err)
        } else {
            Ok(self.terms)
        }
    }

    /// Latches the first builder error encountered so fluent chaining can finish cleanly before
    /// propagating the error during [`SystemBuilder::build`].
    #[inline]
    pub(crate) fn record_error(&mut self, err: FfiResult) {
        if self.first_error.is_none() {
            self.first_error = Some(err);
        }
    }

    /// Validates `component`, `mode`, and the `MAX_TERMS_PER_QUERY` cap before appending a
    /// [`BamTermDesc`] to the query's term list.
    #[inline]
    pub(crate) fn push_term(&mut self, component: BamComponent, mode: AccessMode) {
        self.push_term_with_flags(component, mode, 0);
    }

    /// Validates `component`, `mode`, `flags`, and the `MAX_TERMS_PER_QUERY` cap before appending
    /// a [`BamTermDesc`] with explicit `BAM_TERM_FLAG_*` bits.
    #[inline]
    pub(crate) fn push_term_with_flags(
        &mut self,
        component: BamComponent,
        mode: AccessMode,
        flags: u16,
    ) {
        if !component.is_valid() {
            self.record_error(FfiResult::INVALID_COMPONENT);
            return;
        }
        if !mode.is_valid() || (flags & !BAM_TERM_KNOWN_FLAGS) != 0 {
            self.record_error(FfiResult::INVALID_ARGUMENT);
            return;
        }
        if self.terms.len() >= MAX_TERMS_PER_QUERY {
            self.record_error(FfiResult::CAPACITY_EXCEEDED);
            return;
        }
        self.terms
            .push(BamTermDesc::new(component, mode).with_flags(flags));
    }

    /// Resolves `T: GuestComponent` on the host (trying manifest-qualified identity first) and
    /// appends it with `mode`, latching any resolution error on the builder.
    #[inline]
    fn push_guest_term<T: GuestComponent>(&mut self, mode: AccessMode) {
        match resolve_guest_component::<T>(self.host, self.manifest_name) {
            Ok(comp) => self.push_term(comp, mode),
            Err(err) => self.record_error(err),
        }
    }

    /// Resolves `T: GuestComponent` on the host and appends it with `mode` and explicit
    /// `BAM_TERM_FLAG_*` flags.
    #[inline]
    fn push_guest_term_with_flags<T: GuestComponent>(&mut self, mode: AccessMode, flags: u16) {
        match resolve_guest_component::<T>(self.host, self.manifest_name) {
            Ok(comp) => self.push_term_with_flags(comp, mode, flags),
            Err(err) => self.record_error(err),
        }
    }

    define_query_guest_terms! {
        /// Appends a shared read-only (`&T`) entity column term.
        read<T> => READ;
        /// Appends an exclusive mutable (`&mut T`) entity column term.
        write<T> => WRITE;
        /// Appends an archetype presence filter (`With<T>`) term.
        with<T> => FILTER_ONLY;
        /// Appends an archetype absence filter (`Without<T>`) term.
        without<T> => WITHOUT;
        /// Appends an optional shared read-only (`Option<&T>`) column term.
        optional_read<T> => OPTIONAL_READ;
        /// Appends an optional exclusive mutable (`Option<&mut T>`) column term.
        optional_write<T> => OPTIONAL_WRITE;
        /// Appends a row-level change-tick filter with read-only column access (`Changed<T> + &T`).
        changed<T> => CHANGED;
        /// Appends a row-level added-tick filter with read-only column access (`Added<T> + &T`).
        added<T> => ADDED;
        /// Appends an entity-disabling filter bypass (`Allow<T>`) term.
        allow<T> => ALLOW;
        /// Appends an archetypal presence indicator (`Has<T>`) term yielding an `FfiBool` column.
        has<T> => HAS;
        /// Appends a `RemovedComponents<T>` removal-stream reader (`AccessMode::REMOVED_READ`) term.
        removed<T> => REMOVED_READ;
        /// Appends a shared singleton resource read (`Res<R>`) term to this query.
        resource_read<R> => RESOURCE_READ;
        /// Appends an exclusive singleton resource write (`ResMut<R>`) term to this query.
        resource_write<R> => RESOURCE_WRITE;
        /// Appends a buffered message stream reader (`MessageReader<M>`) term.
        message_read<M> => MESSAGE_READ;
        /// Appends a buffered message stream writer (`MessageWriter<M>`) term.
        message_write<M> => MESSAGE_WRITE;
    }

    /// Appends a disjunctive `Or<(...)>` filter group (`group_id`, `1..=255`) whose terms are
    /// configured via `configure`.
    pub fn or_group(
        mut self,
        group_id: u8,
        configure: impl for<'q> FnOnce(QueryBuilder<'q>) -> QueryBuilder<'q>,
    ) -> Self {
        let sub = QueryBuilder {
            host: self.host,
            manifest_name: self.manifest_name,
            terms: Vec::new(),
            first_error: None,
        };
        let finished = configure(sub);
        if let Some(err) = finished.first_error {
            self.record_error(err);
            return self;
        }
        for mut term in finished.terms {
            if self.terms.len() >= MAX_TERMS_PER_QUERY {
                self.record_error(FfiResult::CAPACITY_EXCEEDED);
                break;
            }
            term.or_group = group_id;
            self.terms.push(term);
        }
        self
    }

    /// Appends a pre-resolved [`BamComponent`] handle with explicit [`AccessMode`].
    #[inline]
    pub fn term(mut self, component: BamComponent, mode: AccessMode) -> Self {
        self.push_term(component, mode);
        self
    }

    /// Appends a pre-resolved [`BamComponent`] handle with explicit [`AccessMode`] and
    /// `BAM_TERM_FLAG_*` flags (such as `BAM_TERM_FLAG_CHANGED` or `BAM_TERM_FLAG_ADDED`).
    #[inline]
    pub fn term_with_flags(
        mut self,
        component: BamComponent,
        mode: AccessMode,
        flags: u16,
    ) -> Self {
        self.push_term_with_flags(component, mode, flags);
        self
    }

    /// Appends an exclusive mutable column term filtered by `Changed<T>`
    /// (`AccessMode::WRITE` + `BAM_TERM_FLAG_CHANGED`).
    #[inline]
    pub fn changed_write<T: GuestComponent>(mut self) -> Self {
        self.push_guest_term_with_flags::<T>(AccessMode::WRITE, bamabi_sys::BAM_TERM_FLAG_CHANGED);
        self
    }

    /// Appends an exclusive mutable column term filtered by `Added<T>`
    /// (`AccessMode::WRITE` + `BAM_TERM_FLAG_ADDED`).
    #[inline]
    pub fn added_write<T: GuestComponent>(mut self) -> Self {
        self.push_guest_term_with_flags::<T>(AccessMode::WRITE, bamabi_sys::BAM_TERM_FLAG_ADDED);
        self
    }

    /// Appends a pure change-detection filter (`AccessMode::FILTER_ONLY` +
    /// `BAM_TERM_FLAG_CHANGED`) without borrowing `T`'s column data.
    #[inline]
    pub fn with_changed<T: GuestComponent>(mut self) -> Self {
        self.push_guest_term_with_flags::<T>(
            AccessMode::FILTER_ONLY,
            bamabi_sys::BAM_TERM_FLAG_CHANGED,
        );
        self
    }

    /// Appends a pure added-detection filter (`AccessMode::FILTER_ONLY` + `BAM_TERM_FLAG_ADDED`)
    /// without borrowing `T`'s column data.
    #[inline]
    pub fn with_added<T: GuestComponent>(mut self) -> Self {
        self.push_guest_term_with_flags::<T>(
            AccessMode::FILTER_ONLY,
            bamabi_sys::BAM_TERM_FLAG_ADDED,
        );
        self
    }

    define_query_handle_terms! {
        /// Appends a pre-resolved shared read-only (`&T`) handle term.
        read_handle => READ;
        /// Appends a pre-resolved exclusive mutable (`&mut T`) handle term.
        write_handle => WRITE;
        /// Appends a pre-resolved `With<T>` filter handle term.
        with_handle => FILTER_ONLY;
        /// Appends a pre-resolved `Without<T>` filter handle term.
        without_handle => WITHOUT;
        /// Appends a pre-resolved `Option<&T>` handle term.
        optional_read_handle => OPTIONAL_READ;
        /// Appends a pre-resolved `Option<&mut T>` handle term.
        optional_write_handle => OPTIONAL_WRITE;
        /// Appends a pre-resolved `Changed<T>` handle term.
        changed_handle => CHANGED;
        /// Appends a pre-resolved `Added<T>` handle term.
        added_handle => ADDED;
        /// Appends a pre-resolved `Allow<T>` disabling-filter bypass handle term.
        allow_handle => ALLOW;
        /// Appends a pre-resolved `Has<T>` archetypal presence handle term.
        has_handle => HAS;
        /// Appends a pre-resolved `RemovedComponents<T>` removal-stream handle term.
        removed_handle => REMOVED_READ;
        /// Appends a pre-resolved `Res<R>` handle term.
        resource_read_handle => RESOURCE_READ;
        /// Appends a pre-resolved `ResMut<R>` handle term.
        resource_write_handle => RESOURCE_WRITE;
        /// Appends a pre-resolved `MessageReader<M>` handle term.
        message_read_handle => MESSAGE_READ;
        /// Appends a pre-resolved `MessageWriter<M>` handle term.
        message_write_handle => MESSAGE_WRITE;
    }

    /// Appends `Allow<Disabled>` so this query also visits Bevy `0.20` disabled entities.
    #[inline]
    pub fn allow_disabled(self) -> Self {
        self.allow::<FfiDisabled>()
    }

    /// Appends `With<Disabled>` to match only Bevy `0.20` disabled entities.
    #[inline]
    pub fn with_disabled(self) -> Self {
        self.with::<FfiDisabled>()
    }

    /// Appends `Without<Disabled>` to explicitly exclude Bevy `0.20` disabled entities.
    #[inline]
    pub fn without_disabled(self) -> Self {
        self.without::<FfiDisabled>()
    }

    /// Appends `Has<Disabled>` to query whether each chunk's archetype carries `Disabled`.
    #[inline]
    pub fn has_disabled(self) -> Self {
        self.has::<FfiDisabled>()
    }

    /// Resolves `name` (with optional `uuid`) on the host and appends a term with `mode`.
    #[inline]
    pub fn term_named(
        mut self,
        name: &CStr,
        uuid: Option<ComponentUuid>,
        mode: AccessMode,
    ) -> Self {
        match resolve_component(self.host, Some(name), uuid) {
            Ok(comp) => self.push_term(comp, mode),
            Err(err) => self.record_error(err),
        }
        self
    }
}

/// Scoped builder returned inside [`SystemBuilder::param_set`] for grouping mutually exclusive
/// queries under a shared non-zero `param_set_group` ID (mirroring Bevy's `ParamSet<(Q0, Q1, ...)>`).
pub struct ParamSetBuilder<'p, 'b, 'a> {
    builder: &'p mut SystemBuilder<'b, 'a>,
    group_id: u32,
}

impl<'b, 'a> ParamSetBuilder<'_, 'b, 'a> {
    /// Returns the shared non-zero `param_set_group` ID assigned to every query added in this scope.
    #[inline]
    pub const fn group_id(&self) -> u32 {
        self.group_id
    }

    /// Appends a [`QueryBuilder`]-configured query clause belonging to this `ParamSet` group.
    pub fn query<F>(&mut self, configure: F) -> &mut Self
    where
        F: for<'q> FnOnce(QueryBuilder<'q>) -> QueryBuilder<'q>,
    {
        self.builder.push_query_with_group(configure, self.group_id);
        self
    }

    /// Appends a typed [`QueryTuple`] query clause belonging to this `ParamSet` group.
    pub fn query_typed<Q: QueryTuple>(&mut self) -> &mut Self {
        self.builder.push_query_typed_with_group::<Q>(self.group_id);
        self
    }
}

/// Unified fluent builder compiling queries, resources, schedule selection, ordering sets,
/// main-thread pinning, and optional `run_if` / `run_if_with` predicates into a single
/// [`BamSystemDesc`](bamabi_sys::BamSystemDesc).
pub struct SystemBuilder<'b, 'a> {
    app: &'b mut GuestApp<'a>,
    name: &'b CStr,
    queries: Vec<Vec<BamTermDesc>>,
    query_param_set_groups: Vec<u32>,
    next_param_set_group: u32,
    system_resources: Vec<BamTermDesc>,
    schedule: ScheduleId,
    flags: u32,
    after: Vec<&'b CStr>,
    before: Vec<&'b CStr>,
    in_sets: Vec<&'b CStr>,
    after_weak: Vec<&'b CStr>,
    before_weak: Vec<&'b CStr>,
    ambiguous_with: Vec<&'b CStr>,
    run_if_preds: Vec<RunIfPredicate>,
    run_if_ctx_terms: Vec<BamTermDesc>,
    run_if_ctx_preds: Vec<(usize, RunIfCtxPredicate)>,
    first_error: Option<FfiResult>,
}

/// Shared non-`N`-generic system callback wrapper so user closures `F` are not monomorphized 4×
/// across bucketed trampolines (`N ∈ {0, 1, 4, 16}`).
fn invoke_system_callback<F, R>(callback: &mut F, ctx: &mut SystemContext<'_>) -> FfiResult
where
    F: FnMut(&mut SystemContext<'_>) -> R,
    R: IntoSystemResult,
{
    callback(ctx).into_ffi_result()
}

/// Abstracts `build` (`FfiResult`) vs `register_handle` (`Result<BamSystem, FfiResult>`) submission
/// over const-bucketed `N ∈ {0, 1, 4, 16}` slice trampolines.
trait SliceSystemSubmitter<Out> {
    fn submit<const N: usize, S>(
        self,
        load_ctx: &mut ValidatedLoadCtx<'_>,
        instance: &mut GuestInstance,
        config: &SystemRegistrationConfig<'_>,
        run_if: Option<PreparedRunIf>,
        closure: S,
    ) -> Out
    where
        S: FnMut(&mut ValidatedSystemSlice<'_>) -> FfiResult + Send + 'static;
}

struct BuildSubmitter;

impl SliceSystemSubmitter<FfiResult> for BuildSubmitter {
    fn submit<const N: usize, S>(
        self,
        load_ctx: &mut ValidatedLoadCtx<'_>,
        instance: &mut GuestInstance,
        config: &SystemRegistrationConfig<'_>,
        run_if: Option<PreparedRunIf>,
        closure: S,
    ) -> FfiResult
    where
        S: FnMut(&mut ValidatedSystemSlice<'_>) -> FfiResult + Send + 'static,
    {
        load_ctx.register_system_slice::<N, S>(instance, config, run_if, closure)
    }
}

struct HandleSubmitter;

impl SliceSystemSubmitter<Result<BamSystem, FfiResult>> for HandleSubmitter {
    fn submit<const N: usize, S>(
        self,
        load_ctx: &mut ValidatedLoadCtx<'_>,
        instance: &mut GuestInstance,
        config: &SystemRegistrationConfig<'_>,
        run_if: Option<PreparedRunIf>,
        closure: S,
    ) -> Result<BamSystem, FfiResult>
    where
        S: FnMut(&mut ValidatedSystemSlice<'_>) -> FfiResult + Send + 'static,
    {
        load_ctx.register_system_handle_slice::<N, S>(instance, config, run_if, closure)
    }
}

impl<'b, 'a> SystemBuilder<'b, 'a> {
    /// Latches the first resolution or capacity error encountered during fluent system building.
    #[inline]
    fn record_error(&mut self, err: FfiResult) {
        if self.first_error.is_none() {
            self.first_error = Some(err);
        }
    }

    /// Internal helper appending a `QueryBuilder` query with `param_set_group` (`0` = standalone).
    fn push_query_with_group<F>(&mut self, configure: F, param_set_group: u32)
    where
        F: for<'q> FnOnce(QueryBuilder<'q>) -> QueryBuilder<'q>,
    {
        if self.queries.len() >= MAX_QUERIES_PER_SYSTEM {
            self.record_error(FfiResult::CAPACITY_EXCEEDED);
            return;
        }
        let host = *self.app.host();
        let qb = QueryBuilder {
            host: &host,
            manifest_name: host.manifest_name(),
            terms: Vec::new(),
            first_error: None,
        };
        let finished = configure(qb);
        if let Some(err) = finished.first_error {
            self.record_error(err);
        } else {
            self.queries.push(finished.terms);
            self.query_param_set_groups.push(param_set_group);
        }
    }

    /// Internal helper appending a typed `QueryTuple` query with `param_set_group` (`0` = standalone).
    fn push_query_typed_with_group<Q: QueryTuple>(&mut self, param_set_group: u32) {
        if self.queries.len() >= MAX_QUERIES_PER_SYSTEM {
            self.record_error(FfiResult::CAPACITY_EXCEEDED);
            return;
        }
        let host = *self.app.host();
        match Q::collect_descs(&host, host.manifest_name()) {
            Ok(terms) => {
                self.queries.push(terms);
                self.query_param_set_groups.push(param_set_group);
            }
            Err(err) => self.record_error(err),
        }
    }

    /// Appends a query clause (`0..=MAX_QUERIES_PER_SYSTEM`) configured via [`QueryBuilder`].
    pub fn query<F>(mut self, configure: F) -> Self
    where
        F: for<'q> FnOnce(QueryBuilder<'q>) -> QueryBuilder<'q>,
    {
        self.push_query_with_group(configure, 0);
        self
    }

    /// Appends a typed query clause derived from [`QueryTuple`] `Q`, matching
    /// [`SystemContext::single_query`] / [`SystemContext::query_typed`] without magic column indices.
    pub fn query_typed<Q: QueryTuple>(mut self) -> Self {
        self.push_query_typed_with_group::<Q>(0);
        self
    }

    /// Groups mutually exclusive queries declared inside `f` under a shared non-zero
    /// `param_set_group` ID (mirroring Bevy's `ParamSet`), allowing conflicting component accesses
    /// across queries within the same group.
    pub fn param_set(mut self, f: impl FnOnce(&mut ParamSetBuilder<'_, 'b, 'a>)) -> Self {
        let group_id = self.next_param_set_group;
        let Some(next) = self.next_param_set_group.checked_add(1) else {
            self.record_error(FfiResult::CAPACITY_EXCEEDED);
            return self;
        };
        self.next_param_set_group = next;
        let mut ps = ParamSetBuilder {
            builder: &mut self,
            group_id,
        };
        f(&mut ps);
        self
    }

    /// Returns the `param_set_group` ID assigned to each declared query (`0` for standalone
    /// queries, non-zero for queries declared inside [`Self::param_set`]).
    #[inline]
    pub fn param_set_groups(&self) -> &[u32] {
        &self.query_param_set_groups
    }

    /// Validates `component` and the `MAX_TERMS_PER_QUERY` cap before appending a system-level
    /// singleton resource or message stream term.
    #[inline]
    fn push_system_resource_handle(&mut self, component: BamComponent, mode: AccessMode) {
        if !component.is_valid() {
            self.record_error(FfiResult::INVALID_COMPONENT);
            return;
        }
        if self.system_resources.len() >= MAX_TERMS_PER_QUERY {
            self.record_error(FfiResult::CAPACITY_EXCEEDED);
            return;
        }
        self.system_resources
            .push(BamTermDesc::new(component, mode));
    }

    /// Declares a shared singleton resource read (`Res<R>`), accessible via
    /// `ctx.resource::<R>(decl_index)` in declaration order (`0, 1, ...`).
    pub fn resource_read<R: GuestComponent>(mut self) -> Self {
        match self.app.resolve_component::<R>() {
            Ok(comp) => self.push_system_resource_handle(comp, AccessMode::RESOURCE_READ),
            Err(err) => self.record_error(err),
        }
        self
    }

    /// Declares an exclusive singleton resource write (`ResMut<R>`), accessible via
    /// `ctx.resource_mut::<R>(decl_index)` in declaration order (`0, 1, ...`).
    pub fn resource_write<R: GuestComponent>(mut self) -> Self {
        match self.app.resolve_component::<R>() {
            Ok(comp) => self.push_system_resource_handle(comp, AccessMode::RESOURCE_WRITE),
            Err(err) => self.record_error(err),
        }
        self
    }

    /// Declares a shared singleton resource read using a pre-resolved [`BamComponent`] handle.
    #[inline]
    pub fn resource_read_handle(mut self, component: BamComponent) -> Self {
        self.push_system_resource_handle(component, AccessMode::RESOURCE_READ);
        self
    }

    /// Declares an exclusive singleton resource write using a pre-resolved [`BamComponent`] handle.
    #[inline]
    pub fn resource_write_handle(mut self, component: BamComponent) -> Self {
        self.push_system_resource_handle(component, AccessMode::RESOURCE_WRITE);
        self
    }

    /// Declares a buffered message stream reader (`MessageReader<M>`) accessible via
    /// [`SystemContext::read_messages`] at declaration index (`0, 1, ...`).
    pub fn message_read<M: GuestComponent>(mut self) -> Self {
        match self.app.resolve_component::<M>() {
            Ok(comp) => self.push_system_resource_handle(comp, AccessMode::MESSAGE_READ),
            Err(err) => self.record_error(err),
        }
        self
    }

    /// Declares a buffered message stream writer (`MessageWriter<M>`) accessible via
    /// [`SystemContext::send_message`] at declaration index (`0, 1, ...`).
    pub fn message_write<M: GuestComponent>(mut self) -> Self {
        match self.app.resolve_component::<M>() {
            Ok(comp) => self.push_system_resource_handle(comp, AccessMode::MESSAGE_WRITE),
            Err(err) => self.record_error(err),
        }
        self
    }

    /// Declares a buffered message stream reader using a pre-resolved [`BamComponent`] handle.
    #[inline]
    pub fn message_read_handle(mut self, component: BamComponent) -> Self {
        self.push_system_resource_handle(component, AccessMode::MESSAGE_READ);
        self
    }

    /// Declares a buffered message stream writer using a pre-resolved [`BamComponent`] handle.
    #[inline]
    pub fn message_write_handle(mut self, component: BamComponent) -> Self {
        self.push_system_resource_handle(component, AccessMode::MESSAGE_WRITE);
        self
    }

    /// Declares a shared singleton resource read (`Res<R>`) for a dynamic [`ModComponent`] `R`.
    pub fn resource_read_dynamic<R: ModComponent>(mut self) -> Self {
        match resolve_mod_component::<R>(self.app.host()) {
            Ok(comp) => self.push_system_resource_handle(comp, AccessMode::RESOURCE_READ),
            Err(err) => self.record_error(err),
        }
        self
    }

    /// Declares an exclusive singleton resource write (`ResMut<R>`) for a dynamic [`ModComponent`] `R`.
    pub fn resource_write_dynamic<R: ModComponent>(mut self) -> Self {
        match resolve_mod_component::<R>(self.app.host()) {
            Ok(comp) => self.push_system_resource_handle(comp, AccessMode::RESOURCE_WRITE),
            Err(err) => self.record_error(err),
        }
        self
    }

    /// Declares a buffered message stream reader (`MessageReader<M>`) for a dynamic
    /// [`ModComponent`] `M`, accessible via [`SystemContext::read_non_pod_messages`].
    pub fn message_read_dynamic<M: ModComponent>(mut self) -> Self {
        match resolve_mod_component::<M>(self.app.host()) {
            Ok(comp) => self.push_system_resource_handle(comp, AccessMode::MESSAGE_READ),
            Err(err) => self.record_error(err),
        }
        self
    }

    /// Declares a buffered message stream writer (`MessageWriter<M>`) for a dynamic
    /// [`ModComponent`] `M`, accessible via [`SystemContext::send_non_pod_message`].
    pub fn message_write_dynamic<M: ModComponent>(mut self) -> Self {
        match resolve_mod_component::<M>(self.app.host()) {
            Ok(comp) => self.push_system_resource_handle(comp, AccessMode::MESSAGE_WRITE),
            Err(err) => self.record_error(err),
        }
        self
    }

    /// Declares a system-level `RemovedComponents<T>` stream (`AccessMode::REMOVED_READ`) for
    /// `T: GuestComponent`, accessible via [`SystemContext::read_removed`] in declaration order.
    pub fn removed<T: GuestComponent>(mut self) -> Self {
        match self.app.resolve_component::<T>() {
            Ok(comp) => self.push_system_resource_handle(comp, AccessMode::REMOVED_READ),
            Err(err) => self.record_error(err),
        }
        self
    }

    /// Declares a system-level `RemovedComponents` stream using a pre-resolved [`BamComponent`]
    /// handle.
    #[inline]
    pub fn removed_handle(mut self, component: BamComponent) -> Self {
        self.push_system_resource_handle(component, AccessMode::REMOVED_READ);
        self
    }

    /// Selects the target Bevy [`ScheduleId`] (default [`ScheduleId::UPDATE`]).
    #[inline]
    pub fn schedule(mut self, schedule: ScheduleId) -> Self {
        self.schedule = schedule;
        self
    }

    /// Orders this system to run after `target_set_name` (`"system"` within the same mod or
    /// `"other_mod::system"` across mods).
    #[inline]
    pub fn after(mut self, target_set_name: &'b CStr) -> Self {
        self.after.push(target_set_name);
        self
    }

    /// Orders this system to run before `target_set_name`.
    #[inline]
    pub fn before(mut self, target_set_name: &'b CStr) -> Self {
        self.before.push(target_set_name);
        self
    }

    /// Places this system into `target_set_name` (`"host::<name>"` or custom mod set).
    #[inline]
    pub fn in_set(mut self, target_set_name: &'b CStr) -> Self {
        self.in_sets.push(target_set_name);
        self
    }

    /// Weakly orders this system to run after `target_set_name` when that target exists in the
    /// schedule (silently ignored when the optional target mod/set is absent, even under `strict`).
    #[inline]
    pub fn after_weak(mut self, target_set_name: &'b CStr) -> Self {
        self.after_weak.push(target_set_name);
        self
    }

    /// Weakly orders this system to run before `target_set_name` when that target exists in the
    /// schedule (silently ignored when the optional target mod/set is absent, even under `strict`).
    #[inline]
    pub fn before_weak(mut self, target_set_name: &'b CStr) -> Self {
        self.before_weak.push(target_set_name);
        self
    }

    /// Suppresses schedule ambiguity warnings/errors between this system and `target_set_name`.
    #[inline]
    pub fn ambiguous_with(mut self, target_set_name: &'b CStr) -> Self {
        self.ambiguous_with.push(target_set_name);
        self
    }

    /// Suppresses schedule ambiguity warnings/errors between this system and all other systems in
    /// its schedule (`BAM_SYSTEM_FLAG_AMBIGUOUS_WITH_ALL`).
    #[inline]
    pub fn ambiguous_with_all(mut self) -> Self {
        self.flags |= BAM_SYSTEM_FLAG_AMBIGUOUS_WITH_ALL;
        self
    }

    /// Pins this system onto Bevy's main thread (`BAM_SYSTEM_FLAG_MAIN_THREAD`).
    #[inline]
    pub fn main_thread(mut self) -> Self {
        self.flags |= BAM_SYSTEM_FLAG_MAIN_THREAD;
        self
    }

    /// Sets explicit `BAM_SYSTEM_FLAG_*` flags on this system descriptor.
    #[inline]
    pub fn flags(mut self, flags: u32) -> Self {
        self.flags |= flags;
        self
    }

    /// Appends a run-condition predicate `|ctx: &RunConditionContext<'_>| -> bool`.
    ///
    /// Multiple `.run_if(...)` / `.run_if_fn(...)` calls compose with short-circuit logical AND
    /// semantics in registration order (without overwriting or leaking earlier predicates).
    ///
    /// For zero-argument predicates or plain `fn() -> bool` functions, use [`Self::run_if_fn`]
    /// or `.run_if(|_| ...)`.
    #[inline]
    pub fn run_if<F>(mut self, condition: F) -> Self
    where
        F: FnMut(&RunConditionContext<'_>) -> bool + Send + 'static,
    {
        if self.first_error.is_none() {
            self.run_if_preds.push(Box::new(condition));
        }
        self
    }

    /// Appends a zero-argument run-condition predicate `|| -> bool`.
    ///
    /// Composes with any preceding `.run_if(...)` / `.run_if_fn(...)` predicates using
    /// short-circuit logical AND semantics.
    #[inline]
    pub fn run_if_fn<F>(self, mut condition: F) -> Self
    where
        F: FnMut() -> bool + Send + 'static,
    {
        self.run_if(move |_ctx| condition())
    }

    /// Appends an ECS-aware run condition backed by `BamSystemDesc::run_if_ctx` and `run_if_query`.
    ///
    /// `configure_query` declares read-only resources, message readers, or component terms
    /// accessible inside `condition` via [`RunConditionSystemContext`]. Any mutable term
    /// (`WRITE`, `OPTIONAL_WRITE`, `RESOURCE_WRITE`, `MESSAGE_WRITE`) is rejected with
    /// [`FfiResult::PERMISSION_DENIED`].
    ///
    /// Multiple `.run_if_with(...)` / `.run_if_condition(...)` calls merge their read-only terms
    /// into a single `run_if_query` (up to [`MAX_TERMS_PER_QUERY`]) and evaluate with
    /// short-circuit logical AND semantics.
    pub fn run_if_with<Q, F>(mut self, configure_query: Q, mut condition: F) -> Self
    where
        Q: for<'q> FnOnce(QueryBuilder<'q>) -> QueryBuilder<'q>,
        F: FnMut(&mut RunConditionSystemContext<'_>) -> bool + Send + 'static,
    {
        if self.first_error.is_some() {
            return self;
        }
        let host = *self.app.host();
        let qb = QueryBuilder {
            host: &host,
            manifest_name: host.manifest_name(),
            terms: Vec::new(),
            first_error: None,
        };
        let finished = configure_query(qb);
        if let Some(err) = finished.first_error {
            self.record_error(err);
            return self;
        }
        self.push_run_if_ctx_terms_and_pred(finished.terms, move |ctx, _offset| condition(ctx));
        self
    }

    /// Appends a pre-built [`EcsRunCondition`] (such as
    /// [`conditions::resource_exists`](crate::conditions::resource_exists),
    /// [`conditions::resource_changed`](crate::conditions::resource_changed),
    /// [`conditions::on_message`](crate::conditions::on_message), or
    /// [`conditions::in_state`](crate::conditions::in_state)).
    ///
    /// Multiple `.run_if_condition(...)` calls compose with short-circuit logical AND semantics
    /// and automatically assign each condition's base term offset in `run_if_query`.
    pub fn run_if_condition(mut self, condition: EcsRunCondition) -> Self {
        if self.first_error.is_some() {
            return self;
        }
        let host = *self.app.host();
        match (condition.build_terms)(&host, host.manifest_name()) {
            Ok(terms) => {
                let mut eval = condition.eval;
                self.push_run_if_ctx_terms_and_pred(terms, move |ctx, offset| eval(ctx, offset));
            }
            Err(err) => self.record_error(err),
        }
        self
    }

    /// Validates read-only terms and capacity, appends them to `run_if_ctx_terms`, and records
    /// `pred` with its starting term offset.
    fn push_run_if_ctx_terms_and_pred<F>(&mut self, terms: Vec<BamTermDesc>, pred: F)
    where
        F: FnMut(&mut RunConditionSystemContext<'_>, usize) -> bool + Send + 'static,
    {
        for term in &terms {
            if term.mode.is_mutable() {
                self.record_error(FfiResult::PERMISSION_DENIED);
                return;
            }
        }
        let offset = self.run_if_ctx_terms.len();
        if offset.saturating_add(terms.len()) > MAX_TERMS_PER_QUERY {
            self.record_error(FfiResult::CAPACITY_EXCEEDED);
            return;
        }
        self.run_if_ctx_terms.extend(terms);
        self.run_if_ctx_preds.push((offset, Box::new(pred)));
    }

    /// Compiles accumulated `.run_if(...)` predicates into a single instance-owned
    /// [`PreparedRunIf`] trampoline with short-circuit logical AND evaluation.
    fn prepare_run_if(
        instance: &mut GuestInstance,
        mut preds: Vec<RunIfPredicate>,
    ) -> Option<PreparedRunIf> {
        if preds.is_empty() {
            None
        } else if preds.len() == 1
            && let Some(mut single) = preds.pop()
        {
            Some(PreparedRunIf::new(instance, move |host| {
                let ctx = RunConditionContext::new(host);
                single(&ctx)
            }))
        } else {
            Some(PreparedRunIf::new(instance, move |host| {
                let ctx = RunConditionContext::new(host);
                preds.iter_mut().all(|pred| pred(&ctx))
            }))
        }
    }

    /// Shared bucketed finalization pipeline for [`Self::build`] and [`Self::register_handle`]:
    /// checks latched builder errors, assembles [`SystemRegistrationConfig`] and [`PreparedRunIf`],
    /// and dispatches across `N ∈ {0, 1, 4, 16}` slice trampolines without duplicating `callback`.
    fn finalize_bucketed<Out, Sub, F, R>(
        self,
        submitter: Sub,
        mut callback: F,
        on_err: impl FnOnce(FfiResult) -> Out,
    ) -> Out
    where
        Sub: SliceSystemSubmitter<Out>,
        F: FnMut(&mut SystemContext<'_>) -> R + Send + 'static,
        R: IntoSystemResult,
    {
        if let Some(err) = self.first_error {
            return on_err(err);
        }
        let (run_if_ctx, run_if_query, run_if_ctx_user_data) = if self.run_if_ctx_preds.is_empty() {
            (None, core::ptr::null(), core::ptr::null_mut())
        } else {
            let query_ptr =
                match allocate_run_if_ctx_query(self.app.instance, &self.run_if_ctx_terms) {
                    Ok(ptr) => ptr,
                    Err(err) => return on_err(err),
                };
            let query_count = usize::from(!self.run_if_ctx_terms.is_empty());
            let mut preds = self.run_if_ctx_preds;
            let (cb, ud) = allocate_run_if_ctx_trampoline(
                self.app.instance,
                query_count,
                move |inv: &mut ValidatedSystemInvocation<'_>| {
                    let mut ctx = RunConditionSystemContext::from_invocation(inv);
                    preds
                        .iter_mut()
                        .all(|(offset, pred)| pred(&mut ctx, *offset))
                },
            );
            (Some(cb), query_ptr, ud)
        };
        let query_slices: Vec<&[BamTermDesc]> = self.queries.iter().map(Vec::as_slice).collect();
        let config = SystemRegistrationConfig {
            name: self.name,
            user_queries: &query_slices,
            user_query_param_set_groups: &self.query_param_set_groups,
            system_resources: &self.system_resources,
            schedule: self.schedule,
            flags: self.flags,
            after: &self.after,
            before: &self.before,
            in_sets: &self.in_sets,
            after_weak: &self.after_weak,
            before_weak: &self.before_weak,
            ambiguous_with: &self.ambiguous_with,
            run_if_ctx,
            run_if_query,
            run_if_ctx_user_data,
        };
        let run_if = Self::prepare_run_if(self.app.instance, self.run_if_preds);
        let total_query_count =
            config.user_queries.len() + usize::from(!config.system_resources.is_empty());
        match total_query_count {
            0 => submitter.submit::<0, _>(
                &mut self.app.load_ctx,
                self.app.instance,
                &config,
                run_if,
                move |inv: &mut ValidatedSystemSlice<'_>| {
                    let mut slots: [Option<ChunkIter<'_>>; 0] = [];
                    let mut ctx = SystemContext::from_slice_invocation(inv, &mut slots);
                    invoke_system_callback(&mut callback, &mut ctx)
                },
            ),
            1 => submitter.submit::<1, _>(
                &mut self.app.load_ctx,
                self.app.instance,
                &config,
                run_if,
                move |inv: &mut ValidatedSystemSlice<'_>| {
                    let mut slots: [Option<ChunkIter<'_>>; 1] = [None];
                    let mut ctx = SystemContext::from_slice_invocation(inv, &mut slots);
                    invoke_system_callback(&mut callback, &mut ctx)
                },
            ),
            2..=4 => submitter.submit::<4, _>(
                &mut self.app.load_ctx,
                self.app.instance,
                &config,
                run_if,
                move |inv: &mut ValidatedSystemSlice<'_>| {
                    let mut slots: [Option<ChunkIter<'_>>; 4] = [const { None }; 4];
                    let mut ctx = SystemContext::from_slice_invocation(inv, &mut slots);
                    invoke_system_callback(&mut callback, &mut ctx)
                },
            ),
            _ => submitter.submit::<MAX_QUERIES_PER_SYSTEM, _>(
                &mut self.app.load_ctx,
                self.app.instance,
                &config,
                run_if,
                move |inv: &mut ValidatedSystemSlice<'_>| {
                    let mut slots: [Option<ChunkIter<'_>>; MAX_QUERIES_PER_SYSTEM] =
                        [const { None }; MAX_QUERIES_PER_SYSTEM];
                    let mut ctx = SystemContext::from_slice_invocation(inv, &mut slots);
                    invoke_system_callback(&mut callback, &mut ctx)
                },
            ),
        }
    }

    /// Finalizes and registers the system on the host `World`, returning [`FfiResult::OK`] on
    /// success or the first registration/resolution error encountered.
    pub fn build<F, R>(self, callback: F) -> FfiResult
    where
        F: FnMut(&mut SystemContext<'_>) -> R + Send + 'static,
        R: IntoSystemResult,
    {
        self.finalize_bucketed(BuildSubmitter, callback, |err| err)
    }

    /// Finalizes and registers the system with persistent per-system local state `local: L`
    /// (mirroring Bevy's `Local<L>` system parameter), passing `(&mut SystemContext<'_>, &mut L)`
    /// on every tick.
    pub fn build_with_local<L, F, R>(self, mut local: L, mut callback: F) -> FfiResult
    where
        L: Send + 'static,
        F: FnMut(&mut SystemContext<'_>, &mut L) -> R + Send + 'static,
        R: IntoSystemResult,
    {
        self.build(move |ctx| callback(ctx, &mut local))
    }

    /// Finalizes and registers the system via `BamHostApi::register_system_handle` (slot 60),
    /// returning its assigned [`BamSystem`] handle for decoupled scheduling with
    /// [`GuestApp::mount_system`].
    pub fn register_handle<F, R>(self, callback: F) -> Result<BamSystem, FfiResult>
    where
        F: FnMut(&mut SystemContext<'_>) -> R + Send + 'static,
        R: IntoSystemResult,
    {
        self.finalize_bucketed(HandleSubmitter, callback, Err)
    }

    /// Registers a decoupled [`BamSystem`] handle carrying persistent per-system local state
    /// `local: L` (mirroring Bevy's `Local<L>`).
    pub fn register_handle_with_local<L, F, R>(
        self,
        mut local: L,
        mut callback: F,
    ) -> Result<BamSystem, FfiResult>
    where
        L: Send + 'static,
        F: FnMut(&mut SystemContext<'_>, &mut L) -> R + Send + 'static,
        R: IntoSystemResult,
    {
        self.register_handle(move |ctx| callback(ctx, &mut local))
    }
}
