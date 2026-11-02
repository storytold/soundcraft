use super::editor::{EditorInner, EditorLink};
use super::host::HostContext;
use super::instance::Instance;
use super::process::{EventList, ParamChanges};
use super::{
    Arc, AtomicBool, AtomicUsize, ComPtr, ComWrapper, Duration, EVENT_CAP, FUnknown, IComponentTrait, IConnectionPointTrait, IEditControllerTrait,
    IPluginBaseTrait, IPluginFactory2Trait, IPluginFactoryTrait, Interface, MAX_CLASSES, ManuallyDrop, Mutex, OnceLock, Ordering, PARAM_QUEUE_CAP,
    Path, PathBuf, PoisonError, TryLockError, Vst3Error, c_char, c_int, c_void, mpsc, ok, read_c8, sb, scan, sv,
};

// ---- loaded binaries -----------------------------------------------------------------------

/// One class as reported by the factory.
#[derive(Debug, Clone)]
pub(crate) struct RawClass {
    pub cid: [u8; 16],
    pub category: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub sdk_version: String,
    pub sub_categories: String,
}

/// A loaded VST3 binary and its plugin factory. Loaded binaries are cached for the life of the
/// process (see [`Bundle::load`]): plugin code must never be unmapped while an instance may run,
/// and VST3 modules are not required to support being re-entered after their exit function, which
/// therefore runs once, at process exit (see [`exit_modules`]).
pub(crate) struct Bundle {
    factory: ComPtr<sb::IPluginFactory>,
    /// The module exit matching the entry that ran at load (`bundleExit`, `ExitDll`, `ModuleExit`).
    exit: Option<ModuleExit>,
    /// The thread that ran the module entry, and so must run the exit.
    entered_on: EntryThread,
    /// Set when plugin objects were leaked on purpose (see [`EditorLink::kill`]): the module exit
    /// must not run while any of its objects are alive.
    leaked: AtomicBool,
    // Never closed: code stays mapped for the process lifetime, even after the module exit.
    _lib: ManuallyDrop<libloading::Library>,
}

/// Loaded binaries, oldest first (module exits run newest first, the reverse of loading).
fn loaded() -> &'static Mutex<Vec<(PathBuf, Arc<Bundle>)>> {
    static L: OnceLock<Mutex<Vec<(PathBuf, Arc<Bundle>)>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Vec::new()))
}

/// Set once the module exits have run: loading another module after that is refused.
static MODULES_EXITED: AtomicBool = AtomicBool::new(false);

/// Where a module's entry ran. Its exit must run on the same thread: Qt-based modules tie their
/// application object to it (Native Instruments' Massive X crashes in `bundleExit` on another
/// thread). Modules wanted on the main thread are entered there; any other thread's requests go
/// to the module thread, because the asking thread may be gone by the time the process exits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryThread {
    Main,
    Modules,
}

/// Whether this is the main thread. Not for exit handlers, where `std::thread::current` panics
/// (its thread-local data is gone by then): compare [`os_thread`] with [`MAIN_OS_THREAD`] there.
fn on_main_thread() -> bool {
    std::thread::current().name() == Some("main")
}

/// The OS id of the main thread, recorded when it loads a module (0 until then).
static MAIN_OS_THREAD: AtomicUsize = AtomicUsize::new(0);

/// The OS id of the calling thread, which (unlike `std::thread::current`) exit handlers can use.
fn os_thread() -> usize {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn pthread_self() -> usize;
        }
        // SAFETY: `pthread_self` takes no arguments and cannot fail; `pthread_t` is an integer or
        // a pointer of pointer size on the Unix systems SoundCraft builds for.
        unsafe { pthread_self() }
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
        // SAFETY: `GetCurrentThreadId` takes no arguments and cannot fail.
        unsafe { GetCurrentThreadId() as usize }
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// The module thread (started on first use): it runs jobs until the process exits.
fn module_thread() -> Option<&'static mpsc::Sender<Job>> {
    static TX: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new().name("vst3-modules".into()).spawn(move || {
            while let Ok(job) = rx.recv() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            }
        });
        spawned.ok().map(|_| tx)
    })
    .as_ref()
}

