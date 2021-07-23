mod reader;
mod writer;

pub use self::reader::MultiValuedFastFieldReader;
pub use self::writer::MultiValuedFastFieldWriter;

#[cfg(test)]
mod tests {

    use crate::collector::TopDocs;
    use crate::query::QueryParser;
    use crate::schema::Cardinality;
    use crate::schema::Facet;
    use crate::schema::IntOptions;
    use crate::schema::Schema;
    use crate::schema::INDEXED;
    use crate::Index;
    use chrono::Duration;

    #[test]
    fn test_multivalued_u64() {
        let mut schema_builder = Schema::builder();
        let field = schema_builder.add_u64_field(
            "multifield",
            IntOptions::default().set_fast(Cardinality::MultiValues),
        );
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        let mut index_writer = index.writer_for_tests().unwrap();
        index_writer.add_document(doc!(field=>1u64, field=>3u64));
        index_writer.add_document(doc!());
        index_writer.add_document(doc!(field=>4u64));
        index_writer.add_document(doc!(field=>5u64, field=>20u64,field=>1u64));
        assert!(index_writer.commit().is_ok());

        let searcher = index.reader().unwrap().searcher();
        let segment_reader = searcher.segment_reader(0);
        let mut vals = Vec::new();
        let multi_value_reader = segment_reader.fast_fields().u64s(field).unwrap();
        {
            multi_value_reader.get_vals(2, &mut vals);
            assert_eq!(&vals, &[4u64]);
        }
        {
            multi_value_reader.get_vals(0, &mut vals);
            assert_eq!(&vals, &[1u64, 3u64]);
        }
        {
            multi_value_reader.get_vals(1, &mut vals);
            assert!(vals.is_empty());
        }
    }

    #[test]
    fn test_multivalued_date() {
        let mut schema_builder = Schema::builder();
        let date_field = schema_builder.add_date_field(
            "multi_date_field",
            IntOptions::default()
                .set_fast(Cardinality::MultiValues)
                .set_indexed()
                .set_normed()
                .set_stored(),
        );
        let time_i =
            schema_builder.add_i64_field("time_stamp_i", IntOptions::default().set_stored());
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        let mut index_writer = index.writer_for_tests().unwrap();
        let first_time_stamp = chrono::Utc::now();
        index_writer.add_document(
            doc!(date_field=>first_time_stamp, date_field=>first_time_stamp, time_i=>1i64),
        );
        index_writer.add_document(doc!(time_i=>0i64));
        // add one second
        index_writer
            .add_document(doc!(date_field=>first_time_stamp + Duration::seconds(1), time_i=>2i64));
        // add another second
        let two_secs_ahead = first_time_stamp + Duration::seconds(2);
        index_writer.add_document(doc!(date_field=>two_secs_ahead, date_field=>two_secs_ahead,date_field=>two_secs_ahead, time_i=>3i64));
        // add three seconds
        index_writer
            .add_document(doc!(date_field=>first_time_stamp + Duration::seconds(3), time_i=>4i64));
        assert!(index_writer.commit().is_ok());

        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let reader = searcher.segment_reader(0);
        assert_eq!(reader.num_docs(), 5);

        {
            let parser = QueryParser::for_index(&index, vec![date_field]);
            let query = parser
                .parse_query(&format!("\"{}\"", first_time_stamp.to_rfc3339()))
                .expect("could not parse query");
            let results = searcher
                .search(&query, &TopDocs::with_limit(5))
                .expect("could not query index");

            assert_eq!(results.len(), 1);
            for (_score, doc_address) in results {
                let retrieved_doc = searcher.doc(doc_address).expect("cannot fetch doc");
                assert_eq!(
                    retrieved_doc
                        .get_first(date_field)
                        .expect("cannot find value")
                        .date_value()
                        .unwrap()
                        .timestamp(),
                    first_time_stamp.timestamp()
                );
                assert_eq!(
                    retrieved_doc
                        .get_first(time_i)
                        .expect("cannot find value")
                        .i64_value(),
                    Some(1i64)
                );
            }
        }

        {
            let parser = QueryParser::for_index(&index, vec![date_field]);
            let query = parser
                .parse_query(&format!("\"{}\"", two_secs_ahead.to_rfc3339()))
                .expect("could not parse query");
            let results = searcher
                .search(&query, &TopDocs::with_limit(5))
                .expect("could not query index");

            assert_eq!(results.len(), 1);

            for (_score, doc_address) in results {
                let retrieved_doc = searcher.doc(doc_address).expect("cannot fetch doc");
                assert_eq!(
                    retrieved_doc
                        .get_first(date_field)
                        .expect("cannot find value")
                        .date_value()
                        .unwrap()
                        .timestamp(),
                    two_secs_ahead.timestamp()
                );
                assert_eq!(
                    retrieved_doc
                        .get_first(time_i)
                        .expect("cannot find value")
                        .i64_value(),
                    Some(3i64)
                );
            }
        }

        {
            let parser = QueryParser::for_index(&index, vec![date_field]);
            let range_q = format!(
                "[{} TO {}}}",
                (first_time_stamp + Duration::seconds(1)).to_rfc3339(),
                (first_time_stamp + Duration::seconds(3)).to_rfc3339()
            );
            let query = parser.parse_query(&range_q).expect("could not parse query");
            let results = searcher
                .search(&query, &TopDocs::with_limit(5))
                .expect("could not query index");

            assert_eq!(results.len(), 2);
            for (i, doc_pair) in results.iter().enumerate() {
                let retrieved_doc = searcher.doc(doc_pair.1).expect("cannot fetch doc");
                let offset_sec = match i {
                    0 => 1,
                    1 => 2,
                    _ => panic!("should not have more than 2 docs"),
                };
                let time_i_val = match i {
                    0 => 2,
                    1 => 3,
                    _ => panic!("should not have more than 2 docs"),
                };
                assert_eq!(
                    retrieved_doc
                        .get_first(date_field)
                        .expect("cannot find value")
                        .date_value()
                        .expect("value not of Date type")
                        .timestamp(),
                    (first_time_stamp + Duration::seconds(offset_sec)).timestamp()
                );
                assert_eq!(
                    retrieved_doc
                        .get_first(time_i)
                        .expect("cannot find value")
                        .i64_value()
                        .expect("value not of i64 type"),
                    time_i_val
                );
            }
        }
    }

