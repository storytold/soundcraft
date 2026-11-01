use super::gui::{GuiInner, GuiLink};
use super::host::{HostState, host_get_extension, host_request_callback, host_request_process, host_request_restart};
use super::instance::Instance;
use super::{
    Arc, CLAP_EXT_AUDIO_PORTS, CLAP_EXT_GUI, CLAP_EXT_LATENCY, CLAP_EXT_NOTE_PORTS, CLAP_EXT_PARAMS, CLAP_EXT_STATE, CLAP_PLUGIN_FACTORY_ID,
    CLAP_VERSION, CString, ClapError, EVENT_CAP, HashMap, MAX_FEATURES, MAX_PLUGINS_PER_FACTORY, Mutex, OnceLock, Path, PathBuf, PoisonError, c_void,
    clap_host, clap_plugin_audio_ports, clap_plugin_entry, clap_plugin_factory, clap_plugin_gui, clap_plugin_latency, clap_plugin_note_ports,
    clap_plugin_params, clap_plugin_state, clap_version_is_compatible, read_cstr, scan,
};

// ---- loaded binaries -----------------------------------------------------------------------

/// A plugin descriptor as reported by the factory.
#[derive(Debug, Clone)]
pub(crate) struct RawDescriptor {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub description: String,
    pub features: Vec<String>,
}

/// A loaded, initialised CLAP binary. Loaded binaries are cached for the life of the process
/// (see [`Bundle::load`]): plugin code must never be unmapped while an instance may still run.
pub(crate) struct Bundle {
    entry: *const clap_plugin_entry,
    factory: *const clap_plugin_factory,
    // Dropped last: the library must outlive every pointer above.
    _lib: libloading::Library,
}

// SAFETY: CLAP requires the entry and the plugin factory to be thread-safe; the raw pointers are
// read-only vtables inside the loaded library, which `_lib` keeps mapped.
unsafe impl Send for Bundle {}

// SAFETY: as above; `Bundle` has no interior mutability on the Rust side.
unsafe impl Sync for Bundle {}

fn loaded() -> &'static Mutex<HashMap<PathBuf, Arc<Bundle>>> {
    static L: OnceLock<Mutex<HashMap<PathBuf, Arc<Bundle>>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

impl Bundle {
    /// Loads (once per path, then cached for the life of the process) and initialises a `.clap`.
    pub fn load(path: &Path) -> Result<Arc<Bundle>, ClapError> {
        let mut map = loaded().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(b) = map.get(path) {
            return Ok(b.clone());
        }
        let b = Arc::new(Bundle::open(path)?);
        map.insert(path.to_path_buf(), b.clone());
        Ok(b)
    }

