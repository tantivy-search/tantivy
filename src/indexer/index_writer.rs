use super::operation::{AddOperation, UserOperation};
use super::segment_updater::SegmentUpdater;
use super::PreparedCommit;
use bit_set::BitSet;
use core::Index;
use core::Segment;
use core::SegmentComponent;
use core::SegmentId;
use core::SegmentMeta;
use core::SegmentReader;
use crossbeam::channel;
use directory::DirectoryLock;
use docset::DocSet;
use error::TantivyError;
use fastfield::write_delete_bitset;
use futures::{Canceled, Future};
use indexer::delete_queue::{DeleteCursor, DeleteQueue};
use indexer::doc_opstamp_mapping::DocToOpstampMapping;
use indexer::operation::DeleteOperation;
use indexer::stamper::Stamper;
use indexer::MergePolicy;
use indexer::SegmentEntry;
use indexer::SegmentWriter;
use postings::compute_table_size;
use schema::Document;
use schema::IndexRecordOption;
use schema::Term;
use std::mem;
use std::ops::Range;
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use Result;

// Size of the margin for the heap. A segment is closed when the remaining memory
// in the heap goes below MARGIN_IN_BYTES.
pub const MARGIN_IN_BYTES: usize = 1_000_000;

// We impose the memory per thread to be at least 3 MB.
pub const HEAP_SIZE_MIN: usize = ((MARGIN_IN_BYTES as u32) * 3u32) as usize;
pub const HEAP_SIZE_MAX: usize = u32::max_value() as usize - MARGIN_IN_BYTES;

// Add document will block if the number of docs waiting in the queue to be indexed
// reaches `PIPELINE_MAX_SIZE_IN_DOCS`
const PIPELINE_MAX_SIZE_IN_DOCS: usize = 10_000;

type DocumentSender = channel::Sender<Vec<AddOperation>>;
type DocumentReceiver = channel::Receiver<Vec<AddOperation>>;

/// Split the thread memory budget into
/// - the heap size
/// - the hash table "table" itself.
///
/// Returns (the heap size in bytes, the hash table size in number of bits)
fn initial_table_size(per_thread_memory_budget: usize) -> usize {
    assert!(per_thread_memory_budget > 1_000);
    let table_size_limit: usize = per_thread_memory_budget / 3;
    if let Some(limit) = (1..)
        .take_while(|num_bits: &usize| compute_table_size(*num_bits) < table_size_limit)
        .last()
    {
        limit.min(19) // we cap it at 2^19 = 512K.
    } else {
        unreachable!(
            "Per thread memory is too small: {}",
            per_thread_memory_budget
        );
    }
}

/// `IndexWriter` is the user entry-point to add document to an index.
///
/// It manages a small number of indexing thread, as well as a shared
/// indexing queue.
/// Each indexing thread builds its own independent `Segment`, via
/// a `SegmentWriter` object.
pub struct IndexWriter {
    // the lock is just used to bind the
    // lifetime of the lock with that of the IndexWriter.
    _directory_lock: Option<DirectoryLock>,

    index: Index,

    heap_size_in_bytes_per_thread: usize,

    workers_join_handle: Vec<JoinHandle<Result<()>>>,

    document_receiver: DocumentReceiver,
    document_sender: DocumentSender,

    segment_updater: SegmentUpdater,

    worker_id: usize,

    num_threads: usize,

    generation: usize,

    delete_queue: DeleteQueue,

    stamper: Stamper,
    committed_opstamp: u64,
}

