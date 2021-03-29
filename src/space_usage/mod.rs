/*!
Representations for the space usage of various parts of a Tantivy index.

This can be used programmatically, and will also be exposed in a human readable fashion in
tantivy-cli.

One important caveat for all of this functionality is that none of it currently takes storage-level
details into consideration. For example, if your file system block size is 4096 bytes, we can
under-count actual resultant space usage by up to 4095 bytes per file.
*/

use crate::schema::Field;
use crate::SegmentComponent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Indicates space usage in bytes
pub type ByteCount = usize;

/// Enum containing any of the possible space usage results for segment components.
pub enum ComponentSpaceUsage {
    /// Data is stored per field in a uniform way
    PerField(PerFieldSpaceUsage),
    /// Data is stored in separate pieces in the store
    Store(StoreSpaceUsage),
    /// Some sort of raw byte count
    Basic(ByteCount),
}

/// Represents combined space usage of an entire searcher and its component segments.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearcherSpaceUsage {
    segments: Vec<SegmentSpaceUsage>,
    total: ByteCount,
}

impl SearcherSpaceUsage {
    pub(crate) fn new() -> SearcherSpaceUsage {
        SearcherSpaceUsage {
            segments: Vec::new(),
            total: 0,
        }
    }

    /// Add a segment, to `self`.
    /// Performs no deduplication or other intelligence.
    pub(crate) fn add_segment(&mut self, segment: SegmentSpaceUsage) {
        self.total += segment.total();
        self.segments.push(segment);
    }

    /// Per segment space usage
    pub fn segments(&self) -> &[SegmentSpaceUsage] {
        &self.segments[..]
    }

    /// Returns total byte usage of this searcher, including all large subcomponents.
    /// Does not account for smaller things like `meta.json`.
    pub fn total(&self) -> ByteCount {
        self.total
    }
}

/// Represents combined space usage for all of the large components comprising a segment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegmentSpaceUsage {
    num_docs: u32,

    termdict: PerFieldSpaceUsage,
    postings: PerFieldSpaceUsage,
    positions: PerFieldSpaceUsage,
    positions_idx: PerFieldSpaceUsage,
    fast_fields: PerFieldSpaceUsage,
    fieldnorms: PerFieldSpaceUsage,

    store: StoreSpaceUsage,

    deletes: ByteCount,

    total: ByteCount,
}

impl SegmentSpaceUsage {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        num_docs: u32,
        termdict: PerFieldSpaceUsage,
        postings: PerFieldSpaceUsage,
        positions: PerFieldSpaceUsage,
        positions_idx: PerFieldSpaceUsage,
        fast_fields: PerFieldSpaceUsage,
        fieldnorms: PerFieldSpaceUsage,
        store: StoreSpaceUsage,
        deletes: ByteCount,
    ) -> SegmentSpaceUsage {
        let total = termdict.total()
            + postings.total()
            + positions.total()
            + fast_fields.total()
            + fieldnorms.total()
            + store.total()
            + deletes;
        SegmentSpaceUsage {
            num_docs,
            termdict,
            postings,
            positions,
            positions_idx,
            fast_fields,
            fieldnorms,
            store,
            deletes,
            total,
        }
    }

    /// Space usage for the given component
    ///
    /// Clones the underlying data.
    /// Use the components directly if this is somehow in performance critical code.
    pub fn component(&self, component: SegmentComponent) -> ComponentSpaceUsage {
        use self::ComponentSpaceUsage::*;
        use crate::SegmentComponent::*;
        match component {
            POSTINGS => PerField(self.postings().clone()),
            POSITIONS => PerField(self.positions().clone()),
            POSITIONSSKIP => PerField(self.positions_skip_idx().clone()),
            FASTFIELDS => PerField(self.fast_fields().clone()),
            FIELDNORMS => PerField(self.fieldnorms().clone()),
            TERMS => PerField(self.termdict().clone()),
            STORE => Store(self.store().clone()),
            DELETE => Basic(self.deletes()),
        }
    }

    /// Num docs in segment
    pub fn num_docs(&self) -> u32 {
        self.num_docs
    }

    /// Space usage for term dictionary
    pub fn termdict(&self) -> &PerFieldSpaceUsage {
        &self.termdict
    }

    /// Space usage for postings list
    pub fn postings(&self) -> &PerFieldSpaceUsage {
        &self.postings
    }

    /// Space usage for positions
    pub fn positions(&self) -> &PerFieldSpaceUsage {
        &self.positions
    }

    /// Space usage for positions skip idx
    pub fn positions_skip_idx(&self) -> &PerFieldSpaceUsage {
        &self.positions_idx
    }

    /// Space usage for fast fields
    pub fn fast_fields(&self) -> &PerFieldSpaceUsage {
        &self.fast_fields
    }

    /// Space usage for field norms
    pub fn fieldnorms(&self) -> &PerFieldSpaceUsage {
        &self.fieldnorms
    }

    /// Space usage for stored documents
    pub fn store(&self) -> &StoreSpaceUsage {
        &self.store
    }

    /// Space usage for document deletions
    pub fn deletes(&self) -> ByteCount {
        self.deletes
    }

    /// Total space usage in bytes for this segment.
    pub fn total(&self) -> ByteCount {
        self.total
    }
}

