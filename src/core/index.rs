use super::segment::Segment;
use crate::core::Executor;
use crate::core::IndexMeta;
use crate::core::SegmentId;
use crate::core::SegmentMeta;
use crate::core::SegmentMetaInventory;
use crate::core::META_FILEPATH;
use crate::directory::error::OpenReadError;
use crate::directory::ManagedDirectory;
#[cfg(feature = "mmap")]
use crate::directory::MmapDirectory;
use crate::directory::INDEX_WRITER_LOCK;
use crate::directory::{Directory, RAMDirectory};
use crate::error::DataCorruption;
use crate::error::TantivyError;
use crate::indexer::index_writer::HEAP_SIZE_MIN;
use crate::indexer::segment_updater::save_new_metas;
use crate::reader::IndexReader;
use crate::reader::IndexReaderBuilder;
use crate::schema::Field;
use crate::schema::FieldType;
use crate::schema::Schema;
use crate::tokenizer::{TextAnalyzerT, TokenizerManager};
use crate::IndexWriter;
use std::collections::HashSet;
use std::fmt;

#[cfg(feature = "mmap")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

fn load_metas(
    directory: &dyn Directory,
    inventory: &SegmentMetaInventory,
) -> crate::Result<IndexMeta> {
    let meta_data = directory.atomic_read(&META_FILEPATH)?;
    let meta_string = String::from_utf8_lossy(&meta_data);
    IndexMeta::deserialize(&meta_string, &inventory)
        .map_err(|e| {
            DataCorruption::new(
                META_FILEPATH.to_path_buf(),
                format!("Meta file cannot be deserialized. {:?}.", e),
            )
        })
        .map_err(From::from)
}

/// Search Index
#[derive(Clone)]
pub struct Index {
    directory: ManagedDirectory,
    schema: Schema,
    executor: Arc<Executor>,
    tokenizers: TokenizerManager,
    inventory: SegmentMetaInventory,
}

impl Index {
    /// Examines the directory to see if it contains an index.
    ///
    /// Effectively, it only checks for the presence of the `meta.json` file.
    pub fn exists<Dir: Directory>(dir: &Dir) -> Result<bool, OpenReadError> {
        dir.exists(&META_FILEPATH)
    }

    /// Accessor to the search executor.
    ///
    /// This pool is used by default when calling `searcher.search(...)`
    /// to perform search on the individual segments.
    ///
    /// By default the executor is single thread, and simply runs in the calling thread.
    pub fn search_executor(&self) -> &Executor {
        self.executor.as_ref()
    }

    /// Replace the default single thread search executor pool
    /// by a thread pool with a given number of threads.
    pub fn set_multithread_executor(&mut self, num_threads: usize) -> crate::Result<()> {
        self.executor = Arc::new(Executor::multi_thread(num_threads, "thrd-tantivy-search-")?);
        Ok(())
    }

    /// Replace the default single thread search executor pool
    /// by a thread pool with a given number of threads.
    pub fn set_default_multithread_executor(&mut self) -> crate::Result<()> {
        let default_num_threads = num_cpus::get();
        self.set_multithread_executor(default_num_threads)
    }

    /// Creates a new index using the `RAMDirectory`.
    ///
    /// The index will be allocated in anonymous memory.
    /// This should only be used for unit tests.
    pub fn create_in_ram(schema: Schema) -> Index {
        let ram_directory = RAMDirectory::create();
        Index::create(ram_directory, schema).expect("Creating a RAMDirectory should never fail")
    }

    /// Creates a new index in a given filepath.
    /// The index will use the `MMapDirectory`.
    ///
    /// If a previous index was in this directory, then its meta file will be destroyed.
    #[cfg(feature = "mmap")]
    pub fn create_in_dir<P: AsRef<Path>>(
        directory_path: P,
        schema: Schema,
    ) -> crate::Result<Index> {
        let mmap_directory = MmapDirectory::open(directory_path)?;
        if Index::exists(&mmap_directory)? {
            return Err(TantivyError::IndexAlreadyExists);
        }
        Index::create(mmap_directory, schema)
    }

    /// Opens or creates a new index in the provided directory
    pub fn open_or_create<Dir: Directory>(dir: Dir, schema: Schema) -> crate::Result<Index> {
        if !Index::exists(&dir)? {
            return Index::create(dir, schema);
        }
        let index = Index::open(dir)?;
        if index.schema() != schema {
            return Err(TantivyError::SchemaError(
                "An index exists but the schema does not match.".to_string(),
            ));
        }
        Ok(index)
    }

