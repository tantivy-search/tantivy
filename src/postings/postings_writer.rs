use super::stacker::{Addr, MemoryArena, TermHashMap};

use crate::fieldnorm::FieldNormReaders;
use crate::postings::recorder::{
    BufferLender, NothingRecorder, Recorder, TFAndPositionRecorder, TermFrequencyRecorder,
};
use crate::postings::UnorderedTermId;
use crate::postings::{FieldSerializer, InvertedIndexSerializer};
use crate::schema::IndexRecordOption;
use crate::schema::{Field, FieldEntry, FieldType, Schema, Term};
use crate::termdict::TermOrdinal;
use crate::tokenizer::{Token, MAX_TOKEN_LEN};
use crate::DocId;
use fnv::FnvHashMap;
use std::collections::HashMap;
use std::io;
use std::marker::PhantomData;
use std::ops::DerefMut;

fn posting_from_field_entry(field_entry: &FieldEntry) -> Box<dyn PostingsWriter> {
    match *field_entry.field_type() {
        FieldType::Str(ref text_options) => text_options
            .get_indexing_options()
            .map(|indexing_options| match indexing_options.index_option() {
                IndexRecordOption::Basic => {
                    SpecializedPostingsWriter::<NothingRecorder>::new_boxed()
                }
                IndexRecordOption::WithFreqs => {
                    SpecializedPostingsWriter::<TermFrequencyRecorder>::new_boxed()
                }
                IndexRecordOption::WithFreqsAndPositions => {
                    SpecializedPostingsWriter::<TFAndPositionRecorder>::new_boxed()
                }
            })
            .unwrap_or_else(|| SpecializedPostingsWriter::<NothingRecorder>::new_boxed()),
        FieldType::U64(_)
        | FieldType::I64(_)
        | FieldType::F64(_)
        | FieldType::Date(_)
        | FieldType::Bytes(_)
        | FieldType::HierarchicalFacet => SpecializedPostingsWriter::<NothingRecorder>::new_boxed(),
    }
}

pub struct MultiFieldPostingsWriter {
    heap: MemoryArena,
    schema: Schema,
    term_index: TermHashMap,
    per_field_postings_writers: Vec<Box<dyn PostingsWriter>>,
}

fn make_field_partition(
    term_offsets: &[(&[u8], Addr, UnorderedTermId)],
) -> Vec<(Field, usize, usize)> {
    let term_offsets_it = term_offsets
        .iter()
        .map(|(key, _, _)| Term::wrap(key).field())
        .enumerate();
    let mut prev_field_opt = None;
    let mut fields = vec![];
    let mut offsets = vec![];
    for (offset, field) in term_offsets_it {
        if Some(field) != prev_field_opt {
            prev_field_opt = Some(field);
            fields.push(field);
            offsets.push(offset);
        }
    }
    offsets.push(term_offsets.len());
    let mut field_offsets = vec![];
    for i in 0..fields.len() {
        field_offsets.push((fields[i], offsets[i], offsets[i + 1]));
    }
    field_offsets
}

impl MultiFieldPostingsWriter {
    /// Create a new `MultiFieldPostingsWriter` given
    /// a schema and a heap.
    pub fn new(schema: &Schema, table_bits: usize) -> MultiFieldPostingsWriter {
        let term_index = TermHashMap::new(table_bits);
        let per_field_postings_writers: Vec<_> = schema
            .fields()
            .map(|(_, field_entry)| posting_from_field_entry(field_entry))
            .collect();
        MultiFieldPostingsWriter {
            heap: MemoryArena::new(),
            schema: schema.clone(),
            term_index,
            per_field_postings_writers,
        }
    }

    pub fn mem_usage(&self) -> usize {
        self.term_index.mem_usage() + self.heap.mem_usage()
    }

    pub fn index_text(
        &mut self,
        doc: DocId,
        field: Field,
        token_stream: &mut dyn Iterator<Item = Token>,
        term_buffer: &mut Term,
    ) -> u32 {
        self.per_field_postings_writers[field.field_id() as usize].index_text(
            &mut self.term_index,
            doc,
            field,
            token_stream,
            &mut self.heap,
            term_buffer,
        )
    }

    pub fn subscribe(&mut self, doc: DocId, term: &Term) -> UnorderedTermId {
        let postings_writer =
            self.per_field_postings_writers[term.field().field_id() as usize].deref_mut();
        postings_writer.subscribe(&mut self.term_index, doc, 0u32, term, &mut self.heap)
    }

    /// Serialize the inverted index.
    /// It pushes all term, one field at a time, towards the
    /// postings serializer.
    pub fn serialize(
        &self,
        serializer: &mut InvertedIndexSerializer,
        fieldnorm_readers: FieldNormReaders,
    ) -> crate::Result<HashMap<Field, FnvHashMap<UnorderedTermId, TermOrdinal>>> {
        let mut term_offsets: Vec<(&[u8], Addr, UnorderedTermId)> =
            self.term_index.iter().collect();
        term_offsets.sort_unstable_by_key(|&(k, _, _)| k);

        let mut unordered_term_mappings: HashMap<Field, FnvHashMap<UnorderedTermId, TermOrdinal>> =
            HashMap::new();

        let field_offsets = make_field_partition(&term_offsets);

        for (field, start, stop) in field_offsets {
            let field_entry = self.schema.get_field_entry(field);

            match *field_entry.field_type() {
                FieldType::Str(_) | FieldType::HierarchicalFacet => {
                    // populating the (unordered term ord) -> (ordered term ord) mapping
                    // for the field.
                    let unordered_term_ids = term_offsets[start..stop]
                        .iter()
                        .map(|&(_, _, bucket)| bucket);
                    let mapping: FnvHashMap<UnorderedTermId, TermOrdinal> = unordered_term_ids
                        .enumerate()
                        .map(|(term_ord, unord_term_id)| {
                            (unord_term_id as UnorderedTermId, term_ord as TermOrdinal)
                        })
                        .collect();
                    unordered_term_mappings.insert(field, mapping);
                }
                FieldType::U64(_) | FieldType::I64(_) | FieldType::F64(_) | FieldType::Date(_) => {}
                FieldType::Bytes(_) => {}
            }

            let postings_writer =
                self.per_field_postings_writers[field.field_id() as usize].as_ref();
            let fieldnorm_reader = fieldnorm_readers.get_field(field)?;
            let mut field_serializer = serializer.new_field(
                field,
                postings_writer.total_num_tokens(),
                fieldnorm_reader,
            )?;
            postings_writer.serialize(
                &term_offsets[start..stop],
                &mut field_serializer,
                &self.term_index.heap,
                &self.heap,
            )?;
            field_serializer.close()?;
        }
        Ok(unordered_term_mappings)
    }
}

