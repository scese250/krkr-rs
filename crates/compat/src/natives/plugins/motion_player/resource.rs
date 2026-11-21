//! `Motion.ResourceManager` — PSB module cache, layer-id allocator and the
//! shared snapshot store used by the players.
//!
//! Behavioural reference: `reference/cpp/plugins/motionplayer/ResourceManager.*`.
//! The reference keeps the cache in a `shared_ptr<State>`; this port keeps one
//! store per VM engine in the compat service state, so a `Player` and the
//! manager it was built from observe the same data without smuggling a
//! pointer through the constructor argument.
#![allow(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;

use tjs::{DetachedValue, RetainedValue, TjsValue, VAL_OBJECT, Value};

use super::StoredValue;
use super::emote::ModuleSettings;
use super::helpers::{eval_object, inst_mut, set_retained};
use super::model::MotionSnapshot;
use crate::engine_api::{JobOutcome, JobValue, JobWork, PendingStorage, StorageReadError};
use crate::native_util::{args, report_error, set_int, set_void, value_as_string};
use crate::natives::plugins::psb_file::container::{PsbDocument, PsbValue, parse_stream};
use crate::natives::plugins::psb_file::media::{
    PsbMedia, install_provider as install_psb_provider, media as psb_media,
};
use crate::natives::plugins::psb_file::normalize_psb_domain;
use crate::natives::storages::extract_storage_name;
use crate::natives::system::{resolve, value_as_i64};
use crate::storage::strip_file_media;
use crate::{CompatCtx, CompatError};

/// Per-engine snapshot store keyed by an opaque id.
#[derive(Default)]
pub(crate) struct MotionStore {
    next: u64,
    snapshots: BTreeMap<u64, Arc<MotionSnapshot>>,
    /// The TJS module object each registered snapshot was built from.
    modules: BTreeMap<u64, StoredValue>,
    /// The storage path each registered snapshot was loaded from.
    paths: BTreeMap<u64, String>,
    pub(crate) decrypt_seed: i64,
    decrypt_func: Option<DetachedValue>,
    rng: u64,
    /// The `D3DEmoteModule` settings; the reference keeps every one of them
    /// as a class-level static (`reference/cpp/plugins/motionplayer/
    /// D3DEmoteModule.h:43-48`), so all instances share a single value.
    pub(crate) module: ModuleSettings,
}

impl MotionStore {
    /// Insert a snapshot and return its id.
    pub(crate) fn insert(&mut self, snapshot: MotionSnapshot) -> u64 {
        self.next += 1;
        let id = self.next;
        self.snapshots.insert(id, Arc::new(snapshot));
        id
    }

    /// Look up a snapshot.
    pub(crate) fn get(&self, id: u64) -> Option<Arc<MotionSnapshot>> {
        self.snapshots.get(&id).cloned()
    }

    /// Attach the TJS module object and source path to a snapshot id.
    pub(crate) fn register_module(&mut self, id: u64, path: &str, module: StoredValue) {
        self.modules.insert(id, module);
        self.paths.insert(id, path.to_string());
    }

    /// The module object registered for a snapshot id.
    pub(crate) fn module(&self, id: u64) -> Option<&StoredValue> {
        self.modules.get(&id)
    }

    /// The source path registered for a snapshot id.
    pub(crate) fn path(&self, id: u64) -> Option<&str> {
        self.paths.get(&id).map(String::as_str)
    }

    /// Remove a snapshot by id (reference `detail::unregisterModuleSnapshot`,
    /// `RuntimeSupport.cpp:1575-1582`). A player that already loaded the
    /// snapshot keeps its own `Arc`, so an in-flight motion is unaffected.
    pub(crate) fn remove(&mut self, id: u64) {
        self.modules.remove(&id);
        self.paths.remove(&id);
        self.snapshots.remove(&id);
    }

    /// Drop every snapshot (reference `detail::clearModuleSnapshots`,
    /// `RuntimeSupport.cpp:1584-1587`).
    pub(crate) fn clear_snapshots(&mut self) {
        self.snapshots.clear();
        self.modules.clear();
        self.paths.clear();
    }

    /// The next pseudo-random value in `[0, 1)`.
    ///
    /// The reference `Player::random()` delegates to a
    /// `Math.RandomGenerator` instance owned by the player and converts its
    /// `random()` result to a double
    /// (`reference/cpp/plugins/motionplayer/PlayerCore.cpp:1318-1332`), so
    /// consecutive calls yield fresh reals in `[0, 1)`. This xorshift keeps
    /// the same observable contract with one advancing state per engine.
    pub(crate) fn next_random(&mut self) -> f64 {
        if self.rng == 0 {
            self.rng = 0x2545_F491_4F6C_DD1D;
        }
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        ((x >> 11) as f64) * (1.0 / ((1u64 << 53) as f64))
    }
}

