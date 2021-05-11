#[cfg(test)]
mod tests {
    use crate::IndexSortByField;
    use crate::Order;
    use crate::{
        collector::TopDocs,
        schema::{Cardinality, TextFieldIndexing},
    };
    use crate::{core::Index, fastfield::MultiValuedFastFieldReader};
    use crate::{
        query::QueryParser,
        schema::{IntOptions, TextOptions},
    };
    use crate::{
        schema::{self, BytesOptions},
        DocAddress,
    };
    use crate::{IndexSettings, Term};
    use futures::executor::block_on;

    fn create_test_index(index_settings: Option<IndexSettings>) -> Index {
        let mut schema_builder = schema::Schema::builder();
        let int_options = IntOptions::default()
            .set_fast(Cardinality::SingleValue)
            .set_indexed();
        let int_field = schema_builder.add_u64_field("intval", int_options);

        let bytes_options = BytesOptions::default().set_fast().set_indexed();
        let bytes_field = schema_builder.add_bytes_field("bytes", bytes_options);

        let multi_numbers = schema_builder.add_u64_field(
            "multi_numbers",
            IntOptions::default().set_fast(Cardinality::MultiValues),
        );
        let text_field_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_index_option(schema::IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();
        let text_field = schema_builder.add_text_field("text_field", text_field_options);
        let schema = schema_builder.build();

        let mut index_builder = Index::builder().schema(schema);
        if let Some(settings) = index_settings {
            index_builder = index_builder.settings(settings);
        }
        let index = index_builder.create_in_ram().unwrap();

        {
            let mut index_writer = index.writer_for_tests().unwrap();

            index_writer.add_document(doc!(int_field=>1_u64));
            index_writer.add_document(
                doc!(int_field=>3_u64, multi_numbers => 3_u64, multi_numbers => 4_u64, bytes_field => vec![1, 2, 3], text_field => "some text"),
            );
            index_writer.add_document(doc!(int_field=>1_u64, text_field=> "deleteme"));
            index_writer.add_document(
                doc!(int_field=>2_u64, multi_numbers => 2_u64, multi_numbers => 3_u64),
            );

            assert!(index_writer.commit().is_ok());
            index_writer.add_document(doc!(int_field=>20_u64, multi_numbers => 20_u64));
            index_writer.add_document(doc!(int_field=>1_u64, text_field=> "deleteme"));
            assert!(index_writer.commit().is_ok());
            index_writer.add_document(
                doc!(int_field=>10_u64, multi_numbers => 10_u64, multi_numbers => 11_u64, text_field=> "blubber"),
            );
            index_writer.add_document(doc!(int_field=>5_u64, text_field=> "deleteme"));
            index_writer.add_document(
                doc!(int_field=>1_000u64, multi_numbers => 1001_u64, multi_numbers => 1002_u64, bytes_field => vec![5, 5],text_field => "the biggest num")
            );

            index_writer.delete_term(Term::from_field_text(text_field, "deleteme"));
            assert!(index_writer.commit().is_ok());
        }

        // Merging the segments
        {
            let segment_ids = index
                .searchable_segment_ids()
                .expect("Searchable segments failed.");
            let mut index_writer = index.writer_for_tests().unwrap();
            assert!(block_on(index_writer.merge(&segment_ids)).is_ok());
            assert!(index_writer.wait_merging_threads().is_ok());
        }
        index
    }

    #[test]
    fn test_merge_sorted_index_desc() {
        let index = create_test_index(Some(IndexSettings {
            sort_by_field: Some(IndexSortByField {
                field: "intval".to_string(),
                order: Order::Desc,
            }),
        }));

        let int_field = index.schema().get_field("intval").unwrap();
        let reader = index.reader().unwrap();

        let searcher = reader.searcher();
        assert_eq!(searcher.segment_readers().len(), 1);
        let segment_reader = searcher.segment_readers().last().unwrap();

        let fast_fields = segment_reader.fast_fields();
        let fast_field = fast_fields.u64(int_field).unwrap();
        assert_eq!(fast_field.get(5u32), 1u64);
        assert_eq!(fast_field.get(4u32), 2u64);
        assert_eq!(fast_field.get(3u32), 3u64);
        assert_eq!(fast_field.get(2u32), 10u64);
        assert_eq!(fast_field.get(1u32), 20u64);
        assert_eq!(fast_field.get(0u32), 1_000u64);

        // test new field norm mapping
        {
            let my_text_field = index.schema().get_field("text_field").unwrap();
            let fieldnorm_reader = segment_reader.get_fieldnorms_reader(my_text_field).unwrap();
            assert_eq!(fieldnorm_reader.fieldnorm(0), 3); // the biggest num
            assert_eq!(fieldnorm_reader.fieldnorm(1), 0);
            assert_eq!(fieldnorm_reader.fieldnorm(2), 1); // blubber
            assert_eq!(fieldnorm_reader.fieldnorm(3), 2); // some text
            assert_eq!(fieldnorm_reader.fieldnorm(5), 0);
        }

        let my_text_field = index.schema().get_field("text_field").unwrap();
        let searcher = index.reader().unwrap().searcher();
        {
            let my_text_field = index.schema().get_field("text_field").unwrap();

            let do_search = |term: &str| {
                let query = QueryParser::for_index(&index, vec![my_text_field])
                    .parse_query(term)
                    .unwrap();
                let top_docs: Vec<(f32, DocAddress)> =
                    searcher.search(&query, &TopDocs::with_limit(3)).unwrap();

                top_docs.iter().map(|el| el.1.doc_id).collect::<Vec<_>>()
            };

            assert_eq!(do_search("some"), vec![3]);
            assert_eq!(do_search("blubber"), vec![2]);
            assert_eq!(do_search("biggest"), vec![0]);
        }

        // access doc store
        {
            let doc = searcher.doc(DocAddress::new(0, 2)).unwrap();
            assert_eq!(
                doc.get_first(my_text_field).unwrap().text(),
                Some("blubber")
            );
        }
    }

    #[test]
    fn test_merge_sorted_index_asc() {
        let index = create_test_index(Some(IndexSettings {
            sort_by_field: Some(IndexSortByField {
                field: "intval".to_string(),
                order: Order::Asc,
            }),
        }));

        let int_field = index.schema().get_field("intval").unwrap();
        let multi_numbers = index.schema().get_field("multi_numbers").unwrap();
        let bytes_field = index.schema().get_field("bytes").unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.segment_readers().len(), 1);
        let segment_reader = searcher.segment_readers().last().unwrap();

        let fast_fields = segment_reader.fast_fields();
        let fast_field = fast_fields.u64(int_field).unwrap();
        assert_eq!(fast_field.get(0u32), 1u64);
        assert_eq!(fast_field.get(1u32), 2u64);
        assert_eq!(fast_field.get(2u32), 3u64);
        assert_eq!(fast_field.get(3u32), 10u64);
        assert_eq!(fast_field.get(4u32), 20u64);
        assert_eq!(fast_field.get(5u32), 1_000u64);

        let get_vals = |fast_field: &MultiValuedFastFieldReader<u64>, doc_id: u32| -> Vec<u64> {
            let mut vals = vec![];
            fast_field.get_vals(doc_id, &mut vals);
            vals
        };
        let fast_fields = segment_reader.fast_fields();
        let fast_field = fast_fields.u64s(multi_numbers).unwrap();
        assert_eq!(&get_vals(&fast_field, 0), &[] as &[u64]);
        assert_eq!(&get_vals(&fast_field, 1), &[2, 3]);
        assert_eq!(&get_vals(&fast_field, 2), &[3, 4]);
        assert_eq!(&get_vals(&fast_field, 3), &[10, 11]);
        assert_eq!(&get_vals(&fast_field, 4), &[20]);
        assert_eq!(&get_vals(&fast_field, 5), &[1001, 1002]);

        let fast_field = fast_fields.bytes(bytes_field).unwrap();
        assert_eq!(fast_field.get_bytes(0), &[] as &[u8]);
        assert_eq!(fast_field.get_bytes(2), &[1, 2, 3]);
        assert_eq!(fast_field.get_bytes(5), &[5, 5]);

        // test new field norm mapping
        {
            let my_text_field = index.schema().get_field("text_field").unwrap();
            let fieldnorm_reader = segment_reader.get_fieldnorms_reader(my_text_field).unwrap();
            assert_eq!(fieldnorm_reader.fieldnorm(0), 0);
            assert_eq!(fieldnorm_reader.fieldnorm(1), 0);
            assert_eq!(fieldnorm_reader.fieldnorm(2), 2); // some text
            assert_eq!(fieldnorm_reader.fieldnorm(3), 1);
            assert_eq!(fieldnorm_reader.fieldnorm(5), 3); // the biggest num
        }

        let searcher = index.reader().unwrap().searcher();
        {
            let my_text_field = index.schema().get_field("text_field").unwrap();

            let do_search = |term: &str| {
                let query = QueryParser::for_index(&index, vec![my_text_field])
                    .parse_query(term)
                    .unwrap();
                let top_docs: Vec<(f32, DocAddress)> =
                    searcher.search(&query, &TopDocs::with_limit(3)).unwrap();

                top_docs.iter().map(|el| el.1.doc_id).collect::<Vec<_>>()
            };

            assert_eq!(do_search("some"), vec![2]);
            assert_eq!(do_search("blubber"), vec![3]);
            assert_eq!(do_search("biggest"), vec![5]);
        }
    }
}