/// Open a new index writer. Attempts to acquire a lockfile.
///
/// The lockfile should be deleted on drop, but it is possible
/// that due to a panic or other error, a stale lockfile will be
/// left in the index directory. If you are sure that no other
/// `IndexWriter` on the system is accessing the index directory,
/// it is safe to manually delete the lockfile.
///
/// `num_threads` specifies the number of indexing workers that
/// should work at the same time.
/// # Errors
/// If the lockfile already exists, returns `Error::FileAlreadyExists`.
/// # Panics
/// If the heap size per thread is too small, panics.
pub fn open_index_writer(
    index: &Index,
    num_threads: usize,
    heap_size_in_bytes_per_thread: usize,
    directory_lock: DirectoryLock,
) -> Result<IndexWriter> {
    if heap_size_in_bytes_per_thread < HEAP_SIZE_MIN {
        let err_msg = format!(
            "The heap size per thread needs to be at least {}.",
            HEAP_SIZE_MIN
        );
        return Err(TantivyError::InvalidArgument(err_msg));
    }
    if heap_size_in_bytes_per_thread >= HEAP_SIZE_MAX {
        let err_msg = format!("The heap size per thread cannot exceed {}", HEAP_SIZE_MAX);
        return Err(TantivyError::InvalidArgument(err_msg));
    }
    let (document_sender, document_receiver): (DocumentSender, DocumentReceiver) =
        channel::bounded(PIPELINE_MAX_SIZE_IN_DOCS);

    let delete_queue = DeleteQueue::new();

    let current_opstamp = index.load_metas()?.opstamp;

    let stamper = Stamper::new(current_opstamp);

    let segment_updater =
        SegmentUpdater::create(index.clone(), stamper.clone(), &delete_queue.cursor())?;

    let mut index_writer = IndexWriter {
        _directory_lock: Some(directory_lock),

        heap_size_in_bytes_per_thread,
        index: index.clone(),

        document_receiver,
        document_sender,

        segment_updater,

        workers_join_handle: vec![],
        num_threads,

        delete_queue,

        committed_opstamp: current_opstamp,
        stamper,

        generation: 0,

        worker_id: 0,
    };
    index_writer.start_workers()?;
    Ok(index_writer)
}

pub fn compute_deleted_bitset(
    delete_bitset: &mut BitSet,
    segment_reader: &SegmentReader,
    delete_cursor: &mut DeleteCursor,
    doc_opstamps: &DocToOpstampMapping,
    target_opstamp: u64,
) -> Result<bool> {
    let mut might_have_changed = false;

    #[cfg_attr(feature = "cargo-clippy", allow(clippy::while_let_loop))]
    loop {
        if let Some(delete_op) = delete_cursor.get() {
            if delete_op.opstamp > target_opstamp {
                break;
            } else {
                // A delete operation should only affect
                // document that were inserted after it.
                //
                // Limit doc helps identify the first document
                // that may be affected by the delete operation.
                let limit_doc = doc_opstamps.compute_doc_limit(delete_op.opstamp);
                let inverted_index = segment_reader.inverted_index(delete_op.term.field());
                if let Some(mut docset) =
                    inverted_index.read_postings(&delete_op.term, IndexRecordOption::Basic)
                {
                    while docset.advance() {
                        let deleted_doc = docset.doc();
                        if deleted_doc < limit_doc {
                            delete_bitset.insert(deleted_doc as usize);
                            might_have_changed = true;
                        }
                    }
                }
            }
        } else {
            break;
        }
        delete_cursor.advance();
    }
    Ok(might_have_changed)
}

/// Advance delete for the given segment up
/// to the target opstamp.
pub fn advance_deletes(
    mut segment: Segment,
    segment_entry: &mut SegmentEntry,
    target_opstamp: u64,
) -> Result<()> {
    {
        if segment_entry.meta().delete_opstamp() == Some(target_opstamp) {
            // We are already up-to-date here.
            return Ok(());
        }

        let segment_reader = SegmentReader::open(&segment)?;
        let max_doc = segment_reader.max_doc();

        let mut delete_bitset: BitSet = match segment_entry.delete_bitset() {
            Some(previous_delete_bitset) => (*previous_delete_bitset).clone(),
            None => BitSet::with_capacity(max_doc as usize),
        };

        let delete_cursor = segment_entry.delete_cursor();

        compute_deleted_bitset(
            &mut delete_bitset,
            &segment_reader,
            delete_cursor,
            &DocToOpstampMapping::None,
            target_opstamp,
        )?;

        // TODO optimize
        for doc in 0u32..max_doc {
            if segment_reader.is_deleted(doc) {
                delete_bitset.insert(doc as usize);
            }
        }

        let num_deleted_docs = delete_bitset.len();
        if num_deleted_docs > 0 {
            segment = segment.with_delete_meta(num_deleted_docs as u32, target_opstamp);
            let mut delete_file = segment.open_write(SegmentComponent::DELETE)?;
            write_delete_bitset(&delete_bitset, &mut delete_file)?;
        }
    }
    segment_entry.set_meta(segment.meta().clone());
    Ok(())
}

