use super::module::tuid_bytes;
use super::{
    AtomicBool, Class, ComWrapper, EDIT_CAP, FUnknown, IComponentHandlerTrait, IHostApplicationTrait, Interface, Mutex, Ordering, PoisonError,
    c_void, sb, sv, write_c16,
};

// ---- host-side COM objects -----------------------------------------------------------------

/// The host context handed to `initialize` (`IHostApplication`) and to the controller
/// (`IComponentHandler`: parameter edits made in the plugin's editor are queued for the UI, which
/// writes them into the session, from where they reach the processor).
pub(crate) struct HostContext {
    pub latency_changed: AtomicBool,
    pub restart_requested: AtomicBool,
    /// `performEdit` values (normalized), bounded by `EDIT_CAP`.
    pub edits: Mutex<Vec<(u32, f64)>>,
}

impl Default for HostContext {
    fn default() -> Self {
        HostContext {
            latency_changed: AtomicBool::new(false),
            restart_requested: AtomicBool::new(false),
            edits: Mutex::new(Vec::with_capacity(EDIT_CAP)),
        }
    }
}

impl HostContext {
    /// Takes the queued editor edits.
    pub fn take_edits(&self) -> Vec<(u32, f64)> {
        let mut e = self.edits.lock().unwrap_or_else(PoisonError::into_inner);
        let out = e.clone();
        e.clear();
        out
    }
}

impl Class for HostContext {
    type Interfaces = (sv::IHostApplication, sv::IComponentHandler);
}

impl IHostApplicationTrait for HostContext {
    unsafe fn getName(&self, name: *mut sv::String128) -> sb::tresult {
        // SAFETY: the plugin passes a pointer to a writable String128 (or null, handled).
        match unsafe { name.as_mut() } {
            Some(n) => {
                write_c16("SoundCraft", n);
                sb::kResultOk
            }
            None => sb::kInvalidArgument,
        }
    }

    /// Creates the objects plugins may ask the host for: `IMessage` (what a controller and its
    /// processor send each other through `IConnectionPoint::notify`) and `IAttributeList`.
    /// Plugins that talk between their halves this way fail without them (Line 6 Helix
    /// reported "Failed to get content database recovery info (Out of memory condition)").
    unsafe fn createInstance(&self, cid: *mut sb::TUID, iid: *mut sb::TUID, obj: *mut *mut c_void) -> sb::tresult {
        // SAFETY: `obj` is the plugin's out-pointer (or null, handled).
        let Some(out) = (unsafe { obj.as_mut() }) else { return sb::kInvalidArgument };
        *out = std::ptr::null_mut();
        // SAFETY: `cid` and `iid` point to 16-byte TUIDs (or are null, handled).
        let (Some(cid), Some(iid)) = (unsafe { cid.as_ref() }, unsafe { iid.as_ref() }) else { return sb::kInvalidArgument };
        let (cid, iid) = (tuid_bytes(cid), tuid_bytes(iid));
        let wants = |class: [u8; 16]| cid == class && (iid == class || iid == FUnknown::IID);
        let made = if wants(sv::IMessage::IID) {
            ComWrapper::new(HostMessage::default()).to_com_ptr::<sv::IMessage>().map(|p| p.into_raw().cast::<c_void>())
        } else if wants(sv::IAttributeList::IID) {
            ComWrapper::new(HostAttributes::default()).to_com_ptr::<sv::IAttributeList>().map(|p| p.into_raw().cast::<c_void>())
        } else {
            None
        };
        match made {
            Some(p) => {
                *out = p;
                sb::kResultOk
            }
            None => sb::kResultFalse,
        }
    }
}

