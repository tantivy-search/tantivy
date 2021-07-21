mod phrase_query;
mod phrase_scorer;
mod phrase_weight;

pub use self::phrase_query::PhraseQuery;
pub use self::phrase_scorer::PhraseScorer;
pub use self::phrase_weight::PhraseWeight;

#[cfg(test)]
pub mod tests {

    use super::*;
    use crate::assert_nearly_equals;
    use crate::collector::tests::{TEST_COLLECTOR_WITHOUT_SCORE, TEST_COLLECTOR_WITH_SCORE};
    use crate::core::Index;
    use crate::query::Weight;
    use crate::schema::{Schema, Term, TEXT};
    use crate::DocId;
    use crate::{DocAddress, TERMINATED};

    pub fn create_index(texts: &[&'static str]) -> Index {
        let mut schema_builder = Schema::builder();
        let text_field = schema_builder.add_text_field("text", TEXT);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        {
            let mut index_writer = index.writer_for_tests().unwrap();
            for &text in texts {
                let doc = doc!(text_field=>text);
                index_writer.add_document(doc);
            }
            assert!(index_writer.commit().is_ok());
        }
        index
    }

    #[test]
    pub fn test_phrase_query() {
        let index = create_index(&[
            "b b b d c g c",
            "a b b d c g c",
            "a b a b c",
            "c a b a d ga a",
            "a b c",
        ]);
        let schema = index.schema();
        let text_field = schema.get_field("text").unwrap();
        let searcher = index.reader().unwrap().searcher();
        let test_query = |texts: Vec<&str>| {
            let terms: Vec<Term> = texts
                .iter()
                .map(|text| Term::from_field_text(text_field, text))
                .collect();
            let phrase_query = PhraseQuery::new(terms);
            let test_fruits = searcher
                .search(&phrase_query, &TEST_COLLECTOR_WITH_SCORE)
                .expect("search should succeed");
            test_fruits
                .docs()
                .iter()
                .map(|docaddr| docaddr.doc_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(test_query(vec!["a", "b"]), vec![1, 2, 3, 4]);
        assert_eq!(test_query(vec!["a", "b", "c"]), vec![2, 4]);
        assert_eq!(test_query(vec!["b", "b"]), vec![0, 1]);
        assert!(test_query(vec!["g", "ewrwer"]).is_empty());
        assert!(test_query(vec!["g", "a"]).is_empty());
    }

    #[test]
    pub fn test_phrase_query_simple() -> crate::Result<()> {
        let index = create_index(&["a b b d c g c", "a b a b c"]);
        let text_field = index.schema().get_field("text").unwrap();
        let searcher = index.reader()?.searcher();
        let terms: Vec<Term> = vec!["a", "b", "c"]
            .iter()
            .map(|text| Term::from_field_text(text_field, text))
            .collect();
        let phrase_query = PhraseQuery::new(terms);
        let phrase_weight = phrase_query.phrase_weight(&searcher, false)?;
        let mut phrase_scorer = phrase_weight.scorer(searcher.segment_reader(0), 1.0)?;
        assert_eq!(phrase_scorer.doc(), 1);
        assert_eq!(phrase_scorer.advance(), TERMINATED);
        Ok(())
    }

    #[test]
    pub fn test_phrase_query_no_score() {
        let index = create_index(&[
            "b b b d c g c",
            "a b b d c g c",
            "a b a b c",
            "c a b a d ga a",
            "a b c",
        ]);
        let schema = index.schema();
        let text_field = schema.get_field("text").unwrap();
        let searcher = index.reader().unwrap().searcher();
        let test_query = |texts: Vec<&str>| {
            let terms: Vec<Term> = texts
                .iter()
                .map(|text| Term::from_field_text(text_field, text))
                .collect();
            let phrase_query = PhraseQuery::new(terms);
            let test_fruits = searcher
                .search(&phrase_query, &TEST_COLLECTOR_WITHOUT_SCORE)
                .expect("search should succeed");
            test_fruits
                .docs()
                .iter()
                .map(|docaddr| docaddr.doc_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(test_query(vec!["a", "b", "c"]), vec![2, 4]);
        assert_eq!(test_query(vec!["a", "b"]), vec![1, 2, 3, 4]);
        assert_eq!(test_query(vec!["b", "b"]), vec![0, 1]);
        assert!(test_query(vec!["g", "ewrwer"]).is_empty());
        assert!(test_query(vec!["g", "a"]).is_empty());
    }

    #[test]
    pub fn test_phrase_query_no_positions() {
        let mut schema_builder = Schema::builder();
        use crate::schema::IndexRecordOption;
        use crate::schema::TextFieldIndexing;
        use crate::schema::TextOptions;
        let no_positions = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("default")
                .set_index_option(IndexRecordOption::WithFreqs),
        );

        let text_field = schema_builder.add_text_field("text", no_positions);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        {
            let mut index_writer = index.writer_for_tests().unwrap();
            index_writer.add_document(doc!(text_field=>"a b c"));
            assert!(index_writer.commit().is_ok());
        }
        let searcher = index.reader().unwrap().searcher();
        let phrase_query = PhraseQuery::new(vec![
            Term::from_field_text(text_field, "a"),
            Term::from_field_text(text_field, "b"),
        ]);

        let search_result = searcher
            .search(&phrase_query, &TEST_COLLECTOR_WITH_SCORE)
            .map(|_| ());
        assert!(matches!(
            search_result,
            Err(crate::TantivyError::SchemaError(msg))
            if msg == "Applied phrase query on field \"text\", which does not have positions \
            indexed"
        ));
    }

    #[test]
    pub fn test_phrase_score() {
        let index = create_index(&["a b c", "a b c a b"]);
        let schema = index.schema();
        let text_field = schema.get_field("text").unwrap();
        let searcher = index.reader().unwrap().searcher();
        let test_query = |texts: Vec<&str>| {
            let terms: Vec<Term> = texts
                .iter()
                .map(|text| Term::from_field_text(text_field, text))
                .collect();
            let phrase_query = PhraseQuery::new(terms);
            searcher
                .search(&phrase_query, &TEST_COLLECTOR_WITH_SCORE)
                .expect("search should succeed")
                .scores()
                .to_vec()
        };
        let scores = test_query(vec!["a", "b"]);
        assert_nearly_equals!(scores[0], 0.40618482);
        assert_nearly_equals!(scores[1], 0.46844664);
    }

    #[ignore]
    #[test]
    pub fn test_phrase_score_with_slop() {
        let index = create_index(&["a c b", "a b c a b"]);
        let schema = index.schema();
        let text_field = schema.get_field("text").unwrap();
        let searcher = index.reader().unwrap().searcher();
        let test_query = |texts: Vec<&str>| {
            let terms: Vec<Term> = texts
                .iter()
                .map(|text| Term::from_field_text(text_field, text))
                .collect();
            let mut phrase_query = PhraseQuery::new(terms);
            phrase_query.slop(1);
            searcher
                .search(&phrase_query, &TEST_COLLECTOR_WITH_SCORE)
                .expect("search should succeed")
                .scores()
                .to_vec()
        };
        let scores = test_query(vec!["a", "b"]);
        assert_nearly_equals!(scores[0], 0.40618482);
        assert_nearly_equals!(scores[1], 0.46844664);
    }

    #[test]
    pub fn test_phrase_score_with_slop_size() {
        let index = create_index(&["a b e c", "a e e e c", "a e e e e c"]);
        let schema = index.schema();
        let text_field = schema.get_field("text").unwrap();
        let searcher = index.reader().unwrap().searcher();
        let test_query = |texts: Vec<&str>| {
            let terms: Vec<Term> = texts
                .iter()
                .map(|text| Term::from_field_text(text_field, text))
                .collect();
            let mut phrase_query = PhraseQuery::new(terms);
            phrase_query.slop(3);
            searcher
                .search(&phrase_query, &TEST_COLLECTOR_WITH_SCORE)
                .expect("search should succeed")
                .scores()
                .to_vec()
        };
        let scores = test_query(vec!["a", "c"]);
        assert_nearly_equals!(scores[0], 0.29086056);
        assert_nearly_equals!(scores[1], 0.26706287);
    }

    #[test]
    pub fn test_phrase_score_with_slop_ordering() {
        let index = create_index(&[
            "a e b e c",
            "a e e e e e b e e e e c",
            "a c b",
            "a c e b e",
            "a e c b",
            "a e b c",
        ]);
        let schema = index.schema();
        let text_field = schema.get_field("text").unwrap();
        let searcher = index.reader().unwrap().searcher();
        let test_query = |texts: Vec<&str>| {
            let terms: Vec<Term> = texts
                .iter()
                .map(|text| Term::from_field_text(text_field, text))
                .collect();
            let mut phrase_query = PhraseQuery::new(terms);
            phrase_query.slop(3);
            searcher
                .search(&phrase_query, &TEST_COLLECTOR_WITH_SCORE)
                .expect("search should succeed")
                .scores()
                .to_vec()
        };
        let scores = test_query(vec!["a", "b", "c"]);
        // The first and last matches.
        assert_nearly_equals!(scores[0], 0.23091172);
        assert_nearly_equals!(scores[1], 0.25024384);
    }

    #[test] // motivated by #234
    pub fn test_phrase_query_docfreq_order() {
        let mut schema_builder = Schema::builder();
        let text_field = schema_builder.add_text_field("text", TEXT);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        {
            let mut index_writer = index.writer_for_tests().unwrap();
            index_writer.add_document(doc!(text_field=>"b"));
            index_writer.add_document(doc!(text_field=>"a b"));
            index_writer.add_document(doc!(text_field=>"b a"));
            assert!(index_writer.commit().is_ok());
        }

        let searcher = index.reader().unwrap().searcher();
        let test_query = |texts: Vec<&str>| {
            let terms: Vec<Term> = texts
                .iter()
                .map(|text| Term::from_field_text(text_field, text))
                .collect();
            let phrase_query = PhraseQuery::new(terms);
            searcher
                .search(&phrase_query, &TEST_COLLECTOR_WITH_SCORE)
                .expect("search should succeed")
                .docs()
                .to_vec()
        };
        assert_eq!(test_query(vec!["a", "b"]), vec![DocAddress::new(0, 1)]);
        assert_eq!(test_query(vec!["b", "a"]), vec![DocAddress::new(0, 2)]);
    }

    #[test] // motivated by #234
    pub fn test_phrase_query_non_trivial_offsets() {
        let mut schema_builder = Schema::builder();
        let text_field = schema_builder.add_text_field("text", TEXT);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        {
            let mut index_writer = index.writer_for_tests().unwrap();
            index_writer.add_document(doc!(text_field=>"a b c d e f g h"));
            assert!(index_writer.commit().is_ok());
        }
        let searcher = index.reader().unwrap().searcher();
        let test_query = |texts: Vec<(usize, &str)>| {
            let terms: Vec<(usize, Term)> = texts
                .iter()
                .map(|(offset, text)| (*offset, Term::from_field_text(text_field, text)))
                .collect();
            let phrase_query = PhraseQuery::new_with_offset(terms);
            searcher
                .search(&phrase_query, &TEST_COLLECTOR_WITH_SCORE)
                .expect("search should succeed")
                .docs()
                .iter()
                .map(|doc_address| doc_address.doc_id)
                .collect::<Vec<DocId>>()
        };
        assert_eq!(test_query(vec![(0, "a"), (1, "b")]), vec![0]);
        assert_eq!(test_query(vec![(1, "b"), (0, "a")]), vec![0]);
        assert!(test_query(vec![(0, "a"), (2, "b")]).is_empty());
        assert_eq!(test_query(vec![(0, "a"), (2, "c")]), vec![0]);
        assert_eq!(test_query(vec![(0, "a"), (2, "c"), (3, "d")]), vec![0]);
        assert_eq!(test_query(vec![(0, "a"), (2, "c"), (4, "e")]), vec![0]);
        assert_eq!(test_query(vec![(4, "e"), (0, "a"), (2, "c")]), vec![0]);
        assert!(test_query(vec![(0, "a"), (2, "d")]).is_empty());
        assert_eq!(test_query(vec![(1, "a"), (3, "c")]), vec![0]);
    }
}