fn index_documents(
    memory_budget: usize,
    segment: &Segment,
    generation: usize,
    document_iterator: &mut Iterator<Item = Vec<AddOperation>>,
    segment_updater: &mut SegmentUpdater,
    mut delete_cursor: DeleteCursor,
) -> Result<bool> {
    let schema = segment.schema();
    let segment_id = segment.id();
    let table_size = initial_table_size(memory_budget);
    let mut segment_writer = SegmentWriter::for_segment(table_size, segment.clone(), &schema)?;
    for documents in document_iterator {
        for doc in documents {
            segment_writer.add_document(doc, &schema)?;
        }
        let mem_usage = segment_writer.mem_usage();
        if mem_usage >= memory_budget - MARGIN_IN_BYTES {
            info!(
                "Buffer limit reached, flushing segment with maxdoc={}.",
                segment_writer.max_doc()
            );
            break;
        }
    }

    if !segment_updater.is_alive() {
        return Ok(false);
    }

    let num_docs = segment_writer.max_doc();

    // this is ensured by the call to peek before starting
    // the worker thread.
    assert!(num_docs > 0);

    let doc_opstamps: Vec<u64> = segment_writer.finalize()?;

    let segment_meta = SegmentMeta::new(segment_id, num_docs);

    let last_docstamp: u64 = *(doc_opstamps.last().unwrap());

    let delete_bitset_opt = if delete_cursor.get().is_some() {
        let doc_to_opstamps = DocToOpstampMapping::from(doc_opstamps);
        let segment_reader = SegmentReader::open(segment)?;
        let mut deleted_bitset = BitSet::with_capacity(num_docs as usize);
        let may_have_deletes = compute_deleted_bitset(
            &mut deleted_bitset,
            &segment_reader,
            &mut delete_cursor,
            &doc_to_opstamps,
            last_docstamp,
        )?;
        if may_have_deletes {
            Some(deleted_bitset)
        } else {
            None
        }
    } else {
        // if there are no delete operation in the queue, no need
        // to even open the segment.
        None
    };
    let segment_entry = SegmentEntry::new(segment_meta, delete_cursor, delete_bitset_opt);
    Ok(segment_updater.add_segment(generation, segment_entry))
}

impl IndexWriter {
    /// The index writer
    pub fn wait_merging_threads(mut self) -> Result<()> {
        // this will stop the indexing thread,
        // dropping the last reference to the segment_updater.
        drop(self.document_sender);

        let former_workers_handles = mem::replace(&mut self.workers_join_handle, vec![]);
        for join_handle in former_workers_handles {
            join_handle
                .join()
                .expect("Indexing Worker thread panicked")
                .map_err(|_| {
                    TantivyError::ErrorInThread("Error in indexing worker thread.".into())
                })?;
        }
        drop(self.workers_join_handle);

        let result = self
            .segment_updater
            .wait_merging_thread()
            .map_err(|_| TantivyError::ErrorInThread("Failed to join merging thread.".into()));

        if let Err(ref e) = result {
            error!("Some merging thread failed {:?}", e);
        }

        result
    }

    #[doc(hidden)]
    pub fn add_segment(&mut self, segment_meta: SegmentMeta) {
        let delete_cursor = self.delete_queue.cursor();
        let segment_entry = SegmentEntry::new(segment_meta, delete_cursor, None);
        self.segment_updater
            .add_segment(self.generation, segment_entry);
    }

    /// Creates a new segment.
    ///
    /// This method is useful only for users trying to do complex
    /// operations, like converting an index format to another.
    ///
    /// It is safe to start writing file associated to the new `Segment`.
    /// These will not be garbage collected as long as an instance object of
    /// `SegmentMeta` object associated to the new `Segment` is "alive".
    pub fn new_segment(&self) -> Segment {
        self.index.new_segment()
    }