    /// Creates a new index in a temp directory.
    ///
    /// The index will use the `MMapDirectory` in a newly created directory.
    /// The temp directory will be destroyed automatically when the `Index` object
    /// is destroyed.
    ///
    /// The temp directory is only used for testing the `MmapDirectory`.
    /// For other unit tests, prefer the `RAMDirectory`, see: `create_in_ram`.
    #[cfg(feature = "mmap")]
    pub fn create_from_tempdir(schema: Schema) -> crate::Result<Index> {
        let mmap_directory = MmapDirectory::create_from_tempdir()?;
        Index::create(mmap_directory, schema)
    }

    /// Creates a new index given an implementation of the trait `Directory`.
    ///
    /// If a directory previously existed, it will be erased.
    pub fn create<Dir: Directory>(dir: Dir, schema: Schema) -> crate::Result<Index> {
        let directory = ManagedDirectory::wrap(dir)?;
        Index::from_directory(directory, schema)
    }

    /// Create a new index from a directory.
    ///
    /// This will overwrite existing meta.json
    fn from_directory(directory: ManagedDirectory, schema: Schema) -> crate::Result<Index> {
        save_new_metas(schema.clone(), &directory)?;
        let metas = IndexMeta::with_schema(schema);
        Index::create_from_metas(directory, &metas, SegmentMetaInventory::default())
    }

    /// Creates a new index given a directory and an `IndexMeta`.
    fn create_from_metas(
        directory: ManagedDirectory,
        metas: &IndexMeta,
        inventory: SegmentMetaInventory,
    ) -> crate::Result<Index> {
        let schema = metas.schema.clone();
        Ok(Index {
            directory,
            schema,
            tokenizers: TokenizerManager::default(),
            executor: Arc::new(Executor::single_thread()),
            inventory,
        })
    }

    /// Accessor for the tokenizer manager.
    pub fn tokenizers(&self) -> &TokenizerManager {
        &self.tokenizers
    }

    /// Helper to access the tokenizer associated to a specific field.
    pub fn tokenizer_for_field(&self, field: Field) -> crate::Result<Box<dyn TextAnalyzerT>> {
        let field_entry = self.schema.get_field_entry(field);
        let field_type = field_entry.field_type();
        let tokenizer_manager: &TokenizerManager = self.tokenizers();
        let tokenizer_name_opt: Option<Box<dyn TextAnalyzerT>> = match field_type {
            FieldType::Str(text_options) => text_options
                .get_indexing_options()
                .map(|text_indexing_options| text_indexing_options.tokenizer().to_string())
                .and_then(|tokenizer_name| tokenizer_manager.get(&tokenizer_name)),
            _ => None,
        };
        match tokenizer_name_opt {
            Some(tokenizer) => Ok(tokenizer),
            None => Err(TantivyError::SchemaError(format!(
                "{:?} is not a text field.",
                field_entry.name()
            ))),
        }
    }

    /// Create a default `IndexReader` for the given index.
    ///
    /// See [`Index.reader_builder()`](#method.reader_builder).
    pub fn reader(&self) -> crate::Result<IndexReader> {
        self.reader_builder().try_into()
    }

    /// Create a `IndexReader` for the given index.
    ///
    /// Most project should create at most one reader for a given index.
    /// This method is typically called only once per `Index` instance,
    /// over the lifetime of most problem.
    pub fn reader_builder(&self) -> IndexReaderBuilder {
        IndexReaderBuilder::new(self.clone())
    }

    /// Opens a new directory from an index path.
    #[cfg(feature = "mmap")]
    pub fn open_in_dir<P: AsRef<Path>>(directory_path: P) -> crate::Result<Index> {
        let mmap_directory = MmapDirectory::open(directory_path)?;
        Index::open(mmap_directory)
    }

    /// Returns the list of the segment metas tracked by the index.
    ///
    /// Such segments can of course be part of the index,
    /// but also they could be segments being currently built or in the middle of a merge
    /// operation.
    pub fn list_all_segment_metas(&self) -> Vec<SegmentMeta> {
        self.inventory.all()
    }

    /// Creates a new segment_meta (Advanced user only).
    ///
    /// As long as the `SegmentMeta` lives, the files associated with the
    /// `SegmentMeta` are guaranteed to not be garbage collected, regardless of
    /// whether the segment is recorded as part of the index or not.
    pub fn new_segment_meta(&self, segment_id: SegmentId, max_doc: u32) -> SegmentMeta {
        self.inventory.new_segment_meta(segment_id, max_doc)
    }

