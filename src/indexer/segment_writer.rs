use super::operation::AddOperation;
use crate::core::Segment;
use crate::core::SerializableSegment;
use crate::fastfield::FastFieldsWriter;
use crate::fieldnorm::{FieldNormReaders, FieldNormsWriter};
use crate::indexer::segment_serializer::SegmentSerializer;
use crate::postings::compute_table_size;
use crate::postings::MultiFieldPostingsWriter;
use crate::schema::FieldType;
use crate::schema::Schema;
use crate::schema::Term;
use crate::schema::Value;
use crate::tokenizer::PreTokenizedStream;
use crate::tokenizer::{DynTokenStreamChain, Tokenizer};
use crate::tokenizer::{FacetTokenizer, TextAnalyzerT, Token};
use crate::Opstamp;
use crate::{DocId, SegmentComponent};

/// Computes the initial size of the hash table.
///
/// Returns a number of bit `b`, such that the recommended initial table size is 2^b.
fn initial_table_size(per_thread_memory_budget: usize) -> crate::Result<usize> {
    let table_memory_upper_bound = per_thread_memory_budget / 3;
    if let Some(limit) = (10..)
        .take_while(|&num_bits| compute_table_size(num_bits) < table_memory_upper_bound)
        .last()
    {
        Ok(limit.min(19)) // we cap it at 2^19 = 512K.
    } else {
        Err(crate::TantivyError::InvalidArgument(
            format!("per thread memory budget (={}) is too small. Raise the memory budget or lower the number of threads.", per_thread_memory_budget)))
    }
}

/// A `SegmentWriter` is in charge of creating segment index from a
/// set of documents.
///
/// They creates the postings list in anonymous memory.
/// The segment is layed on disk when the segment gets `finalized`.
pub struct SegmentWriter {
    max_doc: DocId,
    multifield_postings: MultiFieldPostingsWriter,
    segment_serializer: SegmentSerializer,
    fast_field_writers: FastFieldsWriter,
    fieldnorms_writer: FieldNormsWriter,
    doc_opstamps: Vec<Opstamp>,
    // TODO: change type
    tokenizers: Vec<Option<Box<dyn TextAnalyzerT>>>,
    term_buffer: Term,
}

impl SegmentWriter {
    /// Creates a new `SegmentWriter`
    ///
    /// The arguments are defined as follows
    ///
    /// - heap: most of the segment writer data (terms, and postings lists recorders)
    /// is stored in a user-defined heap object. This makes it possible for the user to define
    /// the flushing behavior as a buffer limit
    /// - segment: The segment being written
    /// - schema
    pub fn for_segment(
        memory_budget: usize,
        segment: Segment,
        schema: &Schema,
    ) -> crate::Result<SegmentWriter> {
        let tokenizer_manager = segment.index().tokenizers().clone();
        let table_num_bits = initial_table_size(memory_budget)?;
        let segment_serializer = SegmentSerializer::for_segment(segment)?;
        let multifield_postings = MultiFieldPostingsWriter::new(schema, table_num_bits);
        let tokenizers = schema
            .fields()
            .map(|(_, field_entry)| match field_entry.field_type() {
                FieldType::Str(text_options) => {
                    text_options
                        .get_indexing_options()
                        .and_then(|text_index_option| {
                            let tokenizer_name = &text_index_option.tokenizer();
                            tokenizer_manager.get(tokenizer_name)
                        })
                }
                _ => None,
            })
            .collect();
        Ok(SegmentWriter {
            max_doc: 0,
            multifield_postings,
            fieldnorms_writer: FieldNormsWriter::for_schema(schema),
            segment_serializer,
            fast_field_writers: FastFieldsWriter::from_schema(schema),
            doc_opstamps: Vec::with_capacity(1_000),
            tokenizers,
            term_buffer: Term::new(),
        })
    }

    /// Lay on disk the current content of the `SegmentWriter`
    ///
    /// Finalize consumes the `SegmentWriter`, so that it cannot
    /// be used afterwards.
    pub fn finalize(mut self) -> crate::Result<Vec<u64>> {
        self.fieldnorms_writer.fill_up_to_max_doc(self.max_doc);
        write(
            &self.multifield_postings,
            &self.fast_field_writers,
            &self.fieldnorms_writer,
            self.segment_serializer,
        )?;
        Ok(self.doc_opstamps)
    }