/// Runs `f` on the module thread and waits for its result (at most `timeout`, when given).
fn on_module_thread<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static, timeout: Option<Duration>) -> Option<R> {
    let (rtx, rrx) = mpsc::sync_channel(1);
    module_thread()?
        .send(Box::new(move || {
            let _ = rtx.send(f());
        }))
        .ok()?;
    match timeout {
        Some(t) => rrx.recv_timeout(t).ok(),
        None => rrx.recv().ok(),
    }
}

type GetFactory = unsafe extern "system" fn() -> *mut sb::IPluginFactory;

/// `bundleExit` (macOS), `ExitDll` (Windows) or `ModuleExit` (Linux/BSD). `extern "system"` is the
/// C ABI everywhere except 32-bit Windows, where it is the `PLUGIN_API` (stdcall) `ExitDll` uses.
type ModuleExit = unsafe extern "system" fn() -> bool;

#[cfg(target_os = "macos")]
mod cf {
    use std::ffi::c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFURLCreateFromFileSystemRepresentation(alloc: *const c_void, buf: *const u8, len: isize, is_dir: u8) -> *const c_void;
        pub fn CFBundleCreate(alloc: *const c_void, url: *const c_void) -> *mut c_void;
        pub fn CFRelease(cf: *const c_void);
    }
}