    /// Open the index using the provided directory
    pub fn open<D: Directory>(directory: D) -> crate::Result<Index> {
        let directory = ManagedDirectory::wrap(directory)?;
        let inventory = SegmentMetaInventory::default();
        let metas = load_metas(&directory, &inventory)?;
        Index::create_from_metas(directory, &metas, inventory)
    }

    /// Reads the index meta file from the directory.
    pub fn load_metas(&self) -> crate::Result<IndexMeta> {
        load_metas(self.directory(), &self.inventory)
    }

    /// Open a new index writer. Attempts to acquire a lockfile.
    ///
    /// The lockfile should be deleted on drop, but it is possible
    /// that due to a panic or other error, a stale lockfile will be
    /// left in the index directory. If you are sure that no other
    /// `IndexWriter` on the system is accessing the index directory,
    /// it is safe to manually delete the lockfile.
    ///
    /// - `num_threads` defines the number of indexing workers that
    /// should work at the same time.
    ///
    /// - `overall_heap_size_in_bytes` sets the amount of memory
    /// allocated for all indexing thread.
    /// Each thread will receive a budget of  `overall_heap_size_in_bytes / num_threads`.
    ///
    /// # Errors
    /// If the lockfile already exists, returns `Error::DirectoryLockBusy` or an `Error::IOError`.
    ///
    /// # Panics
    /// If the heap size per thread is too small, panics.
    pub fn writer_with_num_threads(
        &self,
        num_threads: usize,
        overall_heap_size_in_bytes: usize,
    ) -> crate::Result<IndexWriter> {
        let directory_lock = self
            .directory
            .acquire_lock(&INDEX_WRITER_LOCK)
            .map_err(|err| {
                TantivyError::LockFailure(
                    err,
                    Some(
                        "Failed to acquire index lock. If you are using \
                         a regular directory, this means there is already an \
                         `IndexWriter` working on this `Directory`, in this process \
                         or in a different process."
                            .to_string(),
                    ),
                )
            })?;
        let heap_size_in_bytes_per_thread = overall_heap_size_in_bytes / num_threads;
        IndexWriter::new(
            self,
            num_threads,
            heap_size_in_bytes_per_thread,
            directory_lock,
        )
    }

    /// Helper to create an index writer for tests.
    ///
    /// That index writer only simply has a single thread and a heap of 5 MB.
    /// Using a single thread gives us a deterministic allocation of DocId.
    #[cfg(test)]
    pub fn writer_for_tests(&self) -> crate::Result<IndexWriter> {
        self.writer_with_num_threads(1, 10_000_000)
    }

    /// Creates a multithreaded writer
    ///
    /// Tantivy will automatically define the number of threads to use.
    /// `overall_heap_size_in_bytes` is the total target memory usage that will be split
    /// between a given number of threads.
    ///
    /// # Errors
    /// If the lockfile already exists, returns `Error::FileAlreadyExists`.
    /// # Panics
    /// If the heap size per thread is too small, panics.
    pub fn writer(&self, overall_heap_size_in_bytes: usize) -> crate::Result<IndexWriter> {
        let mut num_threads = num_cpus::get();
        let heap_size_in_bytes_per_thread = overall_heap_size_in_bytes / num_threads;
        if heap_size_in_bytes_per_thread < HEAP_SIZE_MIN {
            num_threads = (overall_heap_size_in_bytes / HEAP_SIZE_MIN).max(1);
        }
        self.writer_with_num_threads(num_threads, overall_heap_size_in_bytes)
    }

    /// Accessor to the index schema
    ///
    /// The schema is actually cloned.
    pub fn schema(&self) -> Schema {
        self.schema.clone()
    }

    /// Returns the list of segments that are searchable
    pub fn searchable_segments(&self) -> crate::Result<Vec<Segment>> {
        Ok(self
            .searchable_segment_metas()?
            .into_iter()
            .map(|segment_meta| self.segment(segment_meta))
            .collect())
    }

    #[doc(hidden)]
    pub fn segment(&self, segment_meta: SegmentMeta) -> Segment {
        Segment::for_index(self.clone(), segment_meta)
    }

    /// Creates a new segment.
    pub fn new_segment(&self) -> Segment {
        let segment_meta = self
            .inventory
            .new_segment_meta(SegmentId::generate_random(), 0);
        self.segment(segment_meta)
    }

    /// Return a reference to the index directory.
    pub fn directory(&self) -> &ManagedDirectory {
        &self.directory
    }