    /// Spawns a new worker thread for indexing.
    /// The thread consumes documents from the pipeline.
    ///
    fn add_indexing_worker(&mut self) -> Result<()> {
        let document_receiver_clone = self.document_receiver.clone();
        let mut segment_updater = self.segment_updater.clone();

        let generation = self.generation;

        let mut delete_cursor = self.delete_queue.cursor();

        let mem_budget = self.heap_size_in_bytes_per_thread;
        let index = self.index.clone();
        let join_handle: JoinHandle<Result<()>> = thread::Builder::new()
            .name(format!(
                "thrd-tantivy-index{}-gen{}",
                self.worker_id, generation
            ))
            .spawn(move || {
                loop {
                    let mut document_iterator =
                        document_receiver_clone.clone().into_iter().peekable();

                    // the peeking here is to avoid
                    // creating a new segment's files
                    // if no document are available.
                    //
                    // this is a valid guarantee as the
                    // peeked document now belongs to
                    // our local iterator.
                    if let Some(operations) = document_iterator.peek() {
                        if let Some(first) = operations.first() {
                            delete_cursor.skip_to(first.opstamp);
                        } else {
                            return Ok(());
                        }
                    } else {
                        // No more documents.
                        // Happens when there is a commit, or if the `IndexWriter`
                        // was dropped.
                        return Ok(());
                    }
                    let segment = index.new_segment();
                    index_documents(
                        mem_budget,
                        &segment,
                        generation,
                        &mut document_iterator,
                        &mut segment_updater,
                        delete_cursor.clone(),
                    )?;
                }
            })?;
        self.worker_id += 1;
        self.workers_join_handle.push(join_handle);
        Ok(())
    }

    /// Accessor to the merge policy.
    pub fn get_merge_policy(&self) -> Arc<Box<MergePolicy>> {
        self.segment_updater.get_merge_policy()
    }

    /// Set the merge policy.
    pub fn set_merge_policy(&self, merge_policy: Box<MergePolicy>) {
        self.segment_updater.set_merge_policy(merge_policy);
    }

    fn start_workers(&mut self) -> Result<()> {
        for _ in 0..self.num_threads {
            self.add_indexing_worker()?;
        }
        Ok(())
    }

    /// Detects and removes the files that
    /// are not used by the index anymore.
    pub fn garbage_collect_files(&mut self) -> Result<()> {
        self.segment_updater.garbage_collect_files()
    }

    /// Merges a given list of segments
    ///
    /// `segment_ids` is required to be non-empty.
    pub fn merge(
        &mut self,
        segment_ids: &[SegmentId],
    ) -> Result<impl Future<Item = SegmentMeta, Error = Canceled>> {
        self.segment_updater.start_merge(segment_ids)
    }

    /// Closes the current document channel send.
    /// and replace all the channels by new ones.
    ///
    /// The current workers will keep on indexing
    /// the pending document and stop
    /// when no documents are remaining.
    ///
    /// Returns the former segment_ready channel.
    fn recreate_document_channel(&mut self) -> DocumentReceiver {
        let (document_sender, document_receiver): (DocumentSender, DocumentReceiver) =
            channel::bounded(PIPELINE_MAX_SIZE_IN_DOCS);
        mem::replace(&mut self.document_sender, document_sender);
        mem::replace(&mut self.document_receiver, document_receiver)
    }

    /// Rollback to the last commit
    ///
    /// This cancels all of the update that
    /// happened before after the last commit.
    /// After calling rollback, the index is in the same
    /// state as it was after the last commit.
    ///
    /// The opstamp at the last commit is returned.
    pub fn rollback(&mut self) -> Result<()> {
        info!("Rolling back to opstamp {}", self.committed_opstamp);

        // marks the segment updater as killed. From now on, all
        // segment updates will be ignored.
        self.segment_updater.kill();

        let document_receiver = self.document_receiver.clone();

        // take the directory lock to create a new index_writer.
        let directory_lock = self
            ._directory_lock
            .take()
            .expect("The IndexWriter does not have any lock. This is a bug, please report.");

        let new_index_writer: IndexWriter = open_index_writer(
            &self.index,
            self.num_threads,
            self.heap_size_in_bytes_per_thread,
            directory_lock,
        )?;

        // the current `self` is dropped right away because of this call.
        //
        // This will drop the document queue, and the thread
        // should terminate.
        mem::replace(self, new_index_writer);

        // Drains the document receiver pipeline :
        // Workers don't need to index the pending documents.
        //
        // This will reach an end as the only document_sender
        // was dropped with the index_writer.
        for _ in document_receiver.clone() {}

        Ok(())
    }