/// Opens the shared library and runs the platform module entry (`bundleEntry` with a
/// `CFBundleRef` on macOS, `InitDll` on Windows, `ModuleEntry` with the `dlopen` handle on Linux).
/// Also returns the matching module exit, when the entry ran and the module exports one.
fn open_library(bundle: &Path, exe: &Path) -> Result<(libloading::Library, Option<ModuleExit>), String> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use libloading::os::unix::{Library as UnixLib, RTLD_LOCAL, RTLD_NOW};
        // SAFETY: loading a shared library runs its static initialisers. That is inherent to
        // hosting plugins; we only load files found in VST3 plugin folders or named explicitly.
        let lib = unsafe { UnixLib::open(Some(exe), RTLD_NOW | RTLD_LOCAL) }.map_err(|e| e.to_string())?;
        type ModuleEntry = unsafe extern "C" fn(*mut c_void) -> bool;
        // SAFETY: `ModuleEntry` is the VST3-mandated Linux entry with this signature.
        let entry = unsafe { lib.get::<ModuleEntry>(b"ModuleEntry\0") }.ok().map(|s| *s);
        let raw = lib.into_raw();
        // SAFETY: `raw` is the handle we just took out of `lib`; it is rewrapped right away.
        let lib = unsafe { UnixLib::from_raw(raw) };
        let mut exit = None;
        if let Some(f) = entry {
            // SAFETY: called once, with the module's own `dlopen` handle, as the spec requires.
            if !unsafe { f(raw) } {
                return Err("ModuleEntry failed".into());
            }
            // SAFETY: `ModuleExit` is the VST3-mandated Linux exit with this signature; the
            // pointer stays valid because loaded modules are never unmapped.
            exit = unsafe { lib.get::<ModuleExit>(b"ModuleExit\0") }.ok().map(|s| *s);
        }
        let _ = bundle;
        Ok((lib.into(), exit))
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        // SAFETY: loading a shared library runs its static initialisers. That is inherent to
        // hosting plugins; we only load files found in VST3 plugin folders or named explicitly.
        let lib = unsafe { libloading::Library::new(exe) }.map_err(|e| e.to_string())?;
        let mut exit: Option<ModuleExit> = None;
        #[cfg(windows)]
        {
            let _ = bundle;
            type InitDll = unsafe extern "system" fn() -> bool;
            // SAFETY: `InitDll` is the VST3-mandated Windows entry with this signature.
            if let Ok(f) = unsafe { lib.get::<InitDll>(b"InitDll\0") } {
                // SAFETY: called once after loading, as the spec requires.
                if !unsafe { f() } {
                    return Err("InitDll failed".into());
                }
                // SAFETY: `ExitDll` is the VST3-mandated Windows exit with this signature; the
                // pointer stays valid because loaded modules are never unmapped.
                exit = unsafe { lib.get::<ModuleExit>(b"ExitDll\0") }.ok().map(|s| *s);
            }
        }
        #[cfg(target_os = "macos")]
        {
            type BundleEntry = unsafe extern "C" fn(*mut c_void) -> bool;
            // SAFETY: `bundleEntry`/`BundleEntry` is the VST3-mandated macOS entry with this signature.
            let entry = unsafe { lib.get::<BundleEntry>(b"bundleEntry\0") }
                .or_else(|_| {
                    // SAFETY: as above (older SDKs export the capitalised name).
                    unsafe { lib.get::<BundleEntry>(b"BundleEntry\0") }
                })
                .ok()
                .map(|s| *s);
            if let Some(f) = entry
                && bundle.is_dir()
            {
                use std::os::unix::ffi::OsStrExt;
                let bytes = bundle.as_os_str().as_bytes();
                let len = isize::try_from(bytes.len()).map_err(|_| "path too long".to_string())?;
                // SAFETY: `bytes` is a valid buffer of `len` bytes for the duration of the call;
                // a null allocator means the default one.
                let url = unsafe { cf::CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), len, 1) };
                if url.is_null() {
                    return Err("cannot make a CFURL for the bundle".into());
                }
                // SAFETY: `url` is a valid CFURL created above.
                let cfbundle = unsafe { cf::CFBundleCreate(std::ptr::null(), url) };
                // SAFETY: we own `url` (Create rule) and no longer need it.
                unsafe { cf::CFRelease(url) };
                if cfbundle.is_null() {
                    return Err("not a loadable bundle".into());
                }
                // SAFETY: called once with a valid CFBundleRef. The bundle ref is intentionally
                // kept (never released): the module may hold on to it for its whole life, and
                // loaded modules are never unloaded.
                if !unsafe { f(cfbundle) } {
                    return Err("bundleEntry failed".into());
                }
                // SAFETY: `bundleExit`/`BundleExit` is the VST3-mandated macOS exit with this
                // signature; the pointer stays valid because loaded modules are never unmapped.
                exit = unsafe { lib.get::<ModuleExit>(b"bundleExit\0") }
                    .or_else(|_| {
                        // SAFETY: as above (older SDKs export the capitalised name).
                        unsafe { lib.get::<ModuleExit>(b"BundleExit\0") }
                    })
                    .ok()
                    .map(|s| *s);
            }
        }
        Ok((lib, exit))
    }
}

/// Runs the module exits at process exit, newest module first, each on the thread that ran its
/// entry. VST3 modules expect their exit before their static destructors run, and some crash in
/// those destructors without it (seen with Steinberg's HALion Sonic, after a plain scan). A module
/// with a live instance or leaked objects keeps running and is not exited, and so is one entered
/// on the main thread when the process exits from another thread. Runs once; later loads are
/// refused.
pub(crate) fn exit_modules() {
    if MODULES_EXITED.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut cache = match loaded().try_lock() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        // Another thread is loading a module right now: leave every module as it is.
        Err(TryLockError::WouldBlock) => return,
    };
    let main = MAIN_OS_THREAD.load(Ordering::Acquire) == os_thread();
    let (mut kept, mut here, mut there) = (Vec::new(), Vec::new(), Vec::new());
    while let Some((path, b)) = cache.pop() {
        if b.leaked.load(Ordering::Acquire) || (b.entered_on == EntryThread::Main && !main) {
            kept.push((path, b));
            continue;
        }
        match Arc::try_unwrap(b) {
            Ok(bundle) if bundle.entered_on == EntryThread::Main => here.push(bundle),
            Ok(bundle) => there.push(bundle),
            Err(b) => kept.push((path, b)),
        }
    }
    kept.reverse();
    *cache = kept;
    drop(cache);
    here.into_iter().for_each(Bundle::exit);
    if !there.is_empty() {
        // Bounded: a module thread stuck inside a plugin must not hang the process exit.
        let _ = on_module_thread(move || there.into_iter().for_each(Bundle::exit), Some(Duration::from_secs(5)));
    }
}