/// Per-instance `ResourceManager` payload.
#[derive(Default)]
pub(crate) struct ResourceInst {
    /// Path key → the module object that `load` returned for it.
    loaded: BTreeMap<String, StoredValue>,
    last: Option<StoredValue>,
    layer_ids: BTreeMap<i64, String>,
    layer_names: BTreeMap<String, i64>,
    used: BTreeSet<i64>,
    next: i64,
}

impl ResourceInst {
    fn require_layer_id(&mut self) -> i64 {
        loop {
            self.next += 1;
            if self.used.insert(self.next) {
                return self.next;
            }
        }
    }

    fn find_key(&self, path: &str) -> Option<&StoredValue> {
        cache_keys(path).iter().find_map(|key| self.loaded.get(key))
    }

    /// The most recently loaded module object.
    pub(crate) fn last_module(&self) -> Option<&StoredValue> {
        self.last.as_ref()
    }

    /// Read, parse and cache the motion module at `path`.
    ///
    /// `Ok(None)` means nothing is mounted at `path`; a mounted file that
    /// cannot be read or parsed is an error.
    pub(crate) fn load_module(
        &mut self,
        ctx: &CompatCtx,
        path: &str,
    ) -> Result<Option<(u64, Arc<MotionSnapshot>)>, String> {
        let Some((id, snapshot, module)) = read_motion_module(ctx, path)? else {
            return Ok(None);
        };
        for key in cache_keys(path) {
            // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `load_module`.
            let duplicate = unsafe { ctx.engine() }
                .duplicate_retained(module.raw_id())
                .map_err(|error| error.to_string())?;
            self.loaded.insert(key, StoredValue::Object(duplicate));
        }
        self.last = Some(StoredValue::Object(module));
        Ok(Some((id, snapshot)))
    }

    /// Drop every cached module, the layer-id allocator state and the engine
    /// snapshot registry.
    pub(crate) fn clear_cache(&mut self, ctx: &CompatCtx) {
        self.loaded.clear();
        self.last = None;
        self.layer_ids.clear();
        self.layer_names.clear();
        self.used.clear();
        self.next = 0;
        // SAFETY: a `CompatCtx` is only created for a bound, live engine.
        unsafe { ctx.services() }
            .state::<MotionStore>()
            .clear_snapshots();
    }
}

fn cache_keys(path: &str) -> Vec<String> {
    let mut keys = vec![path.to_string()];
    if let Some(stripped) = path.strip_prefix("./") {
        keys.push(stripped.to_string());
    }
    keys
}

extern "C" fn resource_create(_engine: *mut std::ffi::c_void) -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(ResourceInst::default())) as *mut std::ffi::c_void
}

extern "C" fn resource_destroy(_engine: *mut std::ffi::c_void, instance: *mut std::ffi::c_void) {
    if instance.is_null() {
        return;
    }
    // SAFETY: `instance` came from `resource_create` and is released once.
    drop(unsafe { Box::from_raw(instance as *mut ResourceInst) });
}

extern "C" fn resource_ctor(
    engine: *mut std::ffi::c_void,
    _instance: *mut std::ffi::c_void,
    argc: std::os::raw::c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut std::ffi::c_char,
    _objthis: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    let Some(res) = resolve(engine) else {
        return report_error(out_error, super::helpers::NO_BINDING);
    };
    // `NCB_CONSTRUCTOR((iTJSDispatch2 *, tjs_int))` registers an arity of 2,
    // and ncbind rejects shorter calls
    // (`reference/cpp/plugins/motionplayer/main.cpp:405`,
    // `reference/cpp/core/plugin/ncbind.hpp:1178`).
    // The reference stores the cache-size hint; this port has no cache
    // eviction yet, so the arguments are accepted after the count gate.
    // SAFETY: the trampoline guarantees `argc` valid entries.
    let arguments = unsafe { args(argv, argc) };
    if arguments.len() < 2 {
        return report_error(out_error, "ResourceManager: bad parameter count");
    }
    // The reference pre-defines three keybinder dictionaries on the KAG
    // window argument when they are absent, because encrypted
    // `keybinder.tjs` dereferences them unconditionally
    // (`reference/cpp/plugins/motionplayer/ResourceManager.cpp:84-101`).
    if arguments[0].ty == VAL_OBJECT {
        let ctx = res.ctx();
        // SAFETY: the caller resolved `ctx` from the installed binding.
        let engine_ref = unsafe { ctx.engine() };
        // SAFETY: the argument is a live object for the duration of the call.
        if let Ok(window) = engine_ref.retain_object_arg(&arguments[0]) {
            for name in [
                "ShortCutInitialPadKeyMap",
                "ShortCutInitialGamePadKeyMap",
                "_proceedingKeyList",
            ] {
                let defined = engine_ref
                    .member(window.raw_id(), name)
                    .is_ok_and(|value| !matches!(value, TjsValue::Void));
                if !defined {
                    let Ok(dictionary) = eval_object(&ctx, "%[]") else {
                        continue;
                    };
                    let id = dictionary.raw_id().as_ptr() as usize as u64;
                    let _ = engine_ref.set_member(window.raw_id(), name, &TjsValue::Retained(id));
                    drop(dictionary);
                }
            }
        }
    }
    set_void(out);
    0
}