/// The `PostingsWriter` is in charge of receiving documenting
/// and building a `Segment` in anonymous memory.
///
/// `PostingsWriter` writes in a `MemoryArena`.
pub trait PostingsWriter {
    /// Record that a document contains a term at a given position.
    ///
    /// * doc  - the document id
    /// * pos  - the term position (expressed in tokens)
    /// * term - the term
    /// * heap - heap used to store the postings informations as well as the terms
    /// in the hashmap.
    fn subscribe(
        &mut self,
        term_index: &mut TermHashMap,
        doc: DocId,
        pos: u32,
        term: &Term,
        heap: &mut MemoryArena,
    ) -> UnorderedTermId;

    /// Serializes the postings on disk.
    /// The actual serialization format is handled by the `PostingsSerializer`.
    fn serialize(
        &self,
        term_addrs: &[(&[u8], Addr, UnorderedTermId)],
        serializer: &mut FieldSerializer<'_>,
        term_heap: &MemoryArena,
        heap: &MemoryArena,
    ) -> io::Result<()>;

    /// Tokenize a text and subscribe all of its token.
    fn index_text(
        &mut self,
        term_index: &mut TermHashMap,
        doc_id: DocId,
        field: Field,
        token_stream: &mut dyn Iterator<Item = Token>,
        heap: &mut MemoryArena,
        term_buffer: &mut Term,
    ) -> u32 {
        term_buffer.set_field(field);
        let mut sink = |token: &Token| {
            // We skip all tokens with a len greater than u16.
            if token.text.len() <= MAX_TOKEN_LEN {
                term_buffer.set_text(token.text.as_str());
                self.subscribe(
                    term_index,
                    doc_id,
                    token.position as u32,
                    &term_buffer,
                    heap,
                );
            } else {
                info!(
                    "A token exceeding MAX_TOKEN_LEN ({}>{}) was dropped. Search for \
                     MAX_TOKEN_LEN in the documentation for more information.",
                    token.text.len(),
                    MAX_TOKEN_LEN
                );
            }
        };
        token_stream.map(|tok| sink(&tok)).count() as u32
    }

    fn total_num_tokens(&self) -> u64;
}

/// The `SpecializedPostingsWriter` is just here to remove dynamic
/// dispatch to the recorder information.
pub(crate) struct SpecializedPostingsWriter<Rec: Recorder + 'static> {
    total_num_tokens: u64,
    _recorder_type: PhantomData<Rec>,
}

impl<Rec: Recorder + 'static> SpecializedPostingsWriter<Rec> {
    /// constructor
    pub fn new() -> SpecializedPostingsWriter<Rec> {
        SpecializedPostingsWriter {
            total_num_tokens: 0u64,
            _recorder_type: PhantomData,
        }
    }

    /// Builds a `SpecializedPostingsWriter` storing its data in a heap.
    pub fn new_boxed() -> Box<dyn PostingsWriter> {
        Box::new(SpecializedPostingsWriter::<Rec>::new())
    }
}

impl<Rec: Recorder + 'static> PostingsWriter for SpecializedPostingsWriter<Rec> {
    fn subscribe(
        &mut self,
        term_index: &mut TermHashMap,
        doc: DocId,
        position: u32,
        term: &Term,
        heap: &mut MemoryArena,
    ) -> UnorderedTermId {
        debug_assert!(term.as_slice().len() >= 4);
        self.total_num_tokens += 1;
        term_index.mutate_or_create(term, |opt_recorder: Option<Rec>| {
            if let Some(mut recorder) = opt_recorder {
                let current_doc = recorder.current_doc();
                if current_doc != doc {
                    recorder.close_doc(heap);
                    recorder.new_doc(doc, heap);
                }
                recorder.record_position(position, heap);
                recorder
            } else {
                let mut recorder = Rec::new();
                recorder.new_doc(doc, heap);
                recorder.record_position(position, heap);
                recorder
            }
        }) as UnorderedTermId
    }

    fn serialize(
        &self,
        term_addrs: &[(&[u8], Addr, UnorderedTermId)],
        serializer: &mut FieldSerializer<'_>,
        termdict_heap: &MemoryArena,
        heap: &MemoryArena,
    ) -> io::Result<()> {
        let mut buffer_lender = BufferLender::default();
        for &(term_bytes, addr, _) in term_addrs {
            let recorder: Rec = termdict_heap.read(addr);
            let term_doc_freq = recorder.term_doc_freq().unwrap_or(0u32);
            serializer.new_term(&term_bytes[4..], term_doc_freq)?;
            recorder.serialize(&mut buffer_lender, serializer, heap)?;
            serializer.close_term()?;
        }
        Ok(())
    }

    fn total_num_tokens(&self) -> u64 {
        self.total_num_tokens
    }
}