/// Represents space usage for the Store for this segment.
///
/// This is composed of two parts.
/// `data` represents the compressed data itself.
/// `offsets` represents a lookup to find the start of a block
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoreSpaceUsage {
    data: ByteCount,
    offsets: ByteCount,
}

impl StoreSpaceUsage {
    pub(crate) fn new(data: ByteCount, offsets: ByteCount) -> StoreSpaceUsage {
        StoreSpaceUsage { data, offsets }
    }

    /// Space usage for the data part of the store
    pub fn data_usage(&self) -> ByteCount {
        self.data
    }

    /// Space usage for the offsets part of the store (doc ID -> offset)
    pub fn offsets_usage(&self) -> ByteCount {
        self.offsets
    }

    /// Total space usage in bytes for this Store
    pub fn total(&self) -> ByteCount {
        self.data + self.offsets
    }
}

/// Represents space usage for all of the (field, index) pairs that appear in a CompositeFile.
///
/// A field can appear with a single index (typically 0) or with multiple indexes.
/// Multiple indexes are used to handle variable length things, where
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PerFieldSpaceUsage {
    fields: HashMap<Field, FieldUsage>,
    total: ByteCount,
}

impl PerFieldSpaceUsage {
    pub(crate) fn new(fields: HashMap<Field, FieldUsage>) -> PerFieldSpaceUsage {
        let total = fields.values().map(FieldUsage::total).sum();
        PerFieldSpaceUsage { fields, total }
    }

    /// Per field space usage
    pub fn fields(&self) -> impl Iterator<Item = (&Field, &FieldUsage)> {
        self.fields.iter()
    }

    /// Bytes used by the represented file
    pub fn total(&self) -> ByteCount {
        self.total
    }
}

/// Represents space usage of a given field, breaking it down into the (field, index) pairs that
/// comprise it.
///
/// See documentation for PerFieldSpaceUsage for slightly more information.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FieldUsage {
    field: Field,
    num_bytes: ByteCount,
    /// A field can be composed of more than one piece.
    /// These pieces are indexed by arbitrary numbers starting at zero.
    /// `self.num_bytes` includes all of `self.sub_num_bytes`.
    sub_num_bytes: Vec<Option<ByteCount>>,
}

impl FieldUsage {
    pub(crate) fn empty(field: Field) -> FieldUsage {
        FieldUsage {
            field,
            num_bytes: 0,
            sub_num_bytes: Vec::new(),
        }
    }

    pub(crate) fn add_field_idx(&mut self, idx: usize, size: ByteCount) {
        if self.sub_num_bytes.len() < idx + 1 {
            self.sub_num_bytes.resize(idx + 1, None);
        }
        assert!(self.sub_num_bytes[idx].is_none());
        self.sub_num_bytes[idx] = Some(size);
        self.num_bytes += size
    }

    /// Field
    pub fn field(&self) -> Field {
        self.field
    }

    /// Space usage for each index
    pub fn sub_num_bytes(&self) -> &[Option<ByteCount>] {
        &self.sub_num_bytes[..]
    }

    /// Total bytes used for this field in this context
    pub fn total(&self) -> ByteCount {
        self.num_bytes
    }
}

#[cfg(test)]
mod test {
    use crate::core::Index;
    use crate::schema::Field;
    use crate::schema::Schema;
    use crate::schema::{FAST, INDEXED, NORMED, STORED, TEXT};
    use crate::space_usage::ByteCount;
    use crate::space_usage::PerFieldSpaceUsage;
    use crate::Term;

    #[test]
    fn test_empty() {
        let schema = Schema::builder().build();
        let index = Index::create_in_ram(schema.clone());
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let searcher_space_usage = searcher.space_usage().unwrap();
        assert_eq!(0, searcher_space_usage.total());
    }