/// Register every resource of a loaded motion container in the `psb://`
/// storage media, under the container's normalized name.
///
/// A motion container is registered under both the name it was loaded by and
/// its final path component, so a game can resolve its icon textures either
/// way. The whole value tree is served, the way the separate registry of
/// the container resources serves it.
fn register_motion_media(media: &PsbMedia, document: Rc<PsbDocument>, container: &str) {
    let alias = normalize_psb_domain(&extract_storage_name(container));
    media.register_whole_tree(container, Rc::clone(&document));
    if alias != container {
        media.register_whole_tree(&alias, document);
    }
}

/// The parsed container and the snapshot built from it: the pure-data part
/// of a module load.
struct ParsedModule {
    /// The parsed container.
    document: PsbDocument,
    /// The snapshot the module object is built from.
    snapshot: MotionSnapshot,
}

/// The container's bytes: the ones a scheduled read produces, or the ones an
/// extraction filter already transformed.
#[derive(Clone)]
enum ContainerBytes {
    /// The read is scheduled on the storage's read pool: the worker waits
    /// for the bytes.
    Scheduled(PendingStorage),
    /// The bytes are final: an extraction filter transformed them on the
    /// frame thread, because the filter is script code.
    Ready(Arc<[u8]>),
}

/// Parse the container's bytes and build the snapshot.
///
/// The parse and the snapshot build are pure data: they run on a worker when
/// the pool takes the job, and on the calling thread when it does not.
fn parse_module(
    source: &ContainerBytes,
    name: &str,
    seed: i64,
    container: &str,
) -> Result<ParsedModule, String> {
    let document = match source {
        ContainerBytes::Scheduled(pending) => {
            let bytes = pending
                .await_bytes(name)
                .map_err(|error| error.to_string())?;
            parse_stream(&bytes, seed).map_err(|error| error.to_string())?
        }
        ContainerBytes::Ready(bytes) => {
            parse_stream(bytes, seed).map_err(|error| error.to_string())?
        }
    };
    let snapshot = MotionSnapshot::from_psb(&document, container);
    Ok(ParsedModule { document, snapshot })
}

/// Parse the container's bytes into a module on the engine's job pool.
///
/// The parse and the snapshot build are pure data, so they run on a worker:
/// the frame thread's own work is the object tree the module object is built
/// from. The frame thread waits for the module, because the object the
/// native returns must carry the container's members.
fn load_parsed_module(
    ctx: &CompatCtx,
    source: ContainerBytes,
    name: &str,
    seed: i64,
    container: &str,
) -> Result<ParsedModule, String> {
    // SAFETY: the host keeps the attached pool alive for the binding.
    let pool = unsafe { ctx.services() }.decode_pool();
    // SAFETY: the host keeps the attached pool alive for the binding.
    let Some(pool) = pool.map(|pool| unsafe { &mut *pool }) else {
        // No pool is attached: the container is parsed on the calling thread.
        return parse_module(&source, name, seed, container);
    };
    // The bytes the job holds: the container's own size.
    let charge = match &source {
        ContainerBytes::Scheduled(pending) => pending.size().unwrap_or(0),
        ContainerBytes::Ready(bytes) => bytes.len() as u64,
    };
    // The parsed module travels to the frame thread through a channel: the
    // pool hands its own value out through a shared pointer, and the parsed
    // container must move to the frame thread.
    let (sender, receiver) = mpsc::channel::<Result<ParsedModule, String>>();
    let job_source = source.clone();
    let job_name = name.to_string();
    let job_container = container.to_string();
    let work: Arc<JobWork> = Arc::new(move || {
        let parsed = parse_module(&job_source, &job_name, seed, &job_container);
        if sender.send(parsed).is_err() {
            // The frame thread is gone: the module goes with it.
            log::warn!("compat.psb: the parsed module could not reach the frame thread");
        }
        Ok(JobValue::new(()))
    });
    match pool.submit_job(work, charge) {
        Ok((_id, job)) => {
            // The frame thread waits for this module, so a worker takes the
            // job before a prefetch job.
            job.demand();
            match receiver.recv() {
                Ok(parsed) => parsed,
                // The worker stopped before it published the module: the
                // pool's own outcome names the cause.
                Err(_) => match job.await_ready() {
                    JobOutcome::Failed(message) => Err(message),
                    JobOutcome::Cancelled => {
                        Err("motionplayer: the module load was cancelled".to_string())
                    }
                    JobOutcome::Ready(_) => {
                        Err("motionplayer: the module load published no module".to_string())
                    }
                },
            }
        }
        Err(error) => {
            // The pool refused the job: the container is parsed on the
            // calling thread, and the refusal is reported.
            log::warn!(
                "compat.psb: the job pool refused the module load ({error}); the container is parsed on the calling thread"
            );
            parse_module(&source, name, seed, container)
        }
    }
}

