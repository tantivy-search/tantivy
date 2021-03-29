use crate::schema::IntOptions;
use crate::schema::TextOptions;
use std::ops::BitOr;

#[derive(Clone)]
pub struct StoredFlag;
/// Flag to mark the field as stored.
/// This flag can apply to any kind of field.
///
/// A stored fields of a document can be retrieved given its `DocId`.
/// Stored field are stored together and compressed.
/// Reading the stored fields of a document is relatively slow.
/// (~ 100 microsecs)
///
/// It should not be used during scoring or collection.
pub const STORED: SchemaFlagList<StoredFlag, ()> = SchemaFlagList {
    head: StoredFlag,
    tail: (),
};

#[derive(Clone)]
pub struct IndexedFlag;
/// Flag to mark the field as indexed.
///
/// The `INDEXED` flag can only be used when building `IntOptions` (`u64`, `i64` and `f64` fields)
/// Of course, text fields can also be indexed... But this is expressed by using either the
/// `STRING` (untokenized) or `TEXT` (tokenized with the english tokenizer) flags.
pub const INDEXED: SchemaFlagList<IndexedFlag, ()> = SchemaFlagList {
    head: IndexedFlag,
    tail: (),
};

#[derive(Clone)]
pub struct NormedFlag;
/// Flag to mark the field as indexed.
///
/// The `INDEXED` flag can only be used when building `IntOptions` (`u64`, `i64` and `f64` fields)
/// Of course, text fields can also be indexed... But this is expressed by using either the
/// `STRING` (untokenized) or `TEXT` (tokenized with the english tokenizer) flags.
pub const NORMED: SchemaFlagList<NormedFlag, ()> = SchemaFlagList {
    head: NormedFlag,
    tail: (),
};

#[derive(Clone)]
pub struct FastFlag;
/// Flag to mark the field as a fast field (similar to Lucene's DocValues)
///
/// Fast fields can be random-accessed rapidly. Fields useful for scoring, filtering
/// or collection should be mark as fast fields.
/// The `FAST` flag can only be used when building `IntOptions` (`u64`, `i64` and `f64` fields)
pub const FAST: SchemaFlagList<FastFlag, ()> = SchemaFlagList {
    head: FastFlag,
    tail: (),
};

impl<Head, OldHead, OldTail> BitOr<SchemaFlagList<Head, ()>> for SchemaFlagList<OldHead, OldTail>
where
    Head: Clone,
    OldHead: Clone,
    OldTail: Clone,
{
    type Output = SchemaFlagList<Head, SchemaFlagList<OldHead, OldTail>>;

    fn bitor(self, head: SchemaFlagList<Head, ()>) -> Self::Output {
        SchemaFlagList {
            head: head.head,
            tail: self,
        }
    }
}

impl<T: Clone + Into<IntOptions>> BitOr<IntOptions> for SchemaFlagList<T, ()> {
    type Output = IntOptions;

    fn bitor(self, rhs: IntOptions) -> Self::Output {
        self.head.into() | rhs
    }
}

impl<T: Clone + Into<TextOptions>> BitOr<TextOptions> for SchemaFlagList<T, ()> {
    type Output = TextOptions;

    fn bitor(self, rhs: TextOptions) -> Self::Output {
        self.head.into() | rhs
    }
}

#[derive(Clone)]
pub struct SchemaFlagList<Head: Clone, Tail: Clone> {
    pub head: Head,
    pub tail: Tail,
}