/// Registers [`exit_modules`] to run at process exit. Exit handlers run in reverse order of
/// registration, C++ static destructors included, so registering again after each module load
/// puts the exits ahead of the destructors of every module loaded so far.
fn register_exit_hook() {
    unsafe extern "C" {
        fn atexit(f: extern "C" fn()) -> c_int;
    }
    extern "C" fn hook() {
        // Nothing may unwind out of an exit handler.
        let _ = std::panic::catch_unwind(exit_modules);
    }
    // SAFETY: `atexit` is the C library function with this signature; `hook` is a plain function
    // of this binary, so it lives as long as the process.
    if unsafe { atexit(hook) } != 0 {
        log::warn!("vst3: cannot register the module exit handler");
    }
}

impl Bundle {
    /// Loads (once per path, then cached for the life of the process) a `.vst3`.
    pub fn load(path: &Path) -> Result<Arc<Bundle>, Vst3Error> {
        let mut cache = loaded().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, b)) = cache.iter().find(|(p, _)| p == path) {
            return Ok(b.clone());
        }
        let err = |m: &str| Vst3Error::Load(path.display().to_string(), m.to_string());
        if MODULES_EXITED.load(Ordering::Acquire) {
            return Err(err("plugin hosting has shut down"));
        }
        let b = if on_main_thread() {
            MAIN_OS_THREAD.store(os_thread(), Ordering::Release);
            Bundle::open(path, EntryThread::Main)?
        } else {
            let p = path.to_path_buf();
            on_module_thread(move || Bundle::open(&p, EntryThread::Modules), None).ok_or_else(|| err("the module thread is not running"))??
        };
        let b = Arc::new(b);
        cache.push((path.to_path_buf(), b.clone()));
        // After every new module, so that the exits run before its static destructors.
        register_exit_hook();
        Ok(b)
    }

    fn open(path: &Path, entered_on: EntryThread) -> Result<Bundle, Vst3Error> {
        let err = |m: String| Vst3Error::Load(path.display().to_string(), m);
        let exe = scan::bundle_binary(path).ok_or_else(|| err("no loadable binary".into()))?;
        let (lib, exit) = open_library(path, &exe).map_err(err)?;
        // The module entry ran: balance it before giving up on the module.
        let fail = |m: String| {
            if let Some(f) = exit {
                // SAFETY: balanced with the entry that ran in `open_library`; no plugin object exists.
                unsafe { f() };
            }
            err(m)
        };
        // SAFETY: `GetPluginFactory` is the VST3-mandated export with this signature.
        let get = match unsafe { lib.get::<GetFactory>(b"GetPluginFactory\0") } {
            Ok(sym) => *sym,
            Err(e) => return Err(fail(format!("not a VST3 plugin: {e}"))),
        };
        // SAFETY: the module is loaded and its entry function (if any) has run.
        let raw = unsafe { get() };
        // SAFETY: a non-null result is a factory pointer with one reference owned by the caller
        // (VST3 contract), which the `ComPtr` takes over.
        let factory = unsafe { ComPtr::from_raw(raw) }.ok_or_else(|| fail("GetPluginFactory returned null".into()))?;
        Ok(Bundle { factory, exit, entered_on, leaked: AtomicBool::new(false), _lib: ManuallyDrop::new(lib) })
    }

    /// Releases the factory, then runs the module exit, as the VST3 SDK's own host does.
    fn exit(self) {
        let Bundle { factory, exit, .. } = self;
        drop(factory);
        if let Some(f) = exit {
            // SAFETY: balanced with the entry that ran when the module was loaded; called once
            // (`self` is consumed), after every instance is gone (each holds an `Arc` to the
            // bundle) and the factory released. The code stays mapped (`_lib` is never closed).
            unsafe { f() };
        }
    }

    /// Plugin objects of this module were leaked on purpose: never run its module exit.
    pub fn mark_leaked(&self) {
        self.leaked.store(true, Ordering::Release);
    }

    /// The vendor from the factory info (fallback for classes without one).
    fn factory_vendor(&self) -> String {
        // SAFETY: `PFactoryInfo` is plain data (char arrays and an int); all-zero is valid.
        let mut info: sb::PFactoryInfo = unsafe { std::mem::zeroed() };
        // SAFETY: valid factory; `info` is a writable struct of the right type.
        if ok(unsafe { self.factory.getFactoryInfo(&mut info) }) { read_c8(&info.vendor) } else { String::new() }
    }

    /// Every class this binary exports.
    pub fn classes(&self) -> Vec<RawClass> {
        // SAFETY: valid factory.
        let n = unsafe { self.factory.countClasses() }.clamp(0, MAX_CLASSES);
        let f2 = self.factory.cast::<sb::IPluginFactory2>();
        let vendor = self.factory_vendor();
        let mut out = Vec::new();
        for i in 0..n {
            if let Some(f2) = &f2 {
                // SAFETY: plain data; all-zero is valid.
                let mut c: sb::PClassInfo2 = unsafe { std::mem::zeroed() };
                // SAFETY: index below the count; `c` is writable.
                if ok(unsafe { f2.getClassInfo2(i, &mut c) }) {
                    let v = read_c8(&c.vendor);
                    out.push(RawClass {
                        cid: tuid_bytes(&c.cid),
                        category: read_c8(&c.category),
                        name: read_c8(&c.name),
                        vendor: if v.is_empty() { vendor.clone() } else { v },
                        version: read_c8(&c.version),
                        sdk_version: read_c8(&c.sdkVersion),
                        sub_categories: read_c8(&c.subCategories),
                    });
                    continue;
                }
            }
            // SAFETY: plain data; all-zero is valid.
            let mut c: sb::PClassInfo = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `c` is writable.
            if ok(unsafe { self.factory.getClassInfo(i, &mut c) }) {
                out.push(RawClass {
                    cid: tuid_bytes(&c.cid),
                    category: read_c8(&c.category),
                    name: read_c8(&c.name),
                    vendor: vendor.clone(),
                    version: String::new(),
                    sdk_version: String::new(),
                    sub_categories: String::new(),
                });
            }
        }
        out
    }

    /// Creates an object of class `cid` and queries interface `I` on it.
    fn create<I: Interface>(&self, cid: &[u8; 16]) -> Option<ComPtr<I>> {
        let mut obj: *mut c_void = std::ptr::null_mut();
        let iid = I::IID;
        // SAFETY: valid factory; `cid` and `iid` point to 16 readable bytes each (VST3 passes TUIDs
        // as `FIDString`); `obj` is writable.
        let r = unsafe { self.factory.createInstance(cid.as_ptr().cast::<c_char>(), iid.as_ptr().cast::<c_char>(), &mut obj) };
        if !ok(r) {
            return None;
        }
        // SAFETY: on success `obj` holds an `I` pointer with one reference owned by us.
        unsafe { ComPtr::from_raw(obj.cast::<I>()) }
    }

    /// Creates, initialises and wires up one plugin instance.
    pub fn instantiate(self: &Arc<Self>, cid: &[u8; 16]) -> Result<Instance, Vst3Error> {
        let name = hex(cid);
        let err = |m: &str| Vst3Error::Instantiate(name.clone(), m.to_string());
        let host = ComWrapper::new(HostContext::default());
        let host_unknown = host.as_com_ref::<sv::IHostApplication>().ok_or_else(|| err("host context"))?.upcast::<FUnknown>().as_ptr();
        let component = self.create::<sv::IComponent>(cid).ok_or_else(|| err("factory cannot create the component"))?;
        // SAFETY: fresh component; the host context outlives it (owned by the returned Instance).
        if !ok(unsafe { component.initialize(host_unknown) }) {
            return Err(err("component initialize failed"));
        }
        let mut inst = Instance {
            processor: None,
            controller: None,
            single: false,
            connections: None,
            component_initialized: true,
            controller_initialized: false,
            active: false,
            processing: false,
            max_frames: 0,
            in_params: ComWrapper::new(ParamChanges::new(PARAM_QUEUE_CAP)),
            out_params: ComWrapper::new(ParamChanges::new(PARAM_QUEUE_CAP)),
            in_events: ComWrapper::new(EventList::new(EVENT_CAP)),
            out_events: ComWrapper::new(EventList::new(EVENT_CAP)),
            // SAFETY: `ProcessContext` is plain data (numbers and small structs); all-zero is valid.
            context: unsafe { std::mem::zeroed() },
            event_in: false,
            editor: Arc::new(EditorLink::default()),
            component,
            host,
            bundle: self.clone(),
        };
        let processor = inst.component.cast::<sv::IAudioProcessor>().ok_or_else(|| err("no IAudioProcessor"))?;
        inst.processor = Some(processor);
        // Edit controller: the component itself (single-component plugin) or a separate class.
        if let Some(c) = inst.component.cast::<sv::IEditController>() {
            inst.controller = Some(c);
            inst.single = true;
        } else {
            let mut ccid: sb::TUID = [0; 16];
            // SAFETY: valid component; `ccid` is a writable 16-byte TUID.
            if ok(unsafe { inst.component.getControllerClassId(&mut ccid) })
                && let Some(c) = self.create::<sv::IEditController>(&tuid_bytes(&ccid))
            {
                // SAFETY: fresh controller; the host context outlives it.
                if ok(unsafe { c.initialize(host_unknown) }) {
                    inst.controller = Some(c);
                    inst.controller_initialized = true;
                } else {
                    log::warn!("vst3 {name}: controller initialize failed; parameters unavailable");
                }
            }
        }
        if let Some(c) = &inst.controller {
            if !inst.single
                && let (Some(a), Some(b)) = (inst.component.cast::<sv::IConnectionPoint>(), c.cast::<sv::IConnectionPoint>())
            {
                // SAFETY: both connection points are valid for as long as `inst` lives; they are
                // disconnected in `Drop` before either side terminates.
                unsafe {
                    a.connect(b.as_ptr());
                    b.connect(a.as_ptr());
                }
                inst.connections = Some((a, b));
            }
            inst.sync_controller_state();
            if let Some(h) = inst.host.as_com_ref::<sv::IComponentHandler>() {
                // SAFETY: valid controller; the handler (host context) outlives it.
                unsafe { c.setComponentHandler(h.as_ptr()) };
            }
        }
        inst.editor = Arc::new(EditorLink {
            inner: Mutex::new(EditorInner { component: Some(inst.component.clone()), controller: inst.controller.clone(), ..EditorInner::default() }),
        });
        Ok(inst)
    }
}

/// A `TUID` (`[i8; 16]`) as raw bytes.
pub(super) fn tuid_bytes(t: &sb::TUID) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (o, b) in out.iter_mut().zip(t.iter()) {
        *o = *b as u8;
    }
    out
}

/// Upper-case hex of a class id (the form used in `vst3:<hex>` ids).
pub(crate) fn hex(cid: &[u8; 16]) -> String {
    cid.iter().map(|b| format!("{b:02X}")).collect()
}