#[cfg(all(test, feature = "unstable"))]
mod bench_sorted_index_merge {

    use crate::core::Index;
    //use cratedoc_id, readerdoc_id_mappinglet vals = reader.fate::schema;
    use crate::fastfield::FastFieldReader;
    use crate::indexer::merger::IndexMerger;
    use crate::schema::Cardinality;
    use crate::schema::Document;
    use crate::schema::IntOptions;
    use crate::schema::Schema;
    use crate::IndexSettings;
    use crate::IndexSortByField;
    use crate::IndexWriter;
    use crate::Order;
    use futures::executor::block_on;
    use test::{self, Bencher};
    fn create_index(sort_by_field: Option<IndexSortByField>) -> Index {
        let mut schema_builder = Schema::builder();
        let int_options = IntOptions::default()
            .set_fast(Cardinality::SingleValue)
            .set_indexed();
        let int_field = schema_builder.add_u64_field("intval", int_options);
        let int_field = schema_builder.add_u64_field("intval", int_options);
        let schema = schema_builder.build();

        let index_builder = Index::builder()
            .schema(schema)
            .settings(IndexSettings { sort_by_field });
        let index = index_builder.create_in_ram().unwrap();

        {
            let mut index_writer = index.writer_for_tests().unwrap();
            let index_doc = |index_writer: &mut IndexWriter, val: u64| {
                let mut doc = Document::default();
                doc.add_u64(int_field, val);
                index_writer.add_document(doc);
            };
            // 3 segments with 10_000 values in the fast fields
            for _ in 0..3 {
                index_doc(&mut index_writer, 5000); // fix to make it unordered
                for i in 0..100 {
                    index_doc(&mut index_writer, i);
                }
                index_writer.commit().unwrap();
            }
        }
        index
    }
    #[bench]
    fn create_sorted_index_walk_overkmerge_on_merge_fastfield(
        b: &mut Bencher,
    ) -> crate::Result<()> {
        let sort_by_field = IndexSortByField {
            field: "intval".to_string(),
            order: Order::Desc,
        };
        let index = create_index(Some(sort_by_field.clone()));
        let field = index.schema().get_field("intval").unwrap();
        let segments = index.searchable_segments().unwrap();
        let merger: IndexMerger =
            IndexMerger::open(index.schema(), index.settings().clone(), &segments[..])?;
        let doc_id_mapping = merger.generate_doc_id_mapping(&sort_by_field).unwrap();
        b.iter(|| {

            let sorted_doc_ids = doc_id_mapping.iter().map(|(doc_id, reader)|{
            let u64_reader: FastFieldReader<u64> = reader
                .fast_fields()
                .typed_fast_field_reader(field)
                .expect("Failed to find a reader for single fast field. This is a tantivy bug and it should never happen.");
                (doc_id, reader, u64_reader)
            });
            // add values in order of the new docids
            let mut val = 0;
            for (doc_id, _reader, field_reader) in sorted_doc_ids {
                val = field_reader.get(*doc_id);
            }

            val

        });

        Ok(())
    }
    #[bench]
    fn create_sorted_index_create_docid_mapping(b: &mut Bencher) -> crate::Result<()> {
        let sort_by_field = IndexSortByField {
            field: "intval".to_string(),
            order: Order::Desc,
        };
        let index = create_index(Some(sort_by_field.clone()));
        let field = index.schema().get_field("intval").unwrap();
        let segments = index.searchable_segments().unwrap();
        let merger: IndexMerger =
            IndexMerger::open(index.schema(), index.settings().clone(), &segments[..])?;
        b.iter(|| {
            merger.generate_doc_id_mapping(&sort_by_field).unwrap();
        });

        Ok(())
    }
}
