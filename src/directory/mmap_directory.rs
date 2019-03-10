extern crate fs2;
extern crate notify;


use self::notify::{RecursiveMode, DebouncedEvent};
use self::fs2::FileExt;
use std::sync::mpsc::{channel, Receiver};
use atomicwrites;
use self::notify::Watcher;
use common::make_io_err;
use directory::error::LockError;
use directory::error::{DeleteError, IOError, OpenDirectoryError, OpenReadError, OpenWriteError};
use directory::read_only_source::BoxedData;
use directory::Directory;
use directory::DirectoryLock;
use directory::Lock;
use directory::ReadOnlySource;
use directory::WritePtr;
use directory::WatchCallback;
use memmap::Mmap;
use std::collections::HashMap;
use std::convert::From;
use std::fmt;
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::result;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::Weak;
use std::thread;
use tempdir::TempDir;
use std::time::Duration;
use directory::WatchEventRouter;
use directory::WatchHandle;

/// Returns None iff the file exists, can be read, but is empty (and hence
/// cannot be mmapped).
///
fn open_mmap(full_path: &Path) -> result::Result<Option<Mmap>, OpenReadError> {
    let file = File::open(full_path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            OpenReadError::FileDoesNotExist(full_path.to_owned())
        } else {
            OpenReadError::IOError(IOError::with_path(full_path.to_owned(), e))
        }
    })?;

    let meta_data = file
        .metadata()
        .map_err(|e| IOError::with_path(full_path.to_owned(), e))?;
    if meta_data.len() == 0 {
        // if the file size is 0, it will not be possible
        // to mmap the file, so we return None
        // instead.
        return Ok(None);
    }
    unsafe {
        memmap::Mmap::map(&file)
            .map(Some)
            .map_err(|e| From::from(IOError::with_path(full_path.to_owned(), e)))
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct CacheCounters {
    // Number of time the cache prevents to call `mmap`
    pub hit: usize,
    // Number of time tantivy had to call `mmap`
    // as no entry was in the cache.
    pub miss: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheInfo {
    pub counters: CacheCounters,
    pub mmapped: Vec<PathBuf>,
}

struct MmapCache {
    counters: CacheCounters,
    cache: HashMap<PathBuf, Weak<BoxedData>>,
}

impl Default for MmapCache {
    fn default() -> MmapCache {
        MmapCache {
            counters: CacheCounters::default(),
            cache: HashMap::new(),
        }
    }
}

impl MmapCache {
    fn get_info(&mut self) -> CacheInfo {
        let paths: Vec<PathBuf> = self.cache.keys().cloned().collect();
        CacheInfo {
            counters: self.counters.clone(),
            mmapped: paths,
        }
    }

    // Returns None if the file exists but as a len of 0 (and hence is not mmappable).
    fn get_mmap(&mut self, full_path: &Path) -> Result<Option<Arc<BoxedData>>, OpenReadError> {
        let path_in_cache = self.cache.contains_key(full_path);
        if path_in_cache {
            {
                let mmap_weak_opt = self.cache.get(full_path);
                if let Some(mmap_arc) = mmap_weak_opt.and_then(|mmap_weak| mmap_weak.upgrade()) {
                    self.counters.hit += 1;
                    return Ok(Some(mmap_arc));
                }
            }
            self.cache.remove(full_path);
        }
        self.counters.miss += 1;
        if let Some(mmap) = open_mmap(full_path)? {
            let mmap_arc: Arc<BoxedData> = Arc::new(Box::new(mmap));
            self.cache
                .insert(full_path.to_owned(), Arc::downgrade(&mmap_arc));
            Ok(Some(mmap_arc))
        } else {
            Ok(None)
        }
    }
}

/// Directory storing data in files, read via mmap.
///
/// The Mmap object are cached to limit the
/// system calls.
///
/// In the `MmapDirectory`, locks are implemented using the `fs2` crate definition of locks.
///
/// On MacOS & linux, it relies on `flock` (aka `BSD Lock`). These locks solve most of the
/// problems related to POSIX Locks, but may their contract may not be respected on `NFS`
/// depending on the implementation.
///
/// On Windows the semantics are again different.
#[derive(Clone)]
pub struct MmapDirectory {
    inner: Arc<MmapDirectoryInner>
}


struct WatcherWrapper {
    watcher: notify::RecommendedWatcher,
    watcher_router: WatchEventRouter,
}

impl WatcherWrapper {
    fn new(watcher: notify::RecommendedWatcher) -> Self {
        WatcherWrapper {
            watcher,
            watcher_router: Default::default()
        }
    }

    fn trigger(&mut self, path: &Path) {
        if self.watcher_router.broadcast(path) {
            self.watcher.unwatch(path);
        }
    }

    fn watch(&mut self, path: &Path, watch_callback: WatchCallback) -> WatchHandle {
        let (has_changed, handle) = self.watcher_router.suscribe(path, watch_callback);
        if has_changed {
            self.watcher.watch(path, RecursiveMode::NonRecursive).unwrap();
        }
        handle
    }
}

struct MmapDirectoryInner {
    root_path: PathBuf,
    mmap_cache: RwLock<MmapCache>,
    _temp_directory: Option<TempDir>,
    watcher: RwLock<WatcherWrapper>,
}

impl MmapDirectoryInner {

    fn new(root_path: PathBuf, temp_directory: Option<TempDir>) -> (MmapDirectoryInner, Receiver<notify::DebouncedEvent>) {
        let (tx, watcher_recv) = channel();
        let watcher = notify::watcher(tx,Duration::from_secs(1)).unwrap(); // TODO unwrap
        let inner = MmapDirectoryInner {
            root_path,
            mmap_cache: Default::default(),
            _temp_directory: temp_directory,
            watcher: RwLock::new(WatcherWrapper::new(watcher))
        };
        (inner, watcher_recv)
    }

    fn trigger(&self, path: &Path) {
        let mut wlock = self.watcher.write().unwrap();
        wlock.trigger(path);
    }

    fn watch(&self, path: &Path, watch_callback: WatchCallback) -> WatchHandle {
        let mut wlock = self.watcher.write().unwrap();
        wlock.watch(path, watch_callback)
    }
}

impl fmt::Debug for MmapDirectory {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "MmapDirectory({:?})", self.inner.root_path)
    }
}