impl IComponentHandlerTrait for HostContext {
    unsafe fn beginEdit(&self, _id: sv::ParamID) -> sb::tresult {
        sb::kResultOk
    }
    unsafe fn performEdit(&self, id: sv::ParamID, value: sv::ParamValue) -> sb::tresult {
        if value.is_finite() {
            let mut e = self.edits.lock().unwrap_or_else(PoisonError::into_inner);
            let v = value.clamp(0.0, 1.0);
            if let Some(x) = e.iter_mut().find(|(i, _)| *i == id) {
                x.1 = v;
            } else if e.len() < EDIT_CAP {
                e.push((id, v));
            }
        }
        sb::kResultOk
    }
    unsafe fn endEdit(&self, _id: sv::ParamID) -> sb::tresult {
        sb::kResultOk
    }
    unsafe fn restartComponent(&self, flags: sb::int32) -> sb::tresult {
        if flags as u32 & sv::RestartFlags_::kLatencyChanged as u32 != 0 {
            self.latency_changed.store(true, Ordering::Relaxed);
        }
        self.restart_requested.store(true, Ordering::Relaxed);
        sb::kResultOk
    }
}

/// Longest string attribute kept, in UTF-16 units (longer ones are cut).
const ATTR_STRING_MAX: usize = 1 << 20;

/// Largest binary attribute kept.
const ATTR_BINARY_MAX: usize = 256 << 20;

/// One `IAttributeList` value. Strings and blobs live in their own allocation, so a pointer
/// handed out by `getBinary` stays valid while the list holds the value.
enum Attr {
    Int(i64),
    Float(f64),
    Str(Box<[u16]>),
    Bin(Box<[u8]>),
}

/// The host's `IAttributeList`: what a plugin puts in an `IMessage`.
#[derive(Default)]
pub(crate) struct HostAttributes {
    values: Mutex<std::collections::HashMap<Vec<u8>, Attr>>,
}

impl Class for HostAttributes {
    type Interfaces = (sv::IAttributeList,);
}

/// An attribute id's bytes (`None` for a null id).
fn attr_key(id: sv::IAttributeList_::AttrID) -> Option<Vec<u8>> {
    // SAFETY: a non-null AttrID is a NUL-terminated C string owned by the caller.
    (!id.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(id) }.to_bytes().to_vec())
}

impl HostAttributes {
    fn values(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<Vec<u8>, Attr>> {
        self.values.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, id: sv::IAttributeList_::AttrID, v: Attr) -> sb::tresult {
        let Some(k) = attr_key(id) else { return sb::kInvalidArgument };
        self.values().insert(k, v);
        sb::kResultOk
    }
}

impl sv::IAttributeListTrait for HostAttributes {
    unsafe fn setInt(&self, id: sv::IAttributeList_::AttrID, value: sb::int64) -> sb::tresult {
        self.set(id, Attr::Int(value))
    }