    /// Return a mutable reference to the index directory.
    pub fn directory_mut(&mut self) -> &mut ManagedDirectory {
        &mut self.directory
    }

    /// Reads the meta.json and returns the list of
    /// `SegmentMeta` from the last commit.
    pub fn searchable_segment_metas(&self) -> crate::Result<Vec<SegmentMeta>> {
        Ok(self.load_metas()?.segments)
    }

    /// Returns the list of segment ids that are searchable.
    pub fn searchable_segment_ids(&self) -> crate::Result<Vec<SegmentId>> {
        Ok(self
            .searchable_segment_metas()?
            .iter()
            .map(SegmentMeta::id)
            .collect())
    }

    /// Returns the set of corrupted files
    pub fn validate_checksum(&self) -> crate::Result<HashSet<PathBuf>> {
        self.directory.list_damaged().map_err(Into::into)
    }
}

impl fmt::Debug for Index {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Index({:?})", self.directory)
    }
}

#[cfg(test)]
mod tests {
    use crate::directory::{RAMDirectory, WatchCallback};
    use crate::schema::Field;
    use crate::schema::{Schema, INDEXED, TEXT};
    use crate::IndexReader;
    use crate::ReloadPolicy;
    use crate::{Directory, Index};

    #[test]
    fn test_indexer_for_field() {
        let mut schema_builder = Schema::builder();
        let num_likes_field = schema_builder.add_u64_field("num_likes", INDEXED);
        let body_field = schema_builder.add_text_field("body", TEXT);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        assert!(index.tokenizer_for_field(body_field).is_ok());
        assert_eq!(
            format!("{:?}", index.tokenizer_for_field(num_likes_field).err()),
            "Some(SchemaError(\"\\\"num_likes\\\" is not a text field.\"))"
        );
    }

    #[test]
    fn test_index_exists() {
        let directory = RAMDirectory::create();
        assert!(!Index::exists(&directory).unwrap());
        assert!(Index::create(directory.clone(), throw_away_schema()).is_ok());
        assert!(Index::exists(&directory).unwrap());
    }

    #[test]
    fn open_or_create_should_create() {
        let directory = RAMDirectory::create();
        assert!(!Index::exists(&directory).unwrap());
        assert!(Index::open_or_create(directory.clone(), throw_away_schema()).is_ok());
        assert!(Index::exists(&directory).unwrap());
    }

    #[test]
    fn open_or_create_should_open() {
        let directory = RAMDirectory::create();
        assert!(Index::create(directory.clone(), throw_away_schema()).is_ok());
        assert!(Index::exists(&directory).unwrap());
        assert!(Index::open_or_create(directory, throw_away_schema()).is_ok());
    }

    #[test]
    fn create_should_wipeoff_existing() {
        let directory = RAMDirectory::create();
        assert!(Index::create(directory.clone(), throw_away_schema()).is_ok());
        assert!(Index::exists(&directory).unwrap());
        assert!(Index::create(directory.clone(), Schema::builder().build()).is_ok());
    }

    #[test]
    fn open_or_create_exists_but_schema_does_not_match() {
        let directory = RAMDirectory::create();
        assert!(Index::create(directory.clone(), throw_away_schema()).is_ok());
        assert!(Index::exists(&directory).unwrap());
        assert!(Index::open_or_create(directory.clone(), throw_away_schema()).is_ok());
        let err = Index::open_or_create(directory, Schema::builder().build());
        assert_eq!(
            format!("{:?}", err.unwrap_err()),
            "SchemaError(\"An index exists but the schema does not match.\")"
        );
    }

    fn throw_away_schema() -> Schema {
        let mut schema_builder = Schema::builder();
        let _ = schema_builder.add_u64_field("num_likes", INDEXED);
        schema_builder.build()
    }