    #[test]
    fn test_multivalued_i64() {
        let mut schema_builder = Schema::builder();
        let field = schema_builder.add_i64_field(
            "multifield",
            IntOptions::default().set_fast(Cardinality::MultiValues),
        );
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        let mut index_writer = index.writer_for_tests().unwrap();
        index_writer.add_document(doc!(field=> 1i64, field => 3i64));
        index_writer.add_document(doc!());
        index_writer.add_document(doc!(field=> -4i64));
        index_writer.add_document(doc!(field=> -5i64, field => -20i64, field=>1i64));
        assert!(index_writer.commit().is_ok());

        let searcher = index.reader().unwrap().searcher();
        let segment_reader = searcher.segment_reader(0);
        let mut vals = Vec::new();
        let multi_value_reader = segment_reader.fast_fields().i64s(field).unwrap();
        multi_value_reader.get_vals(2, &mut vals);
        assert_eq!(&vals, &[-4i64]);
        multi_value_reader.get_vals(0, &mut vals);
        assert_eq!(&vals, &[1i64, 3i64]);
        multi_value_reader.get_vals(1, &mut vals);
        assert!(vals.is_empty());
        multi_value_reader.get_vals(3, &mut vals);
        assert_eq!(&vals, &[-5i64, -20i64, 1i64]);
    }
    #[test]
    #[ignore]
    fn test_many_facets() {
        let mut schema_builder = Schema::builder();
        let field = schema_builder.add_facet_field("facetfield", INDEXED);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        let mut index_writer = index.writer_for_tests().unwrap();
        for i in 0..100_000 {
            index_writer.add_document(doc!(field=> Facet::from(format!("/lang/{}", i).as_str())));
        }
        assert!(index_writer.commit().is_ok());
    }
}