/// Read and parse the PSB at `path`, register the snapshot and its module
/// object in the engine store, and return both plus the module handle.
///
/// The container's bytes, its parse and the snapshot are pure data: the read
/// runs on the storage's read pool and the parse on the engine's job pool,
/// so the frame thread's own work is the object tree the module object is
/// built from.
///
/// `Ok(None)` means nothing is mounted at `path`; a mounted file that cannot
/// be read or parsed is an error.
pub(crate) fn read_motion_module(
    ctx: &CompatCtx,
    path: &str,
) -> Result<Option<(u64, Arc<MotionSnapshot>, DetachedValue)>, String> {
    // SAFETY: the caller resolved `ctx` from the installed binding.
    let services = unsafe { ctx.services() };
    let seed = services.state::<MotionStore>().decrypt_seed;
    let Some(storage) = services.storage() else {
        return Err("motionplayer: no storage is attached".into());
    };
    let name = strip_file_media(path);
    // SAFETY: the host keeps the attached storage alive for the binding.
    let storage = unsafe { &*storage };
    match storage.find(name) {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    }
    let container = normalize_psb_domain(path);
    // The container's bytes. The read is scheduled on the storage's read
    // pool, so the archive decode is not the frame thread's work. An
    // extraction filter transforms the decoded bytes with script code, and
    // script code runs on the frame thread: a filtered container is
    // therefore read here, and only its parse runs on the pool.
    let source = if storage.xp3_extraction_filter.is_some() {
        let bytes = storage.read(name).map_err(|error| error.to_string())?;
        ContainerBytes::Ready(Arc::from(bytes))
    } else {
        let Some(pending) = storage.lookup(name).map_err(|error| error.to_string())? else {
            return Err(StorageReadError::NotFound(name.to_string()).to_string());
        };
        ContainerBytes::Scheduled(pending)
    };
    let parsed = load_parsed_module(ctx, source, name, seed, &container)?;
    let document = Rc::new(parsed.document);
    register_motion_media(&psb_media(services), Rc::clone(&document), &container);
    // The player reads the container's pixel chunks through the `psb://`
    // media, so the media must be served: install its provider with the
    // storage the container was read from. The media is shared with the
    // `psbfile` plugin, and both install the provider that serves it.
    install_psb_provider(services).map_err(|error| error.to_string())?;
    let id = services.state::<MotionStore>().insert(parsed.snapshot);
    let registered = {
        let store = services.state::<MotionStore>();
        store.get(id)
    };
    let Some(snapshot) = registered else {
        services.state::<MotionStore>().remove(id);
        return Err("motionplayer: snapshot registration failed".into());
    };
    // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `read_motion_module`.
    let module = match module_object(ctx, id, path, Some(snapshot.as_ref())) {
        Ok(module) => module,
        Err(error) => {
            services.state::<MotionStore>().remove(id);
            return Err(error);
        }
    };
    // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `read_motion_module`.
    let registered = match unsafe { ctx.engine() }.duplicate_retained(module.raw_id()) {
        Ok(registered) => registered,
        Err(error) => {
            services.state::<MotionStore>().remove(id);
            return Err(error.to_string());
        }
    };
    services
        .state::<MotionStore>()
        .register_module(id, path, StoredValue::Object(registered));
    Ok(Some((id, snapshot, module)))
}

/// The engine-registered module object for `id`, rebuilt from a cached
/// snapshot when the registration was cleared.
pub(crate) fn module_for_snapshot(
    ctx: &CompatCtx,
    id: u64,
    path: &str,
    snapshot: &MotionSnapshot,
) -> Result<DetachedValue, String> {
    let module = module_object(ctx, id, path, Some(snapshot))?;
    // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `module_for_snapshot`.
    let registered = unsafe { ctx.engine() }
        .duplicate_retained(module.raw_id())
        .map_err(|error| error.to_string())?;
    // SAFETY: the caller resolved `ctx` from the installed binding.
    unsafe { ctx.services() }
        .state::<MotionStore>()
        .register_module(id, path, StoredValue::Object(registered));
    Ok(module)
}

