//! The fieldnorm represents the length associated to
//! a given Field of a given document.
//!
//! This metric is important to compute the score of a
//! document : a document having a query word in one its short fields
//! (e.g. title)  is likely to be more relevant than in one of its longer field
//! (e.g. body).
//!
//! It encodes `fieldnorm` on one byte with some precision loss,
//! using the exact same scheme as Lucene. Each value is place on a log-scale
//! that takes values from `0` to `255`.
//!
//! A value on this scale is identified by a `fieldnorm_id`.
//! Apart from compression, this scale also makes it possible to
//! precompute computationally expensive functions of the fieldnorm
//! in a very short array.
//!
//! This trick is used by the Bm25 similarity.
mod code;
mod reader;
mod serializer;
mod writer;

pub use self::reader::{FieldNormReader, FieldNormReaders};
pub use self::serializer::FieldNormsSerializer;
pub use self::writer::FieldNormsWriter;

use self::code::{fieldnorm_to_id, id_to_fieldnorm};

#[cfg(test)]
mod tests {
    use crate::common::CompositeFile;
    use crate::fieldnorm::FieldNormReader;
    use crate::fieldnorm::FieldNormsSerializer;
    use crate::fieldnorm::FieldNormsWriter;
    use crate::{
        directory::{Directory, RAMDirectory, WritePtr},
        schema::{STRING, TEXT},
    };
    use once_cell::sync::Lazy;
    use std::{
        panic::{catch_unwind, AssertUnwindSafe},
        path::Path,
    };

    use crate::schema::{Field, Schema, STORED};

    pub static SCHEMA: Lazy<Schema> = Lazy::new(|| {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("field", STORED);
        schema_builder.add_text_field("txt_field", TEXT);
        schema_builder.add_text_field("str_field", STRING);
        schema_builder.build()
    });

    pub static FIELD: Lazy<Field> = Lazy::new(|| SCHEMA.get_field("field").unwrap());
    pub static TXT_FIELD: Lazy<Field> = Lazy::new(|| SCHEMA.get_field("txt_field").unwrap());
    pub static STR_FIELD: Lazy<Field> = Lazy::new(|| SCHEMA.get_field("str_field").unwrap());

    #[ignore]
    #[test]
    pub fn test_fieldnorm_bug() -> crate::Result<()> {
        let path = Path::new("test");
        let directory: RAMDirectory = RAMDirectory::create();
        {
            let write: WritePtr = directory.open_write(Path::new("test"))?;
            let serializer = FieldNormsSerializer::from_write(write)?;
            let mut fieldnorm_writers = FieldNormsWriter::for_schema(&SCHEMA);
            fieldnorm_writers.fill_up_to_max_doc(1u32);
            fieldnorm_writers.record(0u32, *TXT_FIELD, 5);
            fieldnorm_writers.record(1u32, *TXT_FIELD, 3);
            fieldnorm_writers.serialize(serializer)?;
        }
        let file = directory.open_read(&path)?;
        {
            let fields_composite = CompositeFile::open(&file)?;
            assert!(fields_composite.open_read(*FIELD).is_none());
            assert!(fields_composite.open_read(*TXT_FIELD).is_none());
            assert!(fields_composite.open_read(*STR_FIELD).is_none());
            let data = fields_composite.open_read(*TXT_FIELD).unwrap();
            let fieldnorm_reader = FieldNormReader::open(data)?;
            assert_eq!(fieldnorm_reader.fieldnorm(0u32), 5u32);
            assert_eq!(fieldnorm_reader.fieldnorm(1u32), 3u32);
        }
        Ok(())
    }

    #[test]
    pub fn test_fieldnorm() -> crate::Result<()> {
        let path = Path::new("test");
        let directory: RAMDirectory = RAMDirectory::create();
        {
            let write: WritePtr = directory.open_write(Path::new("test"))?;
            let serializer = FieldNormsSerializer::from_write(write)?;
            let mut fieldnorm_writers = FieldNormsWriter::for_schema(&SCHEMA);
            fieldnorm_writers.fill_up_to_max_doc(1u32);
            fieldnorm_writers.record(1u32, *TXT_FIELD, 3);
            fieldnorm_writers.serialize(serializer)?;
        }
        let file = directory.open_read(&path)?;
        {
            let fields_composite = CompositeFile::open(&file)?;
            assert!(fields_composite.open_read(*FIELD).is_none());
            assert!(fields_composite.open_read(*STR_FIELD).is_none());
            let data = fields_composite.open_read(*TXT_FIELD).unwrap();
            let fieldnorm_reader = FieldNormReader::open(data)?;
            assert_eq!(fieldnorm_reader.fieldnorm(1u32), 3u32);
        }
        Ok(())
    }

    #[test]
    pub fn test_fail_fieldnorm_cannot_registered_twice() {
        let mut fieldnorm_writers = FieldNormsWriter::for_schema(&SCHEMA);
        fieldnorm_writers.fill_up_to_max_doc(1u32);
        fieldnorm_writers.record(1u32, *TXT_FIELD, 5);
        let result = catch_unwind(AssertUnwindSafe(|| {
            fieldnorm_writers.record(1u32, *TXT_FIELD, 3)
        }));
        assert!(result.is_err());
    }
}