    pub fn mem_usage(&self) -> usize {
        self.multifield_postings.mem_usage()
    }

    /// Indexes a new document
    ///
    /// As a user, you should rather use `IndexWriter`'s add_document.
    pub fn add_document(
        &mut self,
        add_operation: AddOperation,
        schema: &Schema,
    ) -> crate::Result<()> {
        let doc_id = self.max_doc;
        let mut doc = add_operation.document;
        self.doc_opstamps.push(add_operation.opstamp);

        self.fast_field_writers.add_document(&doc);

        for (field, field_values) in doc.get_sorted_field_values() {
            let field_entry = schema.get_field_entry(field);
            let make_schema_error = || {
                crate::TantivyError::SchemaError(format!(
                    "Expected a {:?} for field {:?}",
                    field_entry.field_type().value_type(),
                    field_entry.name()
                ))
            };
            if !field_entry.is_indexed() {
                continue;
            }
            let (term_buffer, multifield_postings) =
                (&mut self.term_buffer, &mut self.multifield_postings);
            match field_entry.field_type() {
                FieldType::HierarchicalFacet => {
                    term_buffer.set_field(field);
                    let facets =
                        field_values
                            .iter()
                            .flat_map(|field_value| match field_value.value() {
                                Value::Facet(ref facet) => Some(facet.encoded_str()),
                                _ => {
                                    panic!("Expected hierarchical facet");
                                }
                            });
                    for facet_str in facets {
                        let mut unordered_term_id_opt = None;
                        FacetTokenizer
                            .token_stream(facet_str)
                            .map(|token| {
                                term_buffer.set_text(&token.text);
                                let unordered_term_id =
                                    multifield_postings.subscribe(doc_id, &term_buffer);
                                unordered_term_id_opt = Some(unordered_term_id);
                            })
                            .count();
                        if let Some(unordered_term_id) = unordered_term_id_opt {
                            self.fast_field_writers
                                .get_multivalue_writer(field)
                                .expect("writer for facet missing")
                                .add_val(unordered_term_id);
                        }
                    }
                }
                FieldType::Str(_) => {
                    let mut streams_with_offsets = vec![];
                    let mut total_offset = 0;

                    for field_value in field_values {
                        match field_value.value() {
                            Value::PreTokStr(tok_str) => {
                                streams_with_offsets.push((
                                    Box::new(PreTokenizedStream::from(tok_str.clone()))
                                        as Box<dyn Iterator<Item = Token>>,
                                    total_offset,
                                ));
                                if let Some(last_token) = tok_str.tokens.last() {
                                    total_offset += last_token.offset_to;
                                }
                            }
                            Value::Str(text) => {
                                if let Some(ref mut tokenizer) =
                                    self.tokenizers[field.field_id() as usize]
                                {
                                    streams_with_offsets
                                        .push((tokenizer.token_stream(text), total_offset));
                                    total_offset += text.len();
                                }
                            }
                            _ => (),
                        }
                    }

                    let num_tokens = if streams_with_offsets.is_empty() {
                        0
                    } else {
                        let mut token_stream = DynTokenStreamChain::from_vec(streams_with_offsets);
                        multifield_postings.index_text(
                            doc_id,
                            field,
                            &mut token_stream,
                            term_buffer,
                        )
                    };

                    self.fieldnorms_writer.record(doc_id, field, num_tokens);
                }
                FieldType::U64(int_option) if int_option.is_indexed() => {
                    for field_value in field_values {
                        term_buffer.set_field(field_value.field());
                        let u64_val = field_value
                            .value()
                            .u64_value()
                            .ok_or_else(make_schema_error)?;
                        term_buffer.set_u64(u64_val);
                        multifield_postings.subscribe(doc_id, &term_buffer);
                    }
                }
                FieldType::Date(int_option) if int_option.is_indexed() => {
                    for field_value in field_values {
                        term_buffer.set_field(field_value.field());
                        let date_val = field_value
                            .value()
                            .date_value()
                            .ok_or_else(make_schema_error)?;
                        term_buffer.set_i64(date_val.timestamp());
                        multifield_postings.subscribe(doc_id, &term_buffer);
                    }
                }
                FieldType::I64(int_option) if int_option.is_indexed() => {
                    for field_value in field_values {
                        term_buffer.set_field(field_value.field());
                        let i64_val = field_value
                            .value()
                            .i64_value()
                            .ok_or_else(make_schema_error)?;
                        term_buffer.set_i64(i64_val);
                        multifield_postings.subscribe(doc_id, &term_buffer);
                    }
                }
                FieldType::F64(int_option) if int_option.is_indexed() => {
                    for field_value in field_values {
                        term_buffer.set_field(field_value.field());
                        let f64_val = field_value
                            .value()
                            .f64_value()
                            .ok_or_else(make_schema_error)?;
                        term_buffer.set_f64(f64_val);
                        multifield_postings.subscribe(doc_id, &term_buffer);
                    }
                }
                FieldType::Bytes(option) if option.is_indexed() => {
                    for field_value in field_values {
                        term_buffer.set_field(field_value.field());
                        let bytes = field_value
                            .value()
                            .bytes_value()
                            .ok_or_else(make_schema_error)?;
                        term_buffer.set_bytes(bytes);
                        self.multifield_postings.subscribe(doc_id, &term_buffer);
                    }
                }
                _ => {}
            }
        }
        doc.filter_fields(|field| schema.get_field_entry(field).is_stored());
        doc.prepare_for_store();
        let doc_writer = self.segment_serializer.get_store_writer();
        doc_writer.store(&doc)?;
        self.max_doc += 1;
        Ok(())
    }