    /// Prepares a commit.
    ///
    /// Calling `prepare_commit()` will cut the indexing
    /// queue. All pending documents will be sent to the
    /// indexing workers. They will then terminate, regardless
    /// of the size of their current segment and flush their
    /// work on disk.
    ///
    /// Once a commit is "prepared", you can either
    /// call
    /// * `.commit()`: to accept this commit
    /// * `.abort()`: to cancel this commit.
    ///
    /// In the current implementation, `PreparedCommit` borrows
    /// the `IndexWriter` mutably so we are guaranteed that no new
    /// document can be added as long as it is committed or is
    /// dropped.
    ///
    /// It is also possible to add a payload to the `commit`
    /// using this API.
    /// See [`PreparedCommit::set_payload()`](PreparedCommit.html)
    pub fn prepare_commit(&mut self) -> Result<PreparedCommit> {
        // Here, because we join all of the worker threads,
        // all of the segment update for this commit have been
        // sent.
        //
        // No document belonging to the next generation have been
        // pushed too, because add_document can only happen
        // on this thread.

        // This will move uncommitted segments to the state of
        // committed segments.
        info!("Preparing commit");

        // this will drop the current document channel
        // and recreate a new one channels.
        self.recreate_document_channel();

        let former_workers_join_handle = mem::replace(&mut self.workers_join_handle, Vec::new());

        for worker_handle in former_workers_join_handle {
            let indexing_worker_result = worker_handle
                .join()
                .map_err(|e| TantivyError::ErrorInThread(format!("{:?}", e)))?;
            indexing_worker_result?;
            // add a new worker for the next generation.
            self.add_indexing_worker()?;
        }

        let commit_opstamp = self.stamper.stamp();
        let prepared_commit = PreparedCommit::new(self, commit_opstamp);
        info!("Prepared commit {}", commit_opstamp);
        Ok(prepared_commit)
    }

    /// Commits all of the pending changes
    ///
    /// A call to commit blocks.
    /// After it returns, all of the document that
    /// were added since the last commit are published
    /// and persisted.
    ///
    /// In case of a crash or an hardware failure (as
    /// long as the hard disk is spared), it will be possible
    /// to resume indexing from this point.
    ///
    /// Commit returns the `opstamp` of the last document
    /// that made it in the commit.
    ///
    pub fn commit(&mut self) -> Result<u64> {
        self.prepare_commit()?.commit()
    }

    pub(crate) fn segment_updater(&self) -> &SegmentUpdater {
        &self.segment_updater
    }

    /// Delete all documents containing a given term.
    ///
    /// Delete operation only affects documents that
    /// were added in previous commits, and documents
    /// that were added previously in the same commit.
    ///
    /// Like adds, the deletion itself will be visible
    /// only after calling `commit()`.
    pub fn delete_term(&mut self, term: Term) -> u64 {
        let opstamp = self.stamper.stamp();
        let delete_operation = DeleteOperation { opstamp, term };
        self.delete_queue.push(delete_operation);
        opstamp
    }

    /// Returns the opstamp of the last successful commit.
    ///
    /// This is, for instance, the opstamp the index will
    /// rollback to if there is a failure like a power surge.
    ///
    /// This is also the opstamp of the commit that is currently
    /// available for searchers.
    pub fn commit_opstamp(&self) -> u64 {
        self.committed_opstamp
    }

    /// Adds a document.
    ///
    /// If the indexing pipeline is full, this call may block.
    ///
    /// The opstamp is an increasing `u64` that can
    /// be used by the client to align commits with its own
    /// document queue.
    ///
    /// Currently it represents the number of documents that
    /// have been added since the creation of the index.
    pub fn add_document(&mut self, document: Document) -> u64 {
        let opstamp = self.stamper.stamp();
        let add_operation = AddOperation { opstamp, document };
        let send_result = self.document_sender.send(vec![add_operation]);
        if let Err(e) = send_result {
            panic!("Failed to index document. Sending to indexing channel failed. This probably means all of the indexing threads have panicked. {:?}", e);
        }
        opstamp
    }