    unsafe fn getInt(&self, id: sv::IAttributeList_::AttrID, value: *mut sb::int64) -> sb::tresult {
        // SAFETY: `value` is the caller's out-pointer (or null, handled).
        match (
            attr_key(id).and_then(|k| match self.values().get(&k) {
                Some(Attr::Int(v)) => Some(*v),
                _ => None,
            }),
            unsafe { value.as_mut() },
        ) {
            (Some(v), Some(out)) => {
                *out = v;
                sb::kResultOk
            }
            _ => sb::kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: sv::IAttributeList_::AttrID, value: f64) -> sb::tresult {
        self.set(id, Attr::Float(value))
    }

    unsafe fn getFloat(&self, id: sv::IAttributeList_::AttrID, value: *mut f64) -> sb::tresult {
        // SAFETY: `value` is the caller's out-pointer (or null, handled).
        match (
            attr_key(id).and_then(|k| match self.values().get(&k) {
                Some(Attr::Float(v)) => Some(*v),
                _ => None,
            }),
            unsafe { value.as_mut() },
        ) {
            (Some(v), Some(out)) => {
                *out = v;
                sb::kResultOk
            }
            _ => sb::kResultFalse,
        }
    }

    unsafe fn setString(&self, id: sv::IAttributeList_::AttrID, string: *const sv::TChar) -> sb::tresult {
        if string.is_null() {
            return sb::kInvalidArgument;
        }
        let mut s = Vec::new();
        // SAFETY: a non-null string is NUL-terminated UTF-16 owned by the caller; reading stops at
        // the NUL or at ATTR_STRING_MAX units.
        while s.len() < ATTR_STRING_MAX {
            let c = unsafe { *string.add(s.len()) };
            if c == 0 {
                break;
            }
            s.push(c);
        }
        self.set(id, Attr::Str(s.into_boxed_slice()))
    }

    unsafe fn getString(&self, id: sv::IAttributeList_::AttrID, string: *mut sv::TChar, size_in_bytes: sb::uint32) -> sb::tresult {
        let units = size_in_bytes as usize / 2;
        if string.is_null() || units == 0 {
            return sb::kInvalidArgument;
        }
        let Some(k) = attr_key(id) else { return sb::kInvalidArgument };
        let values = self.values();
        let Some(Attr::Str(s)) = values.get(&k) else { return sb::kResultFalse };
        let n = s.len().min(units - 1);
        // SAFETY: the caller's buffer holds `units` UTF-16 units; `n + 1 <= units` are written.
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), string, n);
            *string.add(n) = 0;
        }
        sb::kResultOk
    }

    unsafe fn setBinary(&self, id: sv::IAttributeList_::AttrID, data: *const c_void, size_in_bytes: sb::uint32) -> sb::tresult {
        let n = size_in_bytes as usize;
        if n > ATTR_BINARY_MAX {
            return sb::kOutOfMemory;
        }
        let bytes: Box<[u8]> = if n == 0 || data.is_null() {
            Box::new([])
        } else {
            // SAFETY: the caller's `data` holds `n` readable bytes.
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), n) }.into()
        };
        self.set(id, Attr::Bin(bytes))
    }

    unsafe fn getBinary(&self, id: sv::IAttributeList_::AttrID, data: *mut *const c_void, size_in_bytes: *mut sb::uint32) -> sb::tresult {
        let Some(k) = attr_key(id) else { return sb::kInvalidArgument };
        let values = self.values();
        let Some(Attr::Bin(b)) = values.get(&k) else { return sb::kResultFalse };
        // SAFETY: both are the caller's out-pointers (or null, handled). The pointer handed out
        // stays valid until this attribute is replaced or the list is released (VST3 contract).
        match (unsafe { data.as_mut() }, unsafe { size_in_bytes.as_mut() }) {
            (Some(d), Some(sz)) => {
                *d = b.as_ptr().cast::<c_void>();
                *sz = u32::try_from(b.len()).unwrap_or(u32::MAX);
                sb::kResultOk
            }
            _ => sb::kInvalidArgument,
        }
    }
}

/// The host's `IMessage`: an id and an attribute list, created for a plugin through
/// `IHostApplication::createInstance`.
pub(crate) struct HostMessage {
    id: Mutex<std::ffi::CString>,
    attributes: ComWrapper<HostAttributes>,
}

impl Default for HostMessage {
    fn default() -> Self {
        HostMessage { id: Mutex::new(std::ffi::CString::default()), attributes: ComWrapper::new(HostAttributes::default()) }
    }
}

impl Class for HostMessage {
    type Interfaces = (sv::IMessage,);
}

impl sv::IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> sb::FIDString {
        // The string's buffer lives until the next `setMessageID`, as the interface expects.
        self.id.lock().unwrap_or_else(PoisonError::into_inner).as_ptr()
    }

    unsafe fn setMessageID(&self, id: sb::FIDString) {
        // SAFETY: a non-null id is a NUL-terminated C string owned by the caller.
        let s = if id.is_null() { std::ffi::CString::default() } else { unsafe { std::ffi::CStr::from_ptr(id) }.to_owned() };
        *self.id.lock().unwrap_or_else(PoisonError::into_inner) = s;
    }

    unsafe fn getAttributes(&self) -> *mut sv::IAttributeList {
        // Not add-ref'ed: the list belongs to the message (VST3 contract).
        self.attributes.as_com_ref::<sv::IAttributeList>().map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
}