/// Build the TJS `%{path, metadata, object, __motionSnapshot}` module.
fn module_object(
    ctx: &CompatCtx,
    id: u64,
    path: &str,
    snapshot: Option<&MotionSnapshot>,
) -> Result<DetachedValue, String> {
    let object = dictionary_new(ctx)?;
    // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `module_object`.
    let engine = unsafe { ctx.engine() };
    engine
        .set_member(object.raw_id(), "path", &TjsValue::String(path.to_string()))
        .map_err(|error| error.to_string())?;
    engine
        .set_member(
            object.raw_id(),
            "__motionSnapshot",
            &TjsValue::Integer(id as i64),
        )
        .map_err(|error| error.to_string())?;
    if let Some(snapshot) = snapshot {
        let metadata = build_psb(ctx, &snapshot.metadata)?;
        install(ctx, object.raw_id(), "metadata", metadata)?;
        let object_tree = build_psb(ctx, &snapshot.root)?;
        install(ctx, object.raw_id(), "object", object_tree)?;
        engine
            .set_member(object.raw_id(), "width", &TjsValue::Real(snapshot.width))
            .map_err(|error| error.to_string())?;
        engine
            .set_member(object.raw_id(), "height", &TjsValue::Real(snapshot.height))
            .map_err(|error| error.to_string())?;
    }
    Ok(object)
}

/// A value ready to install on a TJS object.
enum Built {
    Scalar(TjsValue),
    Object(DetachedValue),
}

fn install(
    ctx: &CompatCtx,
    parent: tjs::Tjs2ValueId,
    key: &str,
    built: Built,
) -> Result<(), String> {
    // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `install`.
    let engine = unsafe { ctx.engine() };
    match built {
        Built::Scalar(value) => engine
            .set_member(parent, key, &value)
            .map_err(|error| error.to_string()),
        Built::Object(object) => {
            let id = object.raw_id().as_ptr() as usize as u64;
            engine
                .set_member(parent, key, &TjsValue::Retained(id))
                .map_err(|error| error.to_string())?;
            drop(object);
            Ok(())
        }
    }
}

fn build_psb(ctx: &CompatCtx, value: &PsbValue) -> Result<Built, String> {
    match value {
        PsbValue::Null => Ok(Built::Scalar(TjsValue::Void)),
        PsbValue::Bool(value) => Ok(Built::Scalar(TjsValue::Integer(i64::from(*value)))),
        PsbValue::Int(value) => Ok(Built::Scalar(TjsValue::Integer(*value))),
        PsbValue::Float(value) => Ok(Built::Scalar(TjsValue::Real(f64::from(*value)))),
        PsbValue::Double(value) => Ok(Built::Scalar(TjsValue::Real(*value))),
        PsbValue::String(value) => Ok(Built::Scalar(TjsValue::String(value.clone()))),
        PsbValue::Array(values) => {
            let array = array_new(ctx)?;
            // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `build_psb`.
            let engine = unsafe { ctx.engine() };
            for value in values {
                engine
                    .array_add(array.raw_id(), &TjsValue::Integer(i64::from(*value)))
                    .map_err(|error| error.to_string())?;
            }
            Ok(Built::Object(array))
        }
        PsbValue::List(items) => {
            let array = array_new(ctx)?;
            for item in items {
                let built = build_psb(ctx, item)?;
                add_built(ctx, array.raw_id(), built)?;
            }
            Ok(Built::Object(array))
        }
        PsbValue::Dictionary(entries) => {
            let dictionary = dictionary_new(ctx)?;
            for (key, item) in entries {
                let built = build_psb(ctx, item)?;
                install(ctx, dictionary.raw_id(), key, built)?;
            }
            Ok(Built::Object(dictionary))
        }
        PsbValue::Resource(reference) => Ok(Built::Scalar(TjsValue::Integer(if reference.extra {
            i64::from(reference.index) | 0x4000_0000
        } else {
            i64::from(reference.index)
        }))),
    }
}

/// A fresh TJS array: the VM's own array class, the object an array
/// literal produces.
///
/// The VM's constructor is used and not a script literal: a literal is
/// compiled and run once per container, and a container holds tens of
/// thousands of them.
fn array_new(ctx: &CompatCtx) -> Result<DetachedValue, String> {
    // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `array_new`.
    unsafe { ctx.engine() }.new_array()
}