    /// Gets a range of stamps from the stamper and "pops" the last stamp
    /// from the range returning a tuple of the last optstamp and the popped
    /// range.
    ///
    /// The total number of stamps generated by this method is `count + 1`;
    /// each operation gets a stamp from the `stamps` iterator and `last_opstamp`
    /// is for the batch itself.
    fn get_batch_opstamps(&mut self, count: u64) -> (u64, Range<u64>) {
        let Range { start, end } = self.stamper.stamps(count + 1u64);
        let last_opstamp = end - 1;
        let stamps = Range {
            start: start,
            end: last_opstamp,
        };
        (last_opstamp, stamps)
    }

    /// Runs a group of document operations ensuring that the operations are
    /// assigned contigous u64 opstamps and that add operations of the same
    /// group are flushed into the same segment.
    /// 
    /// If the indexing pipeline is full, this call may block.
    /// 
    /// Each operation of the given `user_operations` will receive an in-order,
    /// contiguous u64 opstamp. The entire batch itself is also given an
    /// opstamp that is 1 greater than the last given operation. This 
    /// `batch_opstamp` is the return value of `run`. An empty group of
    /// `user_operations`, an empty `Vec<UserOperation>`, still receives
    /// a valid opstamp even though no changes were _actually_ made to the index.
    /// 
    /// Like adds and deletes (see `IndexWriter.add_document` and
    /// `IndexWriter.delete_term`), the changes made by calling `run` will be
    /// visible to readers only after calling `commit()`.
    pub fn run(&mut self, user_operations: Vec<UserOperation>) -> u64 {
        let count = user_operations.len() as u64;
        if count == 0 {
            return self.stamper.stamp();
        }
        let (batch_opstamp, stamps) = self.get_batch_opstamps(count);

        let mut adds: Vec<AddOperation> = Vec::new();

        for (user_op, opstamp) in user_operations.into_iter().zip(stamps) {
            match user_op {
                UserOperation::Delete(term) => {
                    let delete_operation = DeleteOperation {
                        opstamp: opstamp,
                        term: term,
                    };
                    self.delete_queue.push(delete_operation);
                }
                UserOperation::Add(doc) => {
                    let add_operation = AddOperation {
                        opstamp: opstamp,
                        document: doc,
                    };
                    adds.push(add_operation);
                }
            }
        }
        let send_result = self.document_sender.send(adds);
        if let Err(e) = send_result {
            panic!("Failed to index document. Sending to indexing channel failed. This probably means all of the indexing threads have panicked. {:?}", e);
        };

        batch_opstamp
    }
}

#[cfg(test)]
mod tests {

    use super::super::operation::UserOperation;
    use super::initial_table_size;
    use directory::error::LockError;
    use error::*;
    use indexer::NoMergePolicy;
    use schema::{self, Document};
    use Index;
    use Term;

    #[test]
    fn test_operations_group() {
        let mut schema_builder = schema::Schema::builder();
        let text_field = schema_builder.add_text_field("text", schema::TEXT);
        let index = Index::create_in_ram(schema_builder.build());
        let mut index_writer = index.writer_with_num_threads(1, 3_000_000).unwrap();
        let operations = vec![
            UserOperation::Add(doc!(text_field=>"a")),
            UserOperation::Add(doc!(text_field=>"b")),
        ];
        let batch_opstamp1 = index_writer.run(operations);
        assert_eq!(batch_opstamp1, 2u64);
    }

    #[test]
    fn test_empty_operations_group() {
        let schema_builder = schema::Schema::builder();
        let index = Index::create_in_ram(schema_builder.build());
        let mut index_writer = index.writer(3_000_000).unwrap();
        let operations1 = vec![];
        let batch_opstamp1 = index_writer.run(operations1);
        assert_eq!(batch_opstamp1, 0u64);
        let operations2 = vec![];
        let batch_opstamp2 = index_writer.run(operations2);
        assert_eq!(batch_opstamp2, 1u64);
    }

