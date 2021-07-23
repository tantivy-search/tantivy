mod reader;
mod writer;

pub use self::reader::BytesFastFieldReader;
pub use self::writer::BytesFastFieldWriter;

#[cfg(test)]
mod tests {
    use crate::schema::{BytesOptions, IndexRecordOption, Schema, Value};
    use crate::{query::TermQuery, schema::FAST, schema::INDEXED, schema::NORMED, schema::STORED};
    use crate::{DocAddress, DocSet, Index, Searcher, Term};
    use std::ops::Deref;

    #[test]
    fn test_bytes() -> crate::Result<()> {
        let mut schema_builder = Schema::builder();
        let bytes_field = schema_builder.add_bytes_field("bytesfield", FAST);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        let mut index_writer = index.writer_for_tests()?;
        index_writer.add_document(doc!(bytes_field=>vec![0u8, 1, 2, 3]));
        index_writer.add_document(doc!(bytes_field=>vec![]));
        index_writer.add_document(doc!(bytes_field=>vec![255u8]));
        index_writer.add_document(doc!(bytes_field=>vec![1u8, 3, 5, 7, 9]));
        index_writer.add_document(doc!(bytes_field=>vec![0u8; 1000]));
        index_writer.commit()?;
        let searcher = index.reader()?.searcher();
        let segment_reader = searcher.segment_reader(0);
        let bytes_reader = segment_reader.fast_fields().bytes(bytes_field).unwrap();
        assert_eq!(bytes_reader.get_bytes(0), &[0u8, 1, 2, 3]);
        assert!(bytes_reader.get_bytes(1).is_empty());
        assert_eq!(bytes_reader.get_bytes(2), &[255u8]);
        assert_eq!(bytes_reader.get_bytes(3), &[1u8, 3, 5, 7, 9]);
        let long = vec![0u8; 1000];
        assert_eq!(bytes_reader.get_bytes(4), long.as_slice());
        Ok(())
    }

    fn create_index_for_test<T: Into<BytesOptions>>(
        byte_options: T,
    ) -> crate::Result<impl Deref<Target = Searcher>> {
        let mut schema_builder = Schema::builder();
        let field = schema_builder.add_bytes_field("string_bytes", byte_options.into());
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        let mut index_writer = index.writer_for_tests()?;
        index_writer.add_document(doc!(
                field => b"tantivy".as_ref(),
                field => b"lucene".as_ref()
        ));
        index_writer.commit()?;
        Ok(index.reader()?.searcher())
    }

    #[test]
    fn test_stored_bytes() -> crate::Result<()> {
        let searcher = create_index_for_test(STORED)?;
        assert_eq!(searcher.num_docs(), 1);
        let retrieved_doc = searcher.doc(DocAddress::new(0u32, 0u32))?;
        let field = searcher.schema().get_field("string_bytes").unwrap();
        let values: Vec<&Value> = retrieved_doc.get_all(field).collect();
        assert_eq!(values.len(), 2);
        let values_bytes: Vec<&[u8]> = values
            .into_iter()
            .flat_map(|value| value.bytes_value())
            .collect();
        assert_eq!(values_bytes, &[&b"tantivy"[..], &b"lucene"[..]]);
        Ok(())
    }

    #[test]
    fn test_non_stored_bytes() -> crate::Result<()> {
        let searcher = create_index_for_test(INDEXED)?;
        assert_eq!(searcher.num_docs(), 1);
        let retrieved_doc = searcher.doc(DocAddress::new(0u32, 0u32))?;
        let field = searcher.schema().get_field("string_bytes").unwrap();
        assert!(retrieved_doc.get_first(field).is_none());
        Ok(())
    }

    #[test]
    fn test_index_bytes() -> crate::Result<()> {
        let searcher = create_index_for_test(INDEXED | NORMED)?;
        assert_eq!(searcher.num_docs(), 1);
        let field = searcher.schema().get_field("string_bytes").unwrap();
        let term = Term::from_field_bytes(field, b"lucene".as_ref());
        let term_query = TermQuery::new(term, IndexRecordOption::Basic);
        let term_weight = term_query.specialized_weight(&searcher, true)?;
        let term_scorer = term_weight.specialized_scorer(searcher.segment_reader(0), 1.0)?;
        assert_eq!(term_scorer.doc(), 0u32);
        Ok(())
    }

    #[test]
    fn test_non_index_bytes() -> crate::Result<()> {
        let searcher = create_index_for_test(STORED)?;
        assert_eq!(searcher.num_docs(), 1);
        let field = searcher.schema().get_field("string_bytes").unwrap();
        let term = Term::from_field_bytes(field, b"lucene".as_ref());
        let term_query = TermQuery::new(term, IndexRecordOption::Basic);
        let term_weight_err = term_query.specialized_weight(&searcher, false);
        assert!(matches!(
            term_weight_err,
            Err(crate::TantivyError::SchemaError(_))
        ));
        Ok(())
    }

    #[test]
    fn test_fast_bytes_multivalue_value() -> crate::Result<()> {
        let searcher = create_index_for_test(FAST)?;
        assert_eq!(searcher.num_docs(), 1);
        let fast_fields = searcher.segment_reader(0u32).fast_fields();
        let field = searcher.schema().get_field("string_bytes").unwrap();
        let fast_field_reader = fast_fields.bytes(field).unwrap();
        assert_eq!(fast_field_reader.get_bytes(0u32), b"tantivy");
        Ok(())
    }
}
