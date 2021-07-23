use super::term_scorer::TermScorer;
use crate::core::SegmentReader;
use crate::docset::DocSet;
use crate::fieldnorm::FieldNormReader;
use crate::postings::SegmentPostings;
use crate::query::bm25::Bm25Weight;
use crate::query::explanation::does_not_match;
use crate::query::weight::for_each_scorer;
use crate::query::Weight;
use crate::query::{Explanation, Scorer};
use crate::schema::IndexRecordOption;
use crate::Term;
use crate::{DocId, Score};

pub struct TermWeight {
    term: Term,
    index_record_option: IndexRecordOption,
    similarity_weight: Bm25Weight,
    scoring_enabled: bool,
}

impl Weight for TermWeight {
    fn scorer(&self, reader: &SegmentReader, boost: Score) -> crate::Result<Box<dyn Scorer>> {
        let term_scorer = self.specialized_scorer(reader, boost)?;
        Ok(Box::new(term_scorer))
    }

    fn explain(&self, reader: &SegmentReader, doc: DocId) -> crate::Result<Explanation> {
        let mut scorer = self.specialized_scorer(reader, 1.0)?;
        if scorer.doc() > doc || scorer.seek(doc) != doc {
            return Err(does_not_match(doc));
        }
        let mut explanation = scorer.explain();
        explanation.add_context(format!(
            "Term ={:?}:{:?}",
            self.term.field(),
            self.term.value_bytes()
        ));
        Ok(explanation)
    }

    fn count(&self, reader: &SegmentReader) -> crate::Result<u32> {
        if let Some(delete_bitset) = reader.delete_bitset() {
            Ok(self.scorer(reader, 1.0)?.count(delete_bitset))
        } else {
            let field = self.term.field();
            let inv_index = reader.inverted_index(field)?;
            let term_info = inv_index.get_term_info(&self.term)?;
            Ok(term_info.map(|term_info| term_info.doc_freq).unwrap_or(0))
        }
    }

    /// Iterates through all of the document matched by the DocSet
    /// `DocSet` and push the scored documents to the collector.
    fn for_each(
        &self,
        reader: &SegmentReader,
        callback: &mut dyn FnMut(DocId, Score),
    ) -> crate::Result<()> {
        let mut scorer = self.specialized_scorer(reader, 1.0)?;
        for_each_scorer(&mut scorer, callback);
        Ok(())
    }

    /// Calls `callback` with all of the `(doc, score)` for which score
    /// is exceeding a given threshold.
    ///
    /// This method is useful for the TopDocs collector.
    /// For all docsets, the blanket implementation has the benefit
    /// of prefiltering (doc, score) pairs, avoiding the
    /// virtual dispatch cost.
    ///
    /// More importantly, it makes it possible for scorers to implement
    /// important optimization (e.g. BlockWAND for union).
    fn for_each_pruning(
        &self,
        threshold: Score,
        reader: &SegmentReader,
        callback: &mut dyn FnMut(DocId, Score) -> Score,
    ) -> crate::Result<()> {
        let scorer = self.specialized_scorer(reader, 1.0)?;
        crate::query::boolean_query::block_wand(vec![scorer], threshold, callback);
        Ok(())
    }
}

impl TermWeight {
    pub fn new(
        term: Term,
        index_record_option: IndexRecordOption,
        similarity_weight: Bm25Weight,
        scoring_enabled: bool,
    ) -> TermWeight {
        TermWeight {
            term,
            index_record_option,
            similarity_weight,
            scoring_enabled,
        }
    }

    pub(crate) fn specialized_scorer(
        &self,
        reader: &SegmentReader,
        boost: Score,
    ) -> crate::Result<TermScorer> {
        let field = self.term.field();
        let inverted_index = reader.inverted_index(field)?;
        let fieldnorm_reader = if self.scoring_enabled {
            reader
                .get_fieldnorms_reader(field)
                .unwrap_or(FieldNormReader::constant(reader.max_doc(), 1))
        } else {
            FieldNormReader::constant(reader.max_doc(), 1)
        };
        let similarity_weight = self.similarity_weight.boost_by(boost);
        let postings_opt: Option<SegmentPostings> =
            inverted_index.read_postings(&self.term, self.index_record_option)?;
        if let Some(segment_postings) = postings_opt {
            Ok(TermScorer::new(
                segment_postings,
                fieldnorm_reader,
                similarity_weight,
            ))
        } else {
            Ok(TermScorer::new(
                SegmentPostings::empty(),
                fieldnorm_reader,
                similarity_weight,
            ))
        }
    }
}