    fn open(path: &Path) -> Result<Bundle, ClapError> {
        let err = |m: String| ClapError::Load(path.display().to_string(), m);
        let exe = scan::bundle_executable(path).ok_or_else(|| err("no loadable binary".into()))?;
        // SAFETY: loading a shared library runs its static initialisers. That is inherent to
        // hosting plugins; we only load files found in CLAP plugin folders or named explicitly.
        let lib = unsafe { libloading::Library::new(&exe) }.map_err(|e| err(e.to_string()))?;
        // SAFETY: `clap_entry` is the CLAP-mandated exported data symbol of type
        // `const clap_plugin_entry`; the symbol's address is therefore a `*const clap_plugin_entry`.
        let entry: *const clap_plugin_entry = match unsafe { lib.get::<*const clap_plugin_entry>(b"clap_entry\0") } {
            Ok(sym) => *sym,
            Err(e) => return Err(err(format!("not a CLAP plugin: {e}"))),
        };
        if entry.is_null() {
            return Err(err("null clap_entry".into()));
        }
        // SAFETY: non-null pointer to the entry struct inside the library, which `lib` keeps mapped.
        let e = unsafe { *entry };
        if !clap_version_is_compatible(e.clap_version) {
            return Err(err(format!("unsupported CLAP version {}.{}", e.clap_version.major, e.clap_version.minor)));
        }
        let (Some(init), Some(get_factory)) = (e.init, e.get_factory) else { return Err(err("incomplete clap_entry".into())) };
        let cpath = CString::new(path.to_string_lossy().into_owned()).map_err(|_| err("path contains NUL".into()))?;
        // SAFETY: `init` comes from a valid entry; the path is a valid NUL-terminated string that
        // outlives the call.
        if !unsafe { init(cpath.as_ptr()) } {
            return Err(err("clap_entry.init failed".into()));
        }
        // SAFETY: the entry is initialised; the id is a static NUL-terminated string.
        let factory = unsafe { get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) } as *const clap_plugin_factory;
        if factory.is_null() {
            if let Some(deinit) = e.deinit {
                // SAFETY: balanced with the successful `init` above.
                unsafe { deinit() };
            }
            return Err(err("no plugin factory".into()));
        }
        Ok(Bundle { entry, factory, _lib: lib })
    }

    fn factory(&self) -> clap_plugin_factory {
        // SAFETY: `factory` was non-null at load time and points into the still-mapped library.
        unsafe { *self.factory }
    }

    /// The plugins this binary provides.
    pub fn descriptors(&self) -> Vec<RawDescriptor> {
        let f = self.factory();
        let (Some(count), Some(get)) = (f.get_plugin_count, f.get_plugin_descriptor) else { return Vec::new() };
        // SAFETY: vtable call with the factory pointer it belongs to.
        let n = unsafe { count(self.factory) }.min(MAX_PLUGINS_PER_FACTORY);
        let mut out = Vec::new();
        for i in 0..n {
            // SAFETY: index is below the reported count.
            let d = unsafe { get(self.factory, i) };
            if d.is_null() {
                continue;
            }
            // SAFETY: non-null descriptor owned by the plugin library, valid while it is loaded.
            let d = unsafe { *d };
            // SAFETY: descriptor string fields are null or NUL-terminated (CLAP contract).
            let Some(id) = (unsafe { read_cstr(d.id) }).filter(|s| !s.is_empty()) else { continue };
            // SAFETY: as above.
            let name = unsafe { read_cstr(d.name) }.unwrap_or_else(|| id.clone());
            let mut features = Vec::new();
            if !d.features.is_null() {
                for k in 0..MAX_FEATURES {
                    // SAFETY: `features` is a NULL-terminated array; we stop at the terminator
                    // (or the cap) and never read past it.
                    let p = unsafe { *d.features.add(k) };
                    if p.is_null() {
                        break;
                    }
                    // SAFETY: a non-null entry of the feature array is a NUL-terminated string.
                    if let Some(s) = unsafe { read_cstr(p) } {
                        features.push(s);
                    }
                }
            }
            out.push(RawDescriptor {
                id,
                name,
                // SAFETY: descriptor string fields are null or NUL-terminated.
                vendor: unsafe { read_cstr(d.vendor) }.unwrap_or_default(),
                // SAFETY: as above.
                version: unsafe { read_cstr(d.version) }.unwrap_or_default(),
                // SAFETY: as above.
                description: unsafe { read_cstr(d.description) }.unwrap_or_default(),
                features,
            });
        }
        out
    }

    /// Creates and initialises one plugin instance.
    pub fn instantiate(self: &Arc<Self>, plugin_id: &str) -> Result<Instance, ClapError> {
        let err = |m: &str| ClapError::Instantiate(plugin_id.to_string(), m.to_string());
        let create = self.factory().create_plugin.ok_or_else(|| err("factory cannot create plugins"))?;
        let cid = CString::new(plugin_id).map_err(|_| err("id contains NUL"))?;
        let state = Arc::new(HostState::default());
        let host = Box::new(clap_host {
            clap_version: CLAP_VERSION,
            host_data: Arc::as_ptr(&state).cast_mut().cast::<c_void>(),
            name: c"SoundCraft".as_ptr(),
            vendor: c"SoundCraft contributors".as_ptr(),
            url: c"https://github.com/storytold/soundcraft".as_ptr(),
            version: c"0.1.0".as_ptr(),
            get_extension: Some(host_get_extension),
            request_restart: Some(host_request_restart),
            request_process: Some(host_request_process),
            request_callback: Some(host_request_callback),
        });
        // SAFETY: the factory is valid; `host` is boxed (stable address) and owned by the returned
        // `Instance`, which destroys the plugin before dropping the box; `cid` outlives the call.
        let plugin = unsafe { create(self.factory, &*host, cid.as_ptr()) };
        if plugin.is_null() {
            return Err(err("factory returned no instance"));
        }
        let mut inst = Instance {
            plugin,
            params: std::ptr::null(),
            ports: std::ptr::null(),
            latency: std::ptr::null(),
            notes: std::ptr::null(),
            state_ext: std::ptr::null(),
            gui: Arc::new(GuiLink::default()),
            raw: Vec::with_capacity(EVENT_CAP),
            out_params: Vec::with_capacity(EVENT_CAP),
            active: false,
            processing: false,
            max_frames: 0,
            created: true,
            _host: host,
            state,
            _bundle: self.clone(),
        };
        // SAFETY: `plugin` is a fresh non-null instance; we copy its vtable struct.
        let vt = unsafe { *plugin };
        let ok = match vt.init {
            // SAFETY: `init` is called exactly once, right after creation, as CLAP requires.
            Some(init) => unsafe { init(plugin) },
            None => false,
        };
        if !ok {
            // `Drop` destroys the instance (without deactivating: it never activated).
            return Err(err("plugin init failed"));
        }
        inst.params = inst.extension::<clap_plugin_params>(CLAP_EXT_PARAMS);
        inst.ports = inst.extension::<clap_plugin_audio_ports>(CLAP_EXT_AUDIO_PORTS);
        inst.latency = inst.extension::<clap_plugin_latency>(CLAP_EXT_LATENCY);
        inst.notes = inst.extension::<clap_plugin_note_ports>(CLAP_EXT_NOTE_PORTS);
        inst.state_ext = inst.extension::<clap_plugin_state>(CLAP_EXT_STATE);
        let gui = inst.extension::<clap_plugin_gui>(CLAP_EXT_GUI);
        inst.gui = Arc::new(GuiLink {
            inner: Mutex::new(GuiInner { plugin, gui, state: inst.state_ext, alive: true, created: false, visible: false, window: None }),
            host: Arc::clone(&inst.state),
        });
        Ok(inst)
    }
}

impl Drop for Bundle {
    fn drop(&mut self) {
        // SAFETY: `entry` was valid at load time and `_lib` (dropped after this) keeps it mapped.
        let e = unsafe { *self.entry };
        if let Some(deinit) = e.deinit {
            // SAFETY: balanced with the successful `init` in `open`; no instance holds an `Arc`
            // to this bundle any more (they keep it alive), so no plugin code still runs.
            unsafe { deinit() };
        }
    }
}
