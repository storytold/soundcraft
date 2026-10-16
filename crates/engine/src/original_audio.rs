//! Native encoded import receipts. Working PCM remains independent of the retained bytes.
//! File IO and verification happen on the control side; snapshot destruction only signals a
//! bounded collector channel. Published files are never garbage-collected.

#[cfg(not(target_arch = "wasm32"))]
pub const ORIGINAL_VERSION: u32 = 1;
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_ORIGINAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_ORIGINAL_ASSETS: usize = 10_000;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{MAX_ORIGINAL_ASSETS, MAX_ORIGINAL_BYTES, ORIGINAL_VERSION};
    use crate::io::original_files as storage;
    use crate::{EngineError, Result};
    use same_file::Handle;
    use sha2::{Digest, Sha256};
    use soundcraft_model::{OriginalAssetLease, OriginalAssetStamp, OriginalProvenance, Session, SourceId};
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
    use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_RECEIPT: AtomicU64 = AtomicU64::new(1);
    static CLOSED_PENDING: AtomicBool = AtomicBool::new(false);
    static RECEIPTS: OnceLock<Mutex<BTreeMap<u64, Weak<Mutex<State>>>>> = OnceLock::new();
    static PUBLISHED: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();

    fn error(path: &Path, error: impl std::fmt::Display) -> EngineError {
        EngineError::Io(format!("{}: {error}", path.display()))
    }

    fn next_receipt() -> Result<u64> {
        NEXT_RECEIPT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| EngineError::Io("original asset receipt limit reached".into()))
    }

    fn receipts() -> &'static Mutex<BTreeMap<u64, Weak<Mutex<State>>>> {
        RECEIPTS.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    #[derive(Debug)]
    struct Record {
        lease: Weak<OriginalAssetLease>,
        path: PathBuf,
        identity: Handle,
        /// Only private scratch files are owned by this collector.
        owned: bool,
    }

    #[derive(Debug)]
    struct Directory {
        path: PathBuf,
        identity: Handle,
    }

    #[derive(Debug, Default)]
    struct State {
        directory: Option<Directory>,
        records: BTreeMap<u64, Record>,
        closed: Arc<AtomicBool>,
    }

    #[derive(Debug, Clone)]
    struct Worker {
        state: Arc<Mutex<State>>,
        signal: SyncSender<()>,
        closed: Arc<AtomicBool>,
    }

    fn same_identity(path: &Path, identity: &Handle, kind: storage::Kind) -> bool {
        storage::inspect(path).is_ok_and(|metadata| storage::kind(&metadata) == kind)
            && Handle::from_path(path).is_ok_and(|current| current == *identity)
    }

    /// This runs only on a control or collector thread, never from a lease destructor.
    fn collect(state: &mut State) {
        state.records.retain(|id, record| {
            if record.lease.strong_count() != 0 {
                return true;
            }
            // The original handle prevents file-id reuse; a replacement is left alone.
            if record.owned && same_identity(&record.path, &record.identity, storage::Kind::File) {
                let _ = storage::remove_file(&record.path);
            }
            receipts().lock().unwrap_or_else(PoisonError::into_inner).remove(id);
            false
        });
        if state.closed.load(Ordering::Acquire)
            && state.records.is_empty()
            && let Some(directory) = state.directory.take()
            && same_identity(&directory.path, &directory.identity, storage::Kind::Directory)
        {
            // Remove only an empty directory we created, never a recursive/user tree.
            let _ = storage::remove_dir(&directory.path);
        }
    }

    fn collector(state: Arc<Mutex<State>>, receiver: Receiver<()>) {
        while receiver.recv().is_ok() {
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            collect(&mut state);
            if state.closed.load(Ordering::Acquire) && state.records.is_empty() {
                return;
            }
        }
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        state.closed.store(true, Ordering::Release);
        collect(&mut state);
    }

    fn start_worker(directory: Option<Directory>) -> Result<Worker> {
        let closed = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(State { directory, closed: Arc::clone(&closed), ..State::default() }));
        let (signal, receiver) = sync_channel(1);
        let work = Arc::clone(&state);
        if let Err(err) = std::thread::Builder::new().name("soundcraft-originals".into()).spawn(move || collector(work, receiver)) {
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            state.closed.store(true, Ordering::Release);
            collect(&mut state);
            return Err(EngineError::Io(format!("cannot start original asset collector: {err}")));
        }
        Ok(Worker { state, signal, closed })
    }

    fn published_worker() -> Result<Worker> {
        let mut worker = PUBLISHED.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(PoisonError::into_inner);
        if worker.is_none() {
            *worker = Some(start_worker(None)?);
        }
        worker.clone().ok_or_else(|| EngineError::Io("original asset collector unavailable".into()))
    }

    fn stamp(id: u64, metadata: &storage::Metadata) -> OriginalAssetStamp {
        OriginalAssetStamp { receipt_id: id, len: metadata.len(), modified: metadata.modified().ok(), created: metadata.created().ok() }
    }

    fn register(worker: &Worker, path: PathBuf, sha256: String, bytes: u64, identity: Handle, owned: bool) -> Result<Arc<OriginalAssetLease>> {
        let setup = (|| {
            let id = next_receipt()?;
            let metadata = identity.as_file().metadata().map_err(|err| error(&path, err))?;
            if metadata.len() != bytes || !same_identity(&path, &identity, storage::Kind::File) {
                return Err(error(&path, "original asset changed while being registered"));
            }
            Ok(stamp(id, &metadata))
        })();
        let stamp = match setup {
            Ok(stamp) => stamp,
            Err(err) => {
                if owned && same_identity(&path, &identity, storage::Kind::File) {
                    let _ = storage::remove_file(&path);
                }
                return Err(err);
            }
        };
        let id = stamp.receipt_id;
        let lease = Arc::new(OriginalAssetLease::new(path.clone(), sha256, bytes, stamp, Some(worker.signal.clone())));
        worker
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .records
            .insert(id, Record { lease: Arc::downgrade(&lease), path, identity, owned });
        receipts().lock().unwrap_or_else(PoisonError::into_inner).insert(id, Arc::downgrade(&worker.state));
        Ok(lease)
    }

    fn validate(metadata: &OriginalProvenance) -> Result<()> {
        if metadata.version != ORIGINAL_VERSION {
            return Err(EngineError::Io(format!("unsupported original asset version {}", metadata.version)));
        }
        if metadata.bytes > MAX_ORIGINAL_BYTES
            || metadata.sha256.len() != 64
            || !metadata.sha256.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || metadata.path != format!("Original Files/{}.asset", metadata.sha256)
        {
            return Err(EngineError::Io("invalid original asset receipt".into()));
        }
        Ok(())
    }

    fn inspect_regular(path: &Path) -> Result<storage::Metadata> {
        let metadata = storage::inspect(path).map_err(|err| error(path, err))?;
        if storage::kind(&metadata) != storage::Kind::File {
            return Err(error(path, "original asset is not a regular file"));
        }
        Ok(metadata)
    }

    fn verify_cache(lease: &OriginalAssetLease) -> Result<()> {
        let state = receipts()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&lease.stamp.receipt_id)
            .and_then(Weak::upgrade)
            .ok_or_else(|| error(&lease.path, "original asset verification receipt unavailable"))?;
        let state = state.lock().unwrap_or_else(PoisonError::into_inner);
        let record =
            state.records.get(&lease.stamp.receipt_id).ok_or_else(|| error(&lease.path, "original asset verification receipt unavailable"))?;
        if record.lease.as_ptr() != std::ptr::from_ref(lease) {
            return Err(error(&lease.path, "original asset verification receipt does not own this lease"));
        }
        // Captured scratch bytes are app-owned, already read with import authority. Published
        // files are user paths and still require the current automation read grant.
        if !record.owned {
            crate::file_access::check_read(&lease.path).map_err(EngineError::Io)?;
        }
        let metadata = inspect_regular(&lease.path)?;
        if stamp(lease.stamp.receipt_id, &metadata) != lease.stamp || metadata.len() != lease.bytes {
            return Err(error(&lease.path, "original asset changed since verification"));
        }
        if !same_identity(&lease.path, &record.identity, storage::Kind::File) {
            return Err(error(&lease.path, "original asset was replaced since verification"));
        }
        if lease.stamp.modified.is_none() {
            // A filesystem without a modification stamp cannot support the cheap cache check.
            let mut file = storage::open(&lease.path).map_err(|err| error(&lease.path, err))?;
            if digest_reader(&mut file, lease.bytes, None)? != lease.sha256 || !same_identity(&lease.path, &record.identity, storage::Kind::File) {
                return Err(error(&lease.path, "original asset checksum differs from its receipt"));
            }
        }
        Ok(())
    }

    fn checked_folder(dir: &Path, write: bool) -> Result<PathBuf> {
        // A plain `Project.scraft` has an empty parent and uses the current folder.
        let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        if write {
            crate::file_access::check_write(dir).map_err(EngineError::Io)?;
        } else {
            crate::file_access::check_read(dir).map_err(EngineError::Io)?;
        }
        let metadata = storage::inspect(dir).map_err(|err| error(dir, err))?;
        if storage::kind(&metadata) != storage::Kind::Directory {
            return Err(error(dir, "original asset root is not a directory"));
        }
        let folder = dir.join("Original Files");
        match storage::inspect(&folder) {
            Ok(metadata) if storage::kind(&metadata) == storage::Kind::Directory => {}
            Ok(_) => return Err(error(&folder, "original asset folder is not a directory")),
            Err(err) if write && err.kind() == std::io::ErrorKind::NotFound => {
                crate::file_access::check_write(&folder).map_err(EngineError::Io)?;
                storage::make_dir(&folder).map_err(|err| error(&folder, err))?;
            }
            Err(err) => return Err(error(&folder, err)),
        }
        Ok(folder)
    }

    fn digest_reader(reader: &mut impl Read, expected_bytes: u64, output: Option<&mut storage::File>) -> Result<String> {
        let mut output = output;
        let mut digest = Sha256::new();
        let mut bytes = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let size = reader.read(&mut buffer).map_err(|err| EngineError::Io(format!("cannot read original asset: {err}")))?;
            if size == 0 {
                break;
            }
            bytes = bytes
                .checked_add(size as u64)
                .filter(|bytes| *bytes <= expected_bytes && *bytes <= MAX_ORIGINAL_BYTES)
                .ok_or_else(|| EngineError::Io("original asset exceeds its declared size".into()))?;
            let part = buffer.get(..size).ok_or_else(|| EngineError::Io("invalid original read length".into()))?;
            digest.update(part);
            if let Some(output) = output.as_mut() {
                output.write_all(part).map_err(|err| EngineError::Io(format!("cannot write original asset: {err}")))?;
            }
        }
        if bytes != expected_bytes {
            return Err(EngineError::Io("original asset size differs from its receipt".into()));
        }
        record_hash_read();
        Ok(format!("{:x}", digest.finalize()))
    }

    fn verified_published(path: &Path, metadata: &OriginalProvenance) -> Result<Arc<OriginalAssetLease>> {
        crate::file_access::check_read(path).map_err(EngineError::Io)?;
        let before = inspect_regular(path)?;
        if before.len() != metadata.bytes {
            return Err(error(path, "original asset size differs from its receipt"));
        }
        let mut file = storage::open(path).map_err(|err| error(path, err))?;
        let identity = Handle::from_file(file.try_clone().map_err(|err| error(path, err))?).map_err(|err| error(path, err))?;
        if digest_reader(&mut file, metadata.bytes, None)? != metadata.sha256 {
            return Err(error(path, "original asset checksum differs from its receipt"));
        }
        if stamp(0, &inspect_regular(path)?) != stamp(0, &before) || !same_identity(path, &identity, storage::Kind::File) {
            return Err(error(path, "original asset changed while being verified"));
        }
        register(&published_worker()?, path.to_path_buf(), metadata.sha256.clone(), metadata.bytes, identity, false)
    }

    /// Lazy private staging scope. No file is created until a decoded import is accepted.
    #[derive(Debug, Default)]
    pub struct OriginalStore {
        worker: Option<Worker>,
    }

    impl OriginalStore {
        pub fn new() -> Self {
            Self::default()
        }

        fn worker(&mut self) -> Result<&Worker> {
            if self.worker.as_ref().is_some_and(|worker| worker.closed.load(Ordering::Acquire)) {
                return Err(EngineError::Io("original staging scope closed".into()));
            }
            if self.worker.is_none() {
                let epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
                let serial = next_receipt()?;
                let path = std::env::temp_dir().join(format!("soundcraft-originals-{}-{epoch}-{serial}", std::process::id()));
                storage::make_private_dir(&path).map_err(|err| error(&path, err))?;
                let identity = match Handle::from_path(&path) {
                    Ok(identity) => identity,
                    Err(err) => {
                        let _ = storage::remove_dir(&path);
                        return Err(error(&path, err));
                    }
                };
                self.worker = Some(start_worker(Some(Directory { path, identity }))?);
            }
            self.worker.as_ref().ok_or_else(|| EngineError::Io("original staging scope unavailable".into()))
        }

        pub fn capture(&mut self, bytes: &[u8]) -> Result<(OriginalProvenance, Arc<OriginalAssetLease>)> {
            let size = u64::try_from(bytes.len()).map_err(|_| EngineError::Io("original asset is too large".into()))?;
            if size > MAX_ORIGINAL_BYTES {
                return Err(EngineError::Io("original asset is too large".into()));
            }
            let worker = self.worker()?.clone();
            let path = {
                let mut state = worker.state.lock().unwrap_or_else(PoisonError::into_inner);
                collect(&mut state);
                let total = state
                    .records
                    .values()
                    .filter_map(|record| record.lease.upgrade())
                    .try_fold(size, |sum, lease| sum.checked_add(lease.bytes))
                    .ok_or_else(|| EngineError::Io("original staging byte limit reached".into()))?;
                if total > MAX_ORIGINAL_BYTES || state.records.len() >= MAX_ORIGINAL_ASSETS {
                    return Err(EngineError::Io("original staging limit reached".into()));
                }
                let directory = state.directory.as_ref().ok_or_else(|| EngineError::Io("original staging scope closed".into()))?;
                if !same_identity(&directory.path, &directory.identity, storage::Kind::Directory) {
                    return Err(error(&directory.path, "original staging folder was replaced"));
                }
                directory.path.join(format!("{}.asset", next_receipt()?))
            };
            let file = storage::exclusive_private(&path).map_err(|err| error(&path, err))?;
            let mut identity = Handle::from_file(file).map_err(|err| error(&path, err))?;
            let mut input = bytes;
            let digest = digest_reader(&mut input, size, Some(identity.as_file_mut()));
            let digest = match digest.and_then(|digest| identity.as_file().sync_all().map(|()| digest).map_err(|err| error(&path, err))) {
                Ok(digest) => digest,
                Err(err) => {
                    if same_identity(&path, &identity, storage::Kind::File) {
                        let _ = storage::remove_file(&path);
                    }
                    return Err(err);
                }
            };
            let lease = register(&worker, path, digest.clone(), size, identity, true)?;
            let metadata =
                OriginalProvenance { version: ORIGINAL_VERSION, path: format!("Original Files/{digest}.asset"), sha256: digest, bytes: size };
            Ok((metadata, lease))
        }

        /// Bounded control-side collection, useful when closing an engine without external snapshots.
        pub fn collect_expired(&self) {
            if let Some(worker) = &self.worker {
                let mut state = worker.state.lock().unwrap_or_else(PoisonError::into_inner);
                collect(&mut state);
            }
        }

        /// Close and collect on the control side before an orderly process exit. External
        /// snapshots still own their leases and wake the worker after their last release.
        pub fn close(&self) {
            if let Some(worker) = &self.worker {
                worker.closed.store(true, Ordering::Release);
                CLOSED_PENDING.store(true, Ordering::Release);
                self.collect_expired();
                let _ = worker.signal.try_send(());
            }
        }
    }

    impl Drop for OriginalStore {
        fn drop(&mut self) {
            if let Some(worker) = &self.worker {
                worker.closed.store(true, Ordering::Release);
                CLOSED_PENDING.store(true, Ordering::Release);
                let _ = worker.signal.try_send(());
            }
        }
    }

    /// Copy exact retained bytes into the native session folder before writing its document.
    /// Same-path saves trust the verified identity plus unchanged metadata and avoid a hash read.
    pub fn persist_originals(session: &mut Session, dir: &Path) -> Result<usize> {
        let ids: BTreeSet<SourceId> = session.sources.iter().map(|source| source.id).collect();
        let retained: Vec<_> = session.originals.iter().filter(|(id, _)| ids.contains(*id)).map(|(id, metadata)| (*id, metadata.clone())).collect();
        if retained.len() > MAX_ORIGINAL_ASSETS {
            return Err(EngineError::Io("original asset count limit reached".into()));
        }
        let mut total = 0u64;
        for (id, metadata) in &retained {
            validate(metadata)?;
            total = total
                .checked_add(metadata.bytes)
                .filter(|total| *total <= MAX_ORIGINAL_BYTES)
                .ok_or_else(|| EngineError::Io("original asset byte limit reached".into()))?;
            let lease = session.pool.original_asset(*id).ok_or_else(|| EngineError::Io(format!("original asset unavailable for source {}", id.0)))?;
            if lease.sha256 != metadata.sha256 || lease.bytes != metadata.bytes {
                return Err(EngineError::Io("original asset lease differs from receipt".into()));
            }
            verify_cache(lease)?;
        }
        if retained.is_empty() {
            session.originals.retain(|id, _| ids.contains(id));
            session.pool.retain_originals(&ids);
            return Ok(0);
        }
        let folder = checked_folder(dir, true)?;
        let mut replacements = vec![];
        let mut copied = 0usize;
        for (id, metadata) in &retained {
            let path = folder.join(format!("{}.asset", metadata.sha256));
            crate::file_access::check_write(&path).map_err(EngineError::Io)?;
            let lease = session.pool.original_asset(*id).ok_or_else(|| error(&path, "original asset unavailable"))?;
            if lease.path == path {
                verify_cache(lease)?;
                continue;
            }
            let replacement = match storage::inspect(&path) {
                Ok(_) => verified_published(&path, metadata)?,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let mut source = storage::open(&lease.path).map_err(|err| error(&lease.path, err))?;
                    let target = storage::exclusive(&path).map_err(|err| error(&path, err))?;
                    let mut identity = Handle::from_file(target).map_err(|err| error(&path, err))?;
                    let result = digest_reader(&mut source, metadata.bytes, Some(identity.as_file_mut())).and_then(|digest| {
                        if digest != metadata.sha256 {
                            return Err(error(&lease.path, "original asset checksum differs from its receipt"));
                        }
                        verify_cache(lease)?;
                        identity.as_file().sync_all().map_err(|err| error(&path, err))
                    });
                    if let Err(err) = result {
                        if same_identity(&path, &identity, storage::Kind::File) {
                            let _ = storage::remove_file(&path);
                        }
                        return Err(err);
                    }
                    copied = copied.saturating_add(1);
                    register(&published_worker()?, path.clone(), metadata.sha256.clone(), metadata.bytes, identity, false)?
                }
                Err(err) => return Err(error(&path, err)),
            };
            replacements.push((*id, replacement));
        }
        for (id, lease) in replacements {
            session.pool.insert_original(id, lease);
        }
        session.originals.retain(|id, _| ids.contains(id));
        session.pool.retain_originals(&ids);
        Ok(copied)
    }

    /// Keep bad receipts visible as diagnostics. Missing originals do not stop working-PCM open,
    /// but cannot later be silently discarded by Save or Save Copy.
    pub fn load_originals(session: &mut Session, dir: &Path) -> Result<Vec<String>> {
        if session.originals.is_empty() {
            return Ok(vec![]);
        }
        if session.originals.len() > MAX_ORIGINAL_ASSETS {
            return Err(EngineError::Io("original asset count limit reached".into()));
        }
        let folder = checked_folder(dir, false);
        let mut diagnostics = vec![];
        let mut total = 0u64;
        for (id, metadata) in &session.originals {
            session.pool.remove_original(*id);
            let loaded = validate(metadata).and_then(|()| {
                total = total
                    .checked_add(metadata.bytes)
                    .filter(|total| *total <= MAX_ORIGINAL_BYTES)
                    .ok_or_else(|| EngineError::Io("original asset byte limit reached".into()))?;
                let folder = folder.as_ref().map_err(Clone::clone)?;
                verified_published(&folder.join(format!("{}.asset", metadata.sha256)), metadata)
            });
            match loaded {
                Ok(lease) => session.pool.insert_original(*id, lease),
                Err(err) => diagnostics.push(format!("original asset for source {}: {err}", id.0)),
            }
        }
        Ok(diagnostics)
    }

    /// A command may keep its previous document alive until after replacing a staging scope.
    /// Its control-side epilogue can reclaim now-expired closed scopes before process exit.
    pub(crate) fn collect_closed_scopes() {
        if !CLOSED_PENDING.swap(false, Ordering::AcqRel) {
            return;
        }
        let states: Vec<_> = receipts().lock().unwrap_or_else(PoisonError::into_inner).values().filter_map(Weak::upgrade).collect();
        let mut seen = BTreeSet::new();
        for state in states {
            if !seen.insert(Arc::as_ptr(&state) as usize) {
                continue;
            }
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.closed.load(Ordering::Acquire) {
                collect(&mut state);
            }
        }
    }

    #[cfg(not(test))]
    fn record_hash_read() {}

    #[cfg(test)]
    fn record_hash_read() {
        HASH_READS.with(|count| count.set(count.get().saturating_add(1)));
    }

    #[cfg(test)]
    thread_local! { static HASH_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

    #[cfg(test)]
    mod tests {
        use super::*;
        use soundcraft_model::Source;
        use std::time::{Duration, Instant};

        struct TestDirectory(PathBuf);
        impl TestDirectory {
            fn new() -> Self {
                let id = next_receipt().unwrap();
                let path = std::env::temp_dir().join(format!("soundcraft-original-tests-{}-{id}", std::process::id()));
                std::fs::create_dir(&path).unwrap();
                Self(path)
            }
        }
        impl Drop for TestDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        fn retained_session(store: &mut OriginalStore) -> Session {
            let mut session = Session::default();
            let id = SourceId(session.alloc());
            session.sources.push(Source {
                id,
                name: "fixture".into(),
                path: "Audio Files/fixture.wav".into(),
                channels: 1,
                frames: 1,
                sample_rate: 48_000,
                format: soundcraft_audio_io::FileFormat::Wav,
                time_reference: 0,
                unsaved: true,
            });
            let (metadata, lease) = store.capture(b"exact encoded fixture").unwrap();
            session.originals.insert(id, metadata);
            session.pool.insert_original(id, lease);
            session
        }

        fn wait_removed(path: &Path) {
            let started = Instant::now();
            while storage::inspect(path).is_ok() && started.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(storage::inspect(path).is_err(), "collector left {}", path.display());
        }

        #[cfg(unix)]
        #[test]
        fn temporary_original_creation_is_owner_only_under_umask_022() {
            const CHILD: &str = "SOUNDCRAFT_ENCODED_ORIGINAL_PERMISSIONS_CHILD";
            if std::env::var_os(CHILD).as_deref() != Some(std::ffi::OsStr::new("1")) {
                let output = std::process::Command::new("sh")
                    .args(["-c", "umask 022; exec \"$@\"", "soundcraft-original-permissions"])
                    .arg(std::env::current_exe().unwrap())
                    .args(["--exact", "original_audio::native::tests::temporary_original_creation_is_owner_only_under_umask_022", "--nocapture"])
                    .env(CHILD, "1")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "child failed: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains("private-original-permissions: verified"),
                    "child did not execute its permission and cleanup checks"
                );
                return;
            }

            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            let fixture = TestDirectory::new();
            let control = fixture.0.join("ordinary-create");
            drop(storage::exclusive(&control).unwrap());
            assert_eq!(mode(&control), 0o644, "child must exercise the permissive creation control");

            let private = fixture.0.join("private");
            storage::make_private_dir(&private).unwrap();
            assert_eq!(mode(&private), 0o700);
            let staged = private.join("staged.asset");
            let mut file = storage::exclusive_private(&staged).unwrap();
            assert_eq!(file.metadata().unwrap().len(), 0);
            assert_eq!(mode(&staged), 0o600, "permissions must be private before the first byte");
            file.write_all(b"first").unwrap();
            file.flush().unwrap();
            assert_eq!(mode(&staged), 0o600, "permissions must remain private during a write");
            assert_eq!(std::fs::read(&staged).unwrap(), b"first");
            assert_eq!(storage::exclusive_private(&staged).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
            assert_eq!(std::fs::read(&staged).unwrap(), b"first", "exclusive creation must not truncate an existing file");
            file.write_all(b"-last").unwrap();
            file.sync_all().unwrap();
            drop(file);
            assert_eq!(std::fs::read(&staged).unwrap(), b"first-last");

            let mut store = OriginalStore::new();
            let bytes = b"exact private encoded original";
            let (metadata, lease) = store.capture(bytes).unwrap();
            let path = lease.path.clone();
            let directory = path.parent().unwrap().to_path_buf();
            assert_eq!(mode(&directory), 0o700);
            assert_eq!(mode(&path) & 0o077, 0, "actual capture must use the private creation path");
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(metadata.bytes, bytes.len() as u64);
            assert_eq!(metadata.sha256, format!("{:x}", Sha256::digest(bytes)));
            let snapshot = Arc::clone(&lease);
            drop(lease);
            store.close();
            drop(store);
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "a live snapshot must retain its private original");
            drop(snapshot);
            wait_removed(&path);
            wait_removed(&directory);
            println!("private-original-permissions: verified");
        }

        #[test]
        fn repeated_same_path_save_checks_receipt_without_rehashing_the_published_bytes() {
            let folder = TestDirectory::new();
            let mut store = OriginalStore::new();
            let mut session = retained_session(&mut store);
            assert_eq!(persist_originals(&mut session, &folder.0).unwrap(), 1);
            HASH_READS.with(|count| count.set(0));
            assert_eq!(persist_originals(&mut session, &folder.0).unwrap(), 0);
            assert_eq!(persist_originals(&mut session, &folder.0).unwrap(), 0);
            HASH_READS.with(|count| assert_eq!(count.get(), 0, "unchanged same-path saves reread all originals"));
            let id = session.sources[0].id;
            assert_eq!(std::fs::read(&session.pool.original_asset(id).unwrap().path).unwrap(), b"exact encoded fixture");
        }

        #[test]
        fn published_path_replacement_is_refused_and_never_overwritten() {
            let folder = TestDirectory::new();
            let mut store = OriginalStore::new();
            let mut session = retained_session(&mut store);
            persist_originals(&mut session, &folder.0).unwrap();
            let id = session.sources[0].id;
            let lease = Arc::clone(session.pool.original_asset(id).unwrap());
            let old = folder.0.join("original-replaced");
            std::fs::rename(&lease.path, old).unwrap();
            std::fs::write(&lease.path, b"exact encoded fixture").unwrap();
            let before = session.clone();
            let err = persist_originals(&mut session, &folder.0).unwrap_err();
            assert!(err.to_string().contains("original asset"), "{err}");
            assert_eq!(session, before);
            assert_eq!(std::fs::read(&lease.path).unwrap(), b"exact encoded fixture");
        }

        #[test]
        fn last_snapshot_release_after_store_close_collects_only_owned_private_files() {
            let mut store = OriginalStore::new();
            let (_metadata, lease) = store.capture(b"snapshot keeps these bytes").unwrap();
            let path = lease.path.clone();
            let folder = path.parent().unwrap().to_path_buf();
            let snapshot = Arc::clone(&lease);
            drop(lease);
            drop(store);
            assert_eq!(std::fs::read(&path).unwrap(), b"snapshot keeps these bytes");
            drop(snapshot);
            wait_removed(&path);
            wait_removed(&folder);
        }

        #[test]
        fn collector_keeps_a_replacement_at_an_expired_temporary_asset_path() {
            let mut store = OriginalStore::new();
            let (_metadata, lease) = store.capture(b"owned private bytes").unwrap();
            let path = lease.path.clone();
            let folder = path.parent().unwrap().to_path_buf();
            let moved = folder.join("moved-user-file");
            std::fs::rename(&path, &moved).unwrap();
            std::fs::write(&path, b"replacement belongs to someone else").unwrap();
            drop(lease);
            store.collect_expired();
            drop(store);
            assert_eq!(std::fs::read(&path).unwrap(), b"replacement belongs to someone else");
            assert_eq!(std::fs::read(&moved).unwrap(), b"owned private bytes");
            // Test-owned replacements are removed by the fixture, not by production GC.
            std::fs::remove_file(path).unwrap();
            std::fs::remove_file(moved).unwrap();
            let _ = std::fs::remove_dir(folder);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::collect_closed_scopes;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{OriginalStore, load_originals, persist_originals};

#[cfg(target_arch = "wasm32")]
#[derive(Debug, Default)]
pub struct OriginalStore;
#[cfg(target_arch = "wasm32")]
impl OriginalStore {
    pub fn new() -> Self {
        Self
    }
    pub fn capture(
        &mut self,
        _bytes: &[u8],
    ) -> crate::Result<(soundcraft_model::OriginalProvenance, std::sync::Arc<soundcraft_model::OriginalAssetLease>)> {
        Err(crate::EngineError::Unsupported("encoded original retention requires a native session folder".into()))
    }
    pub fn collect_expired(&self) {}
    pub fn close(&self) {
        self.collect_expired();
    }
}
#[cfg(target_arch = "wasm32")]
pub fn persist_originals(session: &mut soundcraft_model::Session, _dir: &std::path::Path) -> crate::Result<usize> {
    if session.originals.is_empty() {
        Ok(0)
    } else {
        Err(crate::EngineError::Unsupported("encoded original retention requires a native session folder".into()))
    }
}
#[cfg(target_arch = "wasm32")]
pub fn load_originals(session: &mut soundcraft_model::Session, _dir: &std::path::Path) -> crate::Result<Vec<String>> {
    if session.originals.is_empty() { Ok(vec![]) } else { Ok(vec!["encoded originals require a native session folder".into()]) }
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn collect_closed_scopes() {}