/// A fresh TJS dictionary: the VM's own dictionary class, the object a
/// dictionary literal produces.
fn dictionary_new(ctx: &CompatCtx) -> Result<DetachedValue, String> {
    // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `dictionary_new`.
    unsafe { ctx.engine() }.new_dictionary()
}

/// Append `built` to `array`: the VM's `add`, the method a script calls to
/// grow an array.
fn add_built(ctx: &CompatCtx, array: tjs::Tjs2ValueId, built: Built) -> Result<(), String> {
    // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `add_built`.
    let engine = unsafe { ctx.engine() };
    match built {
        Built::Scalar(value) => engine.array_add(array, &value),
        Built::Object(object) => {
            let id = object.raw_id().as_ptr() as usize as u64;
            let result = engine.array_add(array, &TjsValue::Retained(id));
            drop(object);
            result
        }
    }
}

extern "C" fn resource_load(
    engine: *mut std::ffi::c_void,
    instance: *mut std::ffi::c_void,
    argc: std::os::raw::c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut std::ffi::c_char,
    _objthis: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    let Some(res) = resolve(engine) else {
        return report_error(out_error, super::helpers::NO_BINDING);
    };
    // SAFETY: the VM guarantees the live payload.
    let Some(inst) = (unsafe { inst_mut::<ResourceInst>(instance) }) else {
        return report_error(out_error, "ResourceManager: null instance");
    };
    // SAFETY: the trampoline guarantees `argc` valid entries.
    let arguments = unsafe { args(argv, argc) };
    // `tTJSVariant load(ttstr path)` / `loadSource(ttstr path)` — ncbind
    // arity 1 (`reference/cpp/plugins/motionplayer/ResourceManager.h:19-20`).
    if arguments.is_empty() {
        return report_error(out_error, "ResourceManager.load: bad parameter count");
    }
    let path = arguments.first().map(value_as_string).unwrap_or_default();
    if path.is_empty() {
        set_void(out);
        return 0;
    }
    match inst.load_module(&res.ctx(), &path) {
        Ok(Some(_)) => {
            let ctx = res.ctx();
            match inst.last_module().map(|module| module.to_retained(&ctx)) {
                Some(Ok(RetainedValue::Object(copy))) => {
                    set_retained(out, copy);
                    0
                }
                Some(Ok(_)) => {
                    set_void(out);
                    0
                }
                Some(Err(error)) => report_error(out_error, &error),
                None => {
                    set_void(out);
                    0
                }
            }
        }
        Ok(None) => {
            log::warn!("motion_player: no stored motion at {path:?}");
            set_void(out);
            0
        }
        Err(error) => report_error(out_error, &error),
    }
}

extern "C" fn resource_load_source(
    engine: *mut std::ffi::c_void,
    instance: *mut std::ffi::c_void,
    argc: std::os::raw::c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut std::ffi::c_char,
    objthis: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    resource_load(engine, instance, argc, argv, out, out_error, objthis)
}

extern "C" fn resource_unload(
    engine: *mut std::ffi::c_void,
    instance: *mut std::ffi::c_void,
    argc: std::os::raw::c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut std::ffi::c_char,
    _objthis: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    let Some(res) = resolve(engine) else {
        return report_error(out_error, super::helpers::NO_BINDING);
    };
    // SAFETY: the VM guarantees the live payload.
    let Some(inst) = (unsafe { inst_mut::<ResourceInst>(instance) }) else {
        return report_error(out_error, "ResourceManager: null instance");
    };
    // SAFETY: the trampoline guarantees `argc` valid entries.
    let arguments = unsafe { args(argv, argc) };
    // `void unload(ttstr path)` — ncbind arity 1
    // (`reference/cpp/plugins/motionplayer/ResourceManager.h:21`).
    if arguments.is_empty() {
        return report_error(out_error, "ResourceManager.unload: bad parameter count");
    }
    let path = arguments.first().map(value_as_string).unwrap_or_default();
    // The reference drops every cache key that points at the same module
    // object, then falls back to erasing the path's own keys
    // (`reference/cpp/plugins/motionplayer/ResourceManager.cpp:172-205`).
    let victim = cache_keys(&path)
        .iter()
        .filter_map(|key| inst.loaded.get(key))
        .find_map(StoredValue::object_id);
    match victim {
        Some(id) => {
            unregister_snapshot(&res, id);
            inst.loaded
                .retain(|_, module| module.object_id() != Some(id));
        }
        None => {
            for key in cache_keys(&path) {
                inst.loaded.remove(&key);
            }
        }
    }
    set_void(out);
    0
}