    #[test]
    fn test_lockfile_stops_duplicates() {
        let schema_builder = schema::Schema::builder();
        let index = Index::create_in_ram(schema_builder.build());
        let _index_writer = index.writer(3_000_000).unwrap();
        match index.writer(3_000_000) {
            Err(TantivyError::LockFailure(LockError::LockBusy, _)) => {}
            _ => panic!("Expected a `LockFailure` error"),
        }
    }

    #[test]
    fn test_lockfile_already_exists_error_msg() {
        let schema_builder = schema::Schema::builder();
        let index = Index::create_in_ram(schema_builder.build());
        let _index_writer = index.writer_with_num_threads(1, 3_000_000).unwrap();
        match index.writer_with_num_threads(1, 3_000_000) {
            Err(err) => {
                let err_msg = err.to_string();
                assert!(err_msg.contains("already an `IndexWriter`"));
            }
            _ => panic!("Expected LockfileAlreadyExists error"),
        }
    }

    #[test]
    fn test_set_merge_policy() {
        let schema_builder = schema::Schema::builder();
        let index = Index::create_in_ram(schema_builder.build());
        let index_writer = index.writer(3_000_000).unwrap();
        assert_eq!(
            format!("{:?}", index_writer.get_merge_policy()),
            "LogMergePolicy { min_merge_size: 8, min_layer_size: 10000, \
             level_log_size: 0.75 }"
        );
        let merge_policy = Box::new(NoMergePolicy::default());
        index_writer.set_merge_policy(merge_policy);
        assert_eq!(
            format!("{:?}", index_writer.get_merge_policy()),
            "NoMergePolicy"
        );
    }

    #[test]
    fn test_lockfile_released_on_drop() {
        let schema_builder = schema::Schema::builder();
        let index = Index::create_in_ram(schema_builder.build());
        {
            let _index_writer = index.writer(3_000_000).unwrap();
            // the lock should be released when the
            // index_writer leaves the scope.
        }
        let _index_writer_two = index.writer(3_000_000).unwrap();
    }

    #[test]
    fn test_commit_and_rollback() {
        let mut schema_builder = schema::Schema::builder();
        let text_field = schema_builder.add_text_field("text", schema::TEXT);
        let index = Index::create_in_ram(schema_builder.build());

        let num_docs_containing = |s: &str| {
            let searcher = index.searcher();
            let term = Term::from_field_text(text_field, s);
            searcher.doc_freq(&term)
        };

        {
            // writing the segment
            let mut index_writer = index.writer(3_000_000).unwrap();
            index_writer.add_document(doc!(text_field=>"a"));
            index_writer.rollback().unwrap();

            assert_eq!(index_writer.commit_opstamp(), 0u64);
            assert_eq!(num_docs_containing("a"), 0);
            {
                index_writer.add_document(doc!(text_field=>"b"));
                index_writer.add_document(doc!(text_field=>"c"));
            }
            assert!(index_writer.commit().is_ok());
            index.load_searchers().unwrap();
            assert_eq!(num_docs_containing("a"), 0);
            assert_eq!(num_docs_containing("b"), 1);
            assert_eq!(num_docs_containing("c"), 1);
        }
        index.load_searchers().unwrap();
        index.searcher();
    }

    #[test]
    fn test_with_merges() {
        let mut schema_builder = schema::Schema::builder();
        let text_field = schema_builder.add_text_field("text", schema::TEXT);
        let index = Index::create_in_ram(schema_builder.build());
        let num_docs_containing = |s: &str| {
            let searcher = index.searcher();
            let term_a = Term::from_field_text(text_field, s);
            searcher.doc_freq(&term_a)
        };
        {
            // writing the segment
            let mut index_writer = index.writer(12_000_000).unwrap();
            // create 8 segments with 100 tiny docs
            for _doc in 0..100 {
                let mut doc = Document::default();
                doc.add_text(text_field, "a");
                index_writer.add_document(doc);
            }
            index_writer.commit().expect("commit failed");
            for _doc in 0..100 {
                let mut doc = Document::default();
                doc.add_text(text_field, "a");
                index_writer.add_document(doc);
            }
            // this should create 8 segments and trigger a merge.
            index_writer.commit().expect("commit failed");
            index_writer
                .wait_merging_threads()
                .expect("waiting merging thread failed");
            index.load_searchers().unwrap();

            assert_eq!(num_docs_containing("a"), 200);
            assert!(index.searchable_segments().unwrap().len() < 8);
        }
    }