fn extract_path_from_event(evt: notify::DebouncedEvent) -> Option<PathBuf> {
    match evt {
        notify::DebouncedEvent::NoticeWrite(path) => Some(path),
        notify::DebouncedEvent::Rename(from_path, dest_path) => Some(dest_path),
        _ => None
    }
}

impl MmapDirectory {

    fn new(root_path: PathBuf, temp_directory: Option<TempDir>) -> Result<MmapDirectory, OpenDirectoryError> {
        let (inner, watcher_recv) = MmapDirectoryInner::new(root_path, temp_directory);
        let inner_arc = Arc::new(inner);
        let inner_arc_clone = inner_arc.clone();
        thread::spawn(move || {
            loop {
                match watcher_recv.recv().map(extract_path_from_event) {
                    Ok(Some(path)) => {
                        inner_arc_clone.trigger(&path);
                    },
                    Ok(None) => {
                        // not an event we are interested in.
                    }
                    Err(e) => {
                        // the watch send channel was dropped
                        break;
                    }
                }
            }
        });
        Ok(MmapDirectory { inner: inner_arc })
    }

    /// Creates a new MmapDirectory in a temporary directory.
    ///
    /// This is mostly useful to test the MmapDirectory itself.
    /// For your unit tests, prefer the RAMDirectory.
    pub fn create_from_tempdir() -> Result<MmapDirectory, OpenDirectoryError> {
        let tempdir = TempDir::new("index").map_err(OpenDirectoryError::FailedToCreateTempDir)?;
        let tempdir_path = PathBuf::from(tempdir.path());
        MmapDirectory::new(tempdir_path, Some(tempdir))
    }

    /// Opens a MmapDirectory in a directory.
    ///
    /// Returns an error if the `directory_path` does not
    /// exist or if it is not a directory.
    pub fn open<P: AsRef<Path>>(directory_path: P) -> Result<MmapDirectory, OpenDirectoryError> {
        let directory_path: &Path = directory_path.as_ref();
        if !directory_path.exists() {
            Err(OpenDirectoryError::DoesNotExist(PathBuf::from(
                directory_path,
            )))
        } else if !directory_path.is_dir() {
            Err(OpenDirectoryError::NotADirectory(PathBuf::from(
                directory_path,
            )))
        } else {
            Ok(MmapDirectory::new(PathBuf::from(directory_path), None)?)
        }
    }

    /// Joins a relative_path to the directory `root_path`
    /// to create a proper complete `filepath`.
    fn resolve_path(&self, relative_path: &Path) -> PathBuf {
        self.inner.root_path.join(relative_path)
    }

    /// Sync the root directory.
    /// In certain FS, this is required to persistently create
    /// a file.
    fn sync_directory(&self) -> Result<(), io::Error> {
        let mut open_opts = OpenOptions::new();

        // Linux needs read to be set, otherwise returns EINVAL
        // write must not be set, or it fails with EISDIR
        open_opts.read(true);

        // On Windows, opening a directory requires FILE_FLAG_BACKUP_SEMANTICS
        // and calling sync_all() only works if write access is requested.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use winapi::winbase;

            open_opts
                .write(true)
                .custom_flags(winbase::FILE_FLAG_BACKUP_SEMANTICS);
        }

