/*!
Schema definition for tantivy's indices.

# Setting your schema in Tantivy

Tantivy has a very strict schema.
The schema defines information about the fields your index contains, that is, for each field:

* the field name (may only contain letters `[a-zA-Z]`, number `[0-9]`, and `_`)
* the type of the field (currently only  `text` and `u64` are supported)
* how the field should be indexed / stored.

This very last point is critical as it will enable / disable some of the functionality
for your index.

Tantivy's schema is stored within the `meta.json` file at the root of your
directory.



# Building a schema "programmatically"


## Setting a text field

### Example

```
use tantivy::schema::*;
let mut schema_builder = Schema::builder();
let title_options = TextOptions::default()
    .set_stored()
    .set_indexing_options(TextFieldIndexing::default()
        .set_tokenizer("default")
        .set_index_option(IndexRecordOption::WithFreqsAndPositions));
schema_builder.add_text_field("title", title_options);
let schema = schema_builder.build();
```

We can split the problem of generating a search result page into two phases :

* identifying the list of 10 or so documents to be displayed (Conceptually `query -> doc_ids[]`)
* for each of these documents, retrieving the information required to generate
  the search results page. (`doc_ids[] -> Document[]`)

In the first phase, the ability to search for documents by the given field is determined by the
[`IndexRecordOption`](enum.IndexRecordOption.html) of our
[`TextOptions`](struct.TextOptions.html).

The effect of each possible setting is described more in detail
[`TextIndexingOptions`](enum.TextIndexingOptions.html).

On the other hand setting the field as stored or not determines whether the field should be returned
when [`searcher.doc(doc_address)`](../struct.Searcher.html#method.doc) is called.


## Setting a u64, a i64 or a f64 field

### Example

```
use tantivy::schema::*;
let mut schema_builder = Schema::builder();
let num_stars_options = IntOptions::default()
    .set_stored()
    .set_indexed();
schema_builder.add_u64_field("num_stars", num_stars_options);
let schema = schema_builder.build();
```

Just like for Text fields (see above),
setting the field as stored defines whether the field will be
returned when [`searcher.doc(doc_address)`](../struct.Searcher.html#method.doc) is called,
and setting the field as indexed means that we will be able perform queries such as `num_stars:10`.
Note that unlike text fields, u64 can only be indexed in one way for the moment.
This may change when we will start supporting range queries.

The `fast` option on the other hand is specific to u64 fields, and is only relevant
if you are implementing your own queries. This functionality is somewhat similar to Lucene's
`DocValues`.

u64 that are indexed as fast will be stored in a special data structure that will
make it possible to access the u64 value given the doc id rapidly. This is useful if the value of
the field is required during scoring or collection for instance.


### Shortcuts


For convenience, it is possible to define your field indexing options by combining different flags
using the  `|` operator.

For instance, a schema containing the two fields defined in the example above could be rewritten :

```
use tantivy::schema::*;
let mut schema_builder = Schema::builder();
schema_builder.add_u64_field("num_stars", INDEXED | STORED);
schema_builder.add_text_field("title", TEXT | STORED);
let schema = schema_builder.build();
```

*/

mod document;
mod facet;
mod facet_options;
mod schema;
mod term;

mod field_entry;
mod field_type;
mod field_value;

mod bytes_options;
mod field;
mod index_record_option;
mod int_options;
mod named_field_document;
mod text_options;
mod value;

mod flags;

pub use self::named_field_document::NamedFieldDocument;
pub use self::schema::DocParsingError;
pub use self::schema::{Schema, SchemaBuilder};
pub use self::value::Value;

pub use self::facet::Facet;
pub use self::facet::FacetParseError;
pub(crate) use self::facet::FACET_SEP_BYTE;
pub use self::facet_options::FacetOptions;

pub use self::document::Document;
pub use self::field::Field;
pub use self::term::Term;

pub use self::field_entry::FieldEntry;
pub use self::field_type::{FieldType, Type};
pub use self::field_value::FieldValue;

pub use self::index_record_option::IndexRecordOption;
pub use self::text_options::TextFieldIndexing;
pub use self::text_options::TextOptions;
pub use self::text_options::STRING;
pub use self::text_options::TEXT;

pub use self::bytes_options::BytesOptions;
pub use self::flags::{FAST, INDEXED, NORMED, STORED};
pub use self::int_options::Cardinality;
pub use self::int_options::IntOptions;

/// Validator for a potential `field_name`.
/// Returns true if the name can be use for a field name.
///
/// A field name can be any character, must have at least one character
/// and must not start with a `-`.
pub fn is_valid_field_name(field_name: &str) -> bool {
    field_name.len() > 0 && !field_name.starts_with('-')
}

#[cfg(test)]
mod tests {

    use super::is_valid_field_name;

    #[test]
    fn test_is_valid_name() {
        assert!(is_valid_field_name("シャボン玉"));
        assert!(!is_valid_field_name("-fieldname"));
        assert!(!is_valid_field_name(""));
    }
}