    #[test]
    fn test_index_on_commit_reload_policy() {
        let schema = throw_away_schema();
        let field = schema.get_field("num_likes").unwrap();
        let index = Index::create_in_ram(schema);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommit)
            .try_into()
            .unwrap();
        assert_eq!(reader.searcher().num_docs(), 0);
        test_index_on_commit_reload_policy_aux(field, &index, &reader);
    }

    #[cfg(feature = "mmap")]
    mod mmap_specific {

        use super::*;
        use crate::Directory;
        use std::path::PathBuf;
        use tempfile::TempDir;

        #[test]
        fn test_index_on_commit_reload_policy_mmap() {
            let schema = throw_away_schema();
            let field = schema.get_field("num_likes").unwrap();
            let tempdir = TempDir::new().unwrap();
            let tempdir_path = PathBuf::from(tempdir.path());
            let index = Index::create_in_dir(&tempdir_path, schema).unwrap();
            let reader = index
                .reader_builder()
                .reload_policy(ReloadPolicy::OnCommit)
                .try_into()
                .unwrap();
            assert_eq!(reader.searcher().num_docs(), 0);
            test_index_on_commit_reload_policy_aux(field, &index, &reader);
        }

        #[test]
        fn test_index_manual_policy_mmap() -> crate::Result<()> {
            let schema = throw_away_schema();
            let field = schema.get_field("num_likes").unwrap();
            let mut index = Index::create_from_tempdir(schema)?;
            let mut writer = index.writer_for_tests()?;
            writer.commit()?;
            let reader = index
                .reader_builder()
                .reload_policy(ReloadPolicy::Manual)
                .try_into()?;
            assert_eq!(reader.searcher().num_docs(), 0);
            writer.add_document(doc!(field=>1u64));
            let (sender, receiver) = crossbeam::channel::unbounded();
            let _handle = index.directory_mut().watch(WatchCallback::new(move || {
                let _ = sender.send(());
            }));
            writer.commit()?;
            assert!(receiver.recv().is_ok());
            assert_eq!(reader.searcher().num_docs(), 0);
            reader.reload()?;
            assert_eq!(reader.searcher().num_docs(), 1);
            Ok(())
        }

        #[test]
        fn test_index_on_commit_reload_policy_different_directories() {
            let schema = throw_away_schema();
            let field = schema.get_field("num_likes").unwrap();
            let tempdir = TempDir::new().unwrap();
            let tempdir_path = PathBuf::from(tempdir.path());
            let write_index = Index::create_in_dir(&tempdir_path, schema).unwrap();
            let read_index = Index::open_in_dir(&tempdir_path).unwrap();
            let reader = read_index
                .reader_builder()
                .reload_policy(ReloadPolicy::OnCommit)
                .try_into()
                .unwrap();
            assert_eq!(reader.searcher().num_docs(), 0);
            test_index_on_commit_reload_policy_aux(field, &write_index, &reader);
        }
    }
    fn test_index_on_commit_reload_policy_aux(field: Field, index: &Index, reader: &IndexReader) {
        let mut reader_index = reader.index();
        let (sender, receiver) = crossbeam::channel::unbounded();
        let _watch_handle = reader_index
            .directory_mut()
            .watch(WatchCallback::new(move || {
                let _ = sender.send(());
            }));
        let mut writer = index.writer_for_tests().unwrap();
        assert_eq!(reader.searcher().num_docs(), 0);
        writer.add_document(doc!(field=>1u64));
        writer.commit().unwrap();
        // We need a loop here because it is possible for notify to send more than
        // one modify event. It was observed on CI on MacOS.
        loop {
            assert!(receiver.recv().is_ok());
            if reader.searcher().num_docs() == 1 {
                break;
            }
        }
        writer.add_document(doc!(field=>2u64));
        writer.commit().unwrap();
        // ... Same as above
        loop {
            assert!(receiver.recv().is_ok());
            if reader.searcher().num_docs() == 2 {
                break;
            }
        }
    }

    // This test will not pass on windows, because windows
    // prevent deleting files that are MMapped.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn garbage_collect_works_as_intended() {
        let directory = RAMDirectory::create();
        let schema = throw_away_schema();
        let field = schema.get_field("num_likes").unwrap();
        let index = Index::create(directory.clone(), schema).unwrap();

        let mut writer = index.writer_with_num_threads(8, 24_000_000).unwrap();
        for i in 0u64..8_000u64 {
            writer.add_document(doc!(field => i));
        }
        let (sender, receiver) = crossbeam::channel::unbounded();
        let _handle = directory.watch(WatchCallback::new(move || {
            let _ = sender.send(());
        }));
        writer.commit().unwrap();
        let mem_right_after_commit = directory.total_mem_usage();
        assert!(receiver.recv().is_ok());
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .unwrap();

        assert_eq!(reader.searcher().num_docs(), 8_000);
        writer.wait_merging_threads().unwrap();
        let mem_right_after_merge_finished = directory.total_mem_usage();

        reader.reload().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.num_docs(), 8_000);
        assert!(
            mem_right_after_merge_finished < mem_right_after_commit,
            "(mem after merge){} is expected < (mem before merge){}",
            mem_right_after_merge_finished,
            mem_right_after_commit
        );
    }
}