        let fd = open_opts.open(&self.inner.root_path)?;
        fd.sync_all()?;
        Ok(())
    }

    /// Returns some statistical information
    /// about the Mmap cache.
    ///
    /// The `MmapDirectory` embeds a `MmapDirectory`
    /// to avoid multiplying the `mmap` system calls.
    pub fn get_cache_info(&mut self) -> CacheInfo {
        self.inner
            .mmap_cache
            .write()
            .expect("Mmap cache lock is poisoned.")
            .get_info()
    }
}

/// We rely on fs2 for file locking. On Windows & MacOS this
/// uses BSD locks (`flock`). The lock is actually released when
/// the `File` object is dropped and its associated file descriptor
/// is closed.
struct ReleaseLockFile {
    _file: File,
    path: PathBuf,
}

impl Drop for ReleaseLockFile {
    fn drop(&mut self) {
        debug!("Releasing lock {:?}", self.path);
    }
}

/// This Write wraps a File, but has the specificity of
/// call `sync_all` on flush.
struct SafeFileWriter(File);

impl SafeFileWriter {
    fn new(file: File) -> SafeFileWriter {
        SafeFileWriter(file)
    }
}

impl Write for SafeFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()?;
        self.0.sync_all()
    }
}

impl Seek for SafeFileWriter {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }
}

impl Directory for MmapDirectory {
    fn open_read(&self, path: &Path) -> result::Result<ReadOnlySource, OpenReadError> {
        debug!("Open Read {:?}", path);
        let full_path = self.resolve_path(path);

        let mut mmap_cache = self.inner.mmap_cache.write().map_err(|_| {
            let msg = format!(
                "Failed to acquired write lock \
                 on mmap cache while reading {:?}",
                path
            );
            IOError::with_path(path.to_owned(), make_io_err(msg))
        })?;
        Ok(mmap_cache
            .get_mmap(&full_path)?
            .map(ReadOnlySource::from)
            .unwrap_or_else(ReadOnlySource::empty))
    }

    /// Any entry associated to the path in the mmap will be
    /// removed before the file is deleted.
    fn delete(&self, path: &Path) -> result::Result<(), DeleteError> {
        debug!("Deleting file {:?}", path);
        let full_path = self.resolve_path(path);
        match fs::remove_file(&full_path) {
            Ok(_) => self
                .sync_directory()
                .map_err(|e| IOError::with_path(path.to_owned(), e).into()),
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    Err(DeleteError::FileDoesNotExist(path.to_owned()))
                } else {
                    Err(IOError::with_path(path.to_owned(), e).into())
                }
            }
        }
    }

    fn exists(&self, path: &Path) -> bool {
        let full_path = self.resolve_path(path);
        full_path.exists()
    }

    fn open_write(&mut self, path: &Path) -> Result<WritePtr, OpenWriteError> {
        debug!("Open Write {:?}", path);
        let full_path = self.resolve_path(path);

        let open_res = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(full_path);

        let mut file = open_res.map_err(|err| {
            if err.kind() == io::ErrorKind::AlreadyExists {
                OpenWriteError::FileAlreadyExists(path.to_owned())
            } else {
                IOError::with_path(path.to_owned(), err).into()
            }
        })?;

        // making sure the file is created.
        file.flush()
            .map_err(|e| IOError::with_path(path.to_owned(), e))?;

        // Apparetntly, on some filesystem syncing the parent
        // directory is required.
        self.sync_directory()
            .map_err(|e| IOError::with_path(path.to_owned(), e))?;

        let writer = SafeFileWriter::new(file);
        Ok(BufWriter::new(Box::new(writer)))
    }

    fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
        let full_path = self.resolve_path(path);
        let mut buffer = Vec::new();
        match File::open(&full_path) {
            Ok(mut file) => {
                file.read_to_end(&mut buffer)
                    .map_err(|e| IOError::with_path(path.to_owned(), e))?;
                Ok(buffer)
            }
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    Err(OpenReadError::FileDoesNotExist(path.to_owned()))
                } else {
                    Err(IOError::with_path(path.to_owned(), e).into())
                }
            }
        }
    }

    fn atomic_write(&mut self, path: &Path, data: &[u8]) -> io::Result<()> {
        debug!("Atomic Write {:?}", path);
        let full_path = self.resolve_path(path);
        let meta_file = atomicwrites::AtomicFile::new(full_path, atomicwrites::AllowOverwrite);
        meta_file.write(|f| f.write_all(data))?;
        Ok(())
    }

    fn acquire_lock(&self, lock: &Lock) -> Result<DirectoryLock, LockError> {
        let full_path = self.resolve_path(&lock.filepath);
        // We make sure that the file exists.
        let file: File = OpenOptions::new()
            .write(true)
            .create(true) //< if the file does not exist yet, create it.
            .open(&full_path)
            .map_err(LockError::IOError)?;
        if lock.is_blocking {
            file.lock_exclusive().map_err(LockError::IOError)?;
        } else {
            file.try_lock_exclusive().map_err(|_| LockError::LockBusy)?
        }
        // dropping the file handle will release the lock.
        Ok(DirectoryLock::from(Box::new(ReleaseLockFile {
            path: lock.filepath.clone(),
            _file: file,
        })))
    }

    fn watch(&self, path: &Path, watch_callback: WatchCallback) -> WatchHandle {
        self.inner.watch(path, watch_callback)
    }
}