/// Remove the snapshot a cached module registered (reference
/// `detail::unregisterModuleSnapshot`, called from `ResourceManager::unload`
/// at `ResourceManager.cpp:194`). The module carries its snapshot id in the
/// `__motionSnapshot` member (`module_object`).
fn unregister_snapshot(res: &crate::natives::system::EngineBinding, module_raw: usize) {
    let Ok(TjsValue::Integer(snapshot_id)) = res.engine().member(
        tjs::Tjs2ValueId::from_raw(module_raw as *mut std::ffi::c_void),
        "__motionSnapshot",
    ) else {
        return;
    };
    let ctx = res.ctx();
    // SAFETY: a `CompatCtx` is only created for a bound, live engine.
    unsafe { ctx.services() }
        .state::<MotionStore>()
        .remove(snapshot_id as u64);
}

extern "C" fn resource_clear_cache(
    engine: *mut std::ffi::c_void,
    instance: *mut std::ffi::c_void,
    _argc: std::os::raw::c_int,
    _argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut std::ffi::c_char,
    _objthis: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    let Some(res) = resolve(engine) else {
        return report_error(out_error, super::helpers::NO_BINDING);
    };
    // SAFETY: the VM guarantees the live payload.
    let Some(inst) = (unsafe { inst_mut::<ResourceInst>(instance) }) else {
        return report_error(out_error, "ResourceManager: null instance");
    };
    inst.clear_cache(&res.ctx());
    set_void(out);
    0
}

extern "C" fn resource_find_source(
    engine: *mut std::ffi::c_void,
    instance: *mut std::ffi::c_void,
    argc: std::os::raw::c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut std::ffi::c_char,
    _objthis: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    let Some(_res) = resolve(engine) else {
        return report_error(out_error, super::helpers::NO_BINDING);
    };
    // SAFETY: the VM guarantees the live payload.
    let Some(inst) = (unsafe { inst_mut::<ResourceInst>(instance) }) else {
        return report_error(out_error, "ResourceManager: null instance");
    };
    // SAFETY: the trampoline guarantees `argc` valid entries.
    let arguments = unsafe { args(argv, argc) };
    // `tTJSVariant findSource(ttstr path)` — ncbind arity 1
    // (`reference/cpp/plugins/motionplayer/ResourceManager.h:27`).
    if arguments.is_empty() {
        return report_error(out_error, "ResourceManager.findSource: bad parameter count");
    }
    let path = arguments.first().map(value_as_string).unwrap_or_default();
    // `findSource` returns the cached module object itself (void on a miss)
    // (`reference/cpp/plugins/motionplayer/ResourceManager.cpp:263-265`).
    match inst.find_key(&path) {
        Some(module) => {
            let ctx = _res.ctx();
            match module.to_retained(&ctx) {
                Ok(RetainedValue::Object(copy)) => {
                    set_retained(out, copy);
                    0
                }
                _ => {
                    set_void(out);
                    0
                }
            }
        }
        None => {
            set_void(out);
            0
        }
    }
}

extern "C" fn resource_require_layer_id(
    engine: *mut std::ffi::c_void,
    instance: *mut std::ffi::c_void,
    _argc: std::os::raw::c_int,
    _argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut std::ffi::c_char,
    _objthis: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    let Some(_res) = resolve(engine) else {
        return report_error(out_error, super::helpers::NO_BINDING);
    };
    // SAFETY: the VM guarantees the live payload.
    let Some(inst) = (unsafe { inst_mut::<ResourceInst>(instance) }) else {
        return report_error(out_error, "ResourceManager: null instance");
    };
    let id = inst.require_layer_id();
    inst.layer_ids.insert(id, String::new());
    set_int(out, id);
    0
}

extern "C" fn resource_release_layer_id(
    engine: *mut std::ffi::c_void,
    instance: *mut std::ffi::c_void,
    argc: std::os::raw::c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut std::ffi::c_char,
    _objthis: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    let Some(_res) = resolve(engine) else {
        return report_error(out_error, super::helpers::NO_BINDING);
    };
    // SAFETY: the VM guarantees the live payload.
    let Some(inst) = (unsafe { inst_mut::<ResourceInst>(instance) }) else {
        return report_error(out_error, "ResourceManager: null instance");
    };
    // SAFETY: the trampoline guarantees `argc` valid entries.
    let arguments = unsafe { args(argv, argc) };
    // `void releaseLayerId(tjs_int id)` — ncbind arity 1
    // (`reference/cpp/plugins/motionplayer/ResourceManager.h:37`).
    if arguments.is_empty() {
        return report_error(
            out_error,
            "ResourceManager.releaseLayerId: bad parameter count",
        );
    }
    let id = arguments.first().map(value_as_i64).unwrap_or(0);
    inst.used.remove(&id);
    if let Some(name) = inst.layer_ids.remove(&id) {
        inst.layer_names.remove(&name);
    }
    set_void(out);
    0
}