    fn expect_single_field(
        field_space: &PerFieldSpaceUsage,
        field: &Field,
        min_size: ByteCount,
        max_size: ByteCount,
    ) {
        assert!(field_space.total() >= min_size);
        assert!(field_space.total() <= max_size);
        assert_eq!(
            vec![(field, field_space.total())],
            field_space
                .fields()
                .map(|(x, y)| (x, y.total()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_fast_indexed() {
        let mut schema_builder = Schema::builder();
        let name = schema_builder.add_u64_field("name", FAST | INDEXED | NORMED);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema.clone());

        {
            let mut index_writer = index.writer_for_tests().unwrap();
            index_writer.add_document(doc!(name => 1u64));
            index_writer.add_document(doc!(name => 2u64));
            index_writer.add_document(doc!(name => 10u64));
            index_writer.add_document(doc!(name => 20u64));
            index_writer.commit().unwrap();
        }

        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let searcher_space_usage = searcher.space_usage().unwrap();
        assert!(searcher_space_usage.total() > 0);
        assert_eq!(1, searcher_space_usage.segments().len());

        let segment = &searcher_space_usage.segments()[0];
        assert!(segment.total() > 0);

        assert_eq!(4, segment.num_docs());

        expect_single_field(segment.termdict(), &name, 1, 512);
        expect_single_field(segment.postings(), &name, 1, 512);
        assert_eq!(0, segment.positions().total());
        assert_eq!(0, segment.positions_skip_idx().total());
        expect_single_field(segment.fast_fields(), &name, 1, 512);
        expect_single_field(segment.fieldnorms(), &name, 1, 512);
        // TODO: understand why the following fails
        //        assert_eq!(0, segment.store().total());
        assert_eq!(0, segment.deletes());
    }

    #[test]
    fn test_text() {
        let mut schema_builder = Schema::builder();
        let name = schema_builder.add_text_field("name", TEXT);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema.clone());

        {
            let mut index_writer = index.writer_for_tests().unwrap();
            index_writer.add_document(doc!(name => "hi"));
            index_writer.add_document(doc!(name => "this is a test"));
            index_writer.add_document(
                doc!(name => "some more documents with some word overlap with the other test"),
            );
            index_writer.add_document(doc!(name => "hello hi goodbye"));
            index_writer.commit().unwrap();
        }

        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let searcher_space_usage = searcher.space_usage().unwrap();
        assert!(searcher_space_usage.total() > 0);
        assert_eq!(1, searcher_space_usage.segments().len());

        let segment = &searcher_space_usage.segments()[0];
        assert!(segment.total() > 0);

        assert_eq!(4, segment.num_docs());

        expect_single_field(segment.termdict(), &name, 1, 512);
        expect_single_field(segment.postings(), &name, 1, 512);
        expect_single_field(segment.positions(), &name, 1, 512);
        expect_single_field(segment.positions_skip_idx(), &name, 1, 512);
        assert_eq!(0, segment.fast_fields().total());
        expect_single_field(segment.fieldnorms(), &name, 1, 512);
        // TODO: understand why the following fails
        //        assert_eq!(0, segment.store().total());
        assert_eq!(0, segment.deletes());
    }

    #[test]
    fn test_store() {
        let mut schema_builder = Schema::builder();
        let name = schema_builder.add_text_field("name", STORED);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema.clone());

        {
            let mut index_writer = index.writer_for_tests().unwrap();
            index_writer.add_document(doc!(name => "hi"));
            index_writer.add_document(doc!(name => "this is a test"));
            index_writer.add_document(
                doc!(name => "some more documents with some word overlap with the other test"),
            );
            index_writer.add_document(doc!(name => "hello hi goodbye"));
            index_writer.commit().unwrap();
        }
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let searcher_space_usage = searcher.space_usage().unwrap();
        assert!(searcher_space_usage.total() > 0);
        assert_eq!(1, searcher_space_usage.segments().len());

        let segment = &searcher_space_usage.segments()[0];
        assert!(segment.total() > 0);

        assert_eq!(4, segment.num_docs());

        assert_eq!(0, segment.termdict().total());
        assert_eq!(0, segment.postings().total());
        assert_eq!(0, segment.positions().total());
        assert_eq!(0, segment.positions_skip_idx().total());
        assert_eq!(0, segment.fast_fields().total());
        assert_eq!(0, segment.fieldnorms().total());
        assert!(segment.store().total() > 0);
        assert!(segment.store().total() < 512);
        assert_eq!(0, segment.deletes());
    }

    #[test]
    fn test_deletes() -> crate::Result<()> {
        let mut schema_builder = Schema::builder();
        let name = schema_builder.add_u64_field("name", INDEXED | NORMED);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema.clone());

        {
            let mut index_writer = index.writer_for_tests()?;
            index_writer.add_document(doc!(name => 1u64));
            index_writer.add_document(doc!(name => 2u64));
            index_writer.add_document(doc!(name => 3u64));
            index_writer.add_document(doc!(name => 4u64));
            index_writer.commit()?;
        }

        {
            let mut index_writer2 = index.writer(50_000_000)?;
            index_writer2.delete_term(Term::from_field_u64(name, 2u64));
            index_writer2.delete_term(Term::from_field_u64(name, 3u64));
            // ok, now we should have a deleted doc
            index_writer2.commit()?;
        }

        let reader = index.reader()?;
        let searcher = reader.searcher();
        let searcher_space_usage = searcher.space_usage()?;
        assert!(searcher_space_usage.total() > 0);
        assert_eq!(1, searcher_space_usage.segments().len());

        let segment_space_usage = &searcher_space_usage.segments()[0];
        assert!(segment_space_usage.total() > 0);

        assert_eq!(2, segment_space_usage.num_docs());

        expect_single_field(segment_space_usage.termdict(), &name, 1, 512);
        expect_single_field(segment_space_usage.postings(), &name, 1, 512);
        assert_eq!(0, segment_space_usage.positions().total());
        assert_eq!(0, segment_space_usage.positions_skip_idx().total());
        assert_eq!(0, segment_space_usage.fast_fields().total());
        expect_single_field(segment_space_usage.fieldnorms(), &name, 1, 512);
        assert!(segment_space_usage.deletes() > 0);
        Ok(())
    }
}