#[cfg(test)]
mod tests {

    // There are more tests in directory/mod.rs
    // The following tests are specific to the MmapDirectory

    use super::*;

    #[test]
    fn test_open_non_existant_path() {
        assert!(MmapDirectory::open(PathBuf::from("./nowhere")).is_err());
    }

    #[test]
    fn test_open_empty() {
        // empty file is actually an edge case because those
        // cannot be mmapped.
        //
        // In that case the directory returns a SharedVecSlice.
        let mut mmap_directory = MmapDirectory::create_from_tempdir().unwrap();
        let path = PathBuf::from("test");
        {
            let mut w = mmap_directory.open_write(&path).unwrap();
            w.flush().unwrap();
        }
        let readonlymap = mmap_directory.open_read(&path).unwrap();
        assert_eq!(readonlymap.len(), 0);
    }

    #[test]
    fn test_cache() {
        let content = "abc".as_bytes();

        // here we test if the cache releases
        // mmaps correctly.
        let mut mmap_directory = MmapDirectory::create_from_tempdir().unwrap();
        let num_paths = 10;
        let paths: Vec<PathBuf> = (0..num_paths)
            .map(|i| PathBuf::from(&*format!("file_{}", i)))
            .collect();
        {
            for path in &paths {
                let mut w = mmap_directory.open_write(path).unwrap();
                w.write(content).unwrap();
                w.flush().unwrap();
            }
        }

        let mut keep = vec![];
        for (i, path) in paths.iter().enumerate() {
            keep.push(mmap_directory.open_read(path).unwrap());
            assert_eq!(mmap_directory.get_cache_info().mmapped.len(), i + 1);
        }
        assert_eq!(mmap_directory.get_cache_info().counters.hit, 0);
        assert_eq!(mmap_directory.get_cache_info().counters.miss, 10);
        assert_eq!(mmap_directory.get_cache_info().mmapped.len(), 10);
        for path in paths.iter() {
            let _r = mmap_directory.open_read(path).unwrap();
            assert_eq!(mmap_directory.get_cache_info().mmapped.len(), num_paths);
        }
        assert_eq!(mmap_directory.get_cache_info().counters.hit, 10);
        assert_eq!(mmap_directory.get_cache_info().counters.miss, 10);
        assert_eq!(mmap_directory.get_cache_info().mmapped.len(), 10);

        for path in paths.iter() {
            let _r = mmap_directory.open_read(path).unwrap();
            assert_eq!(mmap_directory.get_cache_info().mmapped.len(), num_paths);
        }
        assert_eq!(mmap_directory.get_cache_info().counters.hit, 20);
        assert_eq!(mmap_directory.get_cache_info().counters.miss, 10);
        assert_eq!(mmap_directory.get_cache_info().mmapped.len(), 10);
        drop(keep);
        for path in paths.iter() {
            let _r = mmap_directory.open_read(path).unwrap();
            assert_eq!(mmap_directory.get_cache_info().mmapped.len(), num_paths);
        }
        assert_eq!(mmap_directory.get_cache_info().counters.hit, 20);
        assert_eq!(mmap_directory.get_cache_info().counters.miss, 20);
        assert_eq!(mmap_directory.get_cache_info().mmapped.len(), 10);

        for path in &paths {
            mmap_directory.delete(path).unwrap();
        }
        assert_eq!(mmap_directory.get_cache_info().counters.hit, 20);
        assert_eq!(mmap_directory.get_cache_info().counters.miss, 20);
        assert_eq!(mmap_directory.get_cache_info().mmapped.len(), 10);
        for path in paths.iter() {
            assert!(mmap_directory.open_read(path).is_err());
        }
        assert_eq!(mmap_directory.get_cache_info().counters.hit, 20);
        assert_eq!(mmap_directory.get_cache_info().counters.miss, 30);
        assert_eq!(mmap_directory.get_cache_info().mmapped.len(), 0);
    }

}