extern "C" fn resource_set_decrypt_seed(
    engine: *mut std::ffi::c_void,
    argc: std::os::raw::c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut std::ffi::c_char,
) -> std::os::raw::c_int {
    let Some(res) = resolve(engine) else {
        return report_error(out_error, super::helpers::NO_BINDING);
    };
    // SAFETY: the trampoline guarantees `argc` valid entries.
    let arguments = unsafe { args(argv, argc) };
    // Typed validation straight from the reference static member: exactly
    // one argument, and it must be an integer — anything else fails with
    // `TJS_E_BADPARAMCOUNT` / `TJS_E_INVALIDPARAM` instead of silently
    // defaulting to 0
    // (`reference/cpp/plugins/motionplayer/ResourceManager.cpp:112-121`).
    if arguments.len() != 1 {
        return report_error(out_error, "setEmotePSBDecryptSeed: bad parameter count");
    }
    if arguments[0].ty != tjs::VAL_INTEGER {
        return report_error(out_error, "setEmotePSBDecryptSeed: invalid parameter");
    }
    let seed = value_as_i64(&arguments[0]);
    let ctx = res.ctx();
    // SAFETY: the caller resolved `ctx` from the installed binding.
    let services = unsafe { ctx.services() };
    services.state::<MotionStore>().decrypt_seed = seed;
    set_void(out);
    0
}

extern "C" fn resource_set_decrypt_func(
    engine: *mut std::ffi::c_void,
    argc: std::os::raw::c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut std::ffi::c_char,
) -> std::os::raw::c_int {
    let Some(res) = resolve(engine) else {
        return report_error(out_error, super::helpers::NO_BINDING);
    };
    // SAFETY: the trampoline guarantees `argc` valid entries.
    let arguments = unsafe { args(argv, argc) };
    // Exactly one argument, per the reference static member
    // (`reference/cpp/plugins/motionplayer/ResourceManager.cpp:129-132`);
    // an object value stores the closure, anything else clears it
    // (`ResourceManager.cpp:136-141`).
    if arguments.len() != 1 {
        return report_error(out_error, "setEmotePSBDecryptFunc: bad parameter count");
    }
    let ctx = res.ctx();
    // SAFETY: `ctx` is only created for a bound, live engine, so its service pointer is valid for `resource_set_decrypt_func`.
    let mut store = unsafe { ctx.services() }.state::<MotionStore>();
    store.decrypt_func = None;
    if let Some(value) = arguments.first()
        && value.ty == VAL_OBJECT
        // SAFETY: `ctx` is only created for a bound, live engine, so its VM pointer is valid for `resource_set_decrypt_func`.
        && let Ok(retained) = unsafe { ctx.engine() }.retain_object_arg(value)
    {
        store.decrypt_func = Some(retained);
    }
    set_void(out);
    0
}

/// Register `Motion.ResourceManager` and its static decrypt hooks.
pub(crate) fn register(ctx: &mut CompatCtx) -> Result<(), CompatError> {
    use tjs::{NativeInstanceBuilder, NativeInstanceMethodDef};

    ctx.register_instance(&NativeInstanceBuilder {
        name: "ResourceManager",
        create: resource_create,
        destroy: resource_destroy,
        invalidate: None,
        methods: vec![
            NativeInstanceMethodDef {
                name: "ResourceManager",
                f: resource_ctor,
            },
            NativeInstanceMethodDef {
                name: "load",
                f: resource_load,
            },
            NativeInstanceMethodDef {
                name: "loadSource",
                f: resource_load_source,
            },
            NativeInstanceMethodDef {
                name: "unload",
                f: resource_unload,
            },
            NativeInstanceMethodDef {
                name: "clearCache",
                f: resource_clear_cache,
            },
            NativeInstanceMethodDef {
                name: "findSource",
                f: resource_find_source,
            },
            NativeInstanceMethodDef {
                name: "requireLayerId",
                f: resource_require_layer_id,
            },
            NativeInstanceMethodDef {
                name: "releaseLayerId",
                f: resource_release_layer_id,
            },
        ],
        properties: vec![],
    })?;
    ctx.register_static_members(&tjs::NativeStaticMembers {
        class_name: "ResourceManager",
        methods: vec![
            tjs::NativeMethodDef {
                name: "setEmotePSBDecryptSeed",
                f: resource_set_decrypt_seed,
            },
            tjs::NativeMethodDef {
                name: "setEmotePSBDecryptFunc",
                f: resource_set_decrypt_func,
            },
        ],
        properties: vec![],
    })?;
    Ok(())
}