    /// Max doc is
    /// - the number of documents in the segment assuming there is no deletes
    /// - the maximum document id (including deleted documents) + 1
    ///
    /// Currently, **tantivy** does not handle deletes anyway,
    /// so `max_doc == num_docs`
    pub fn max_doc(&self) -> u32 {
        self.max_doc
    }

    /// Number of documents in the index.
    /// Deleted documents are not counted.
    ///
    /// Currently, **tantivy** does not handle deletes anyway,
    /// so `max_doc == num_docs`
    #[allow(dead_code)]
    pub fn num_docs(&self) -> u32 {
        self.max_doc
    }
}

// This method is used as a trick to workaround the borrow checker
fn write(
    multifield_postings: &MultiFieldPostingsWriter,
    fast_field_writers: &FastFieldsWriter,
    fieldnorms_writer: &FieldNormsWriter,
    mut serializer: SegmentSerializer,
) -> crate::Result<()> {
    if let Some(fieldnorms_serializer) = serializer.extract_fieldnorms_serializer() {
        fieldnorms_writer.serialize(fieldnorms_serializer)?;
    }
    let fieldnorm_data = serializer
        .segment()
        .open_read(SegmentComponent::FIELDNORMS)?;
    let fieldnorm_readers = FieldNormReaders::open(fieldnorm_data)?;
    let term_ord_map =
        multifield_postings.serialize(serializer.get_postings_serializer(), fieldnorm_readers)?;
    fast_field_writers.serialize(serializer.get_fast_field_serializer(), &term_ord_map)?;
    serializer.close()?;
    Ok(())
}

impl SerializableSegment for SegmentWriter {
    fn write(&self, serializer: SegmentSerializer) -> crate::Result<u32> {
        let max_doc = self.max_doc;
        write(
            &self.multifield_postings,
            &self.fast_field_writers,
            &self.fieldnorms_writer,
            serializer,
        )?;
        Ok(max_doc)
    }
}

#[cfg(test)]
mod tests {
    use super::initial_table_size;

    #[test]
    fn test_hashmap_size() {
        assert_eq!(initial_table_size(100_000).unwrap(), 11);
        assert_eq!(initial_table_size(1_000_000).unwrap(), 14);
        assert_eq!(initial_table_size(10_000_000).unwrap(), 17);
        assert_eq!(initial_table_size(1_000_000_000).unwrap(), 19);
    }
}