    #[test]
    fn test_prepare_with_commit_message() {
        let mut schema_builder = schema::Schema::builder();
        let text_field = schema_builder.add_text_field("text", schema::TEXT);
        let index = Index::create_in_ram(schema_builder.build());

        {
            // writing the segment
            let mut index_writer = index.writer(12_000_000).unwrap();
            // create 8 segments with 100 tiny docs
            for _doc in 0..100 {
                index_writer.add_document(doc!(text_field => "a"));
            }
            {
                let mut prepared_commit = index_writer.prepare_commit().expect("commit failed");
                prepared_commit.set_payload("first commit");
                prepared_commit.commit().expect("commit failed");
            }
            {
                let metas = index.load_metas().unwrap();
                assert_eq!(metas.payload.unwrap(), "first commit");
            }
            for _doc in 0..100 {
                index_writer.add_document(doc!(text_field => "a"));
            }
            index_writer.commit().unwrap();
            {
                let metas = index.load_metas().unwrap();
                assert!(metas.payload.is_none());
            }
        }
    }

    #[test]
    fn test_prepare_but_rollback() {
        let mut schema_builder = schema::Schema::builder();
        let text_field = schema_builder.add_text_field("text", schema::TEXT);
        let index = Index::create_in_ram(schema_builder.build());

        {
            // writing the segment
            let mut index_writer = index.writer_with_num_threads(4, 12_000_000).unwrap();
            // create 8 segments with 100 tiny docs
            for _doc in 0..100 {
                index_writer.add_document(doc!(text_field => "a"));
            }
            {
                let mut prepared_commit = index_writer.prepare_commit().expect("commit failed");
                prepared_commit.set_payload("first commit");
                prepared_commit.abort().expect("commit failed");
            }
            {
                let metas = index.load_metas().unwrap();
                assert!(metas.payload.is_none());
            }
            for _doc in 0..100 {
                index_writer.add_document(doc!(text_field => "b"));
            }
            index_writer.commit().unwrap();
        }
        index.load_searchers().unwrap();
        let num_docs_containing = |s: &str| {
            let searcher = index.searcher();
            let term_a = Term::from_field_text(text_field, s);
            searcher.doc_freq(&term_a)
        };
        assert_eq!(num_docs_containing("a"), 0);
        assert_eq!(num_docs_containing("b"), 100);
    }

    #[test]
    fn test_hashmap_size() {
        assert_eq!(initial_table_size(100_000), 12);
        assert_eq!(initial_table_size(1_000_000), 15);
        assert_eq!(initial_table_size(10_000_000), 18);
        assert_eq!(initial_table_size(1_000_000_000), 19);
    }

    #[cfg(not(feature = "no_fail"))]
    #[test]
    fn test_write_commit_fails() {
        use fail;
        let mut schema_builder = schema::Schema::builder();
        let text_field = schema_builder.add_text_field("text", schema::TEXT);
        let index = Index::create_in_ram(schema_builder.build());

        let mut index_writer = index.writer_with_num_threads(1, 3_000_000).unwrap();
        for _ in 0..100 {
            index_writer.add_document(doc!(text_field => "a"));
        }
        index_writer.commit().unwrap();
        fail::cfg("RAMDirectory::atomic_write", "return(error_write_failed)").unwrap();
        for _ in 0..100 {
            index_writer.add_document(doc!(text_field => "b"));
        }
        assert!(index_writer.commit().is_err());
        index.load_searchers().unwrap();
        let num_docs_containing = |s: &str| {
            let searcher = index.searcher();
            let term_a = Term::from_field_text(text_field, s);
            searcher.doc_freq(&term_a)
        };
        assert_eq!(num_docs_containing("a"), 100);
        assert_eq!(num_docs_containing("b"), 0);
        fail::cfg("RAMDirectory::atomic_write", "off").unwrap();
    }
}
