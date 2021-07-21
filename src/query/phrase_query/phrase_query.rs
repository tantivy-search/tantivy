use std::collections::BTreeMap;

use super::PhraseWeight;
use crate::core::searcher::Searcher;
use crate::query::bm25::Bm25Weight;
use crate::query::Query;
use crate::query::Weight;
use crate::schema::IndexRecordOption;
use crate::schema::{Field, Term};

/// `PhraseQuery` matches a specific sequence of words.
///
/// For instance the phrase query for `"part time"` will match
/// the sentence
///
/// **Alan just got a part time job.**
///
/// On the other hand it will not match the sentence.
///
/// **This is my favorite part of the job.**
///
/// Using a `PhraseQuery` on a field requires positions
/// to be indexed for this field.
///
#[derive(Clone, Debug)]
pub struct PhraseQuery {
    field: Field,
    phrase_terms: Vec<(usize, Term)>,
    slop: u32,
}

impl PhraseQuery {
    /// Creates a new `PhraseQuery` given a list of terms.
    ///
    /// There must be at least two terms, and all terms
    /// must belong to the same field.
    /// Offset for each term will be same as index in the Vector
    pub fn new(terms: Vec<Term>) -> PhraseQuery {
        let terms_with_offset = terms.into_iter().enumerate().collect();
        PhraseQuery::new_with_offset(terms_with_offset)
    }

    /// Creates a new `PhraseQuery` given a list of terms and their offsets.
    ///
    /// Can be used to provide custom offset for each term.
    pub fn new_with_offset(mut terms: Vec<(usize, Term)>) -> PhraseQuery {
        assert!(
            terms.len() > 1,
            "A phrase query is required to have strictly more than one term."
        );
        terms.sort_by_key(|&(offset, _)| offset);
        let field = terms[0].1.field();
        assert!(
            terms[1..].iter().all(|term| term.1.field() == field),
            "All terms from a phrase query must belong to the same field"
        );
        PhraseQuery {
            field,
            phrase_terms: terms,
            slop: 0,
        }
    }

    /// Slop allowed for the phrase.
    pub fn slop(&mut self, value: u32) {
        self.slop = value;
    }

    /// The `Field` this `PhraseQuery` is targeting.
    pub fn field(&self) -> Field {
        self.field
    }

    /// `Term`s in the phrase without the associated offsets.
    pub fn phrase_terms(&self) -> Vec<Term> {
        self.phrase_terms
            .iter()
            .map(|(_, term)| term.clone())
            .collect::<Vec<Term>>()
    }

    /// Returns the `PhraseWeight` for the given phrase query given a specific `searcher`.
    ///
    /// This function is the same as `.weight(...)` except it returns
    /// a specialized type `PhraseWeight` instead of a Boxed trait.
    pub(crate) fn phrase_weight(
        &self,
        searcher: &Searcher,
        scoring_enabled: bool,
    ) -> crate::Result<PhraseWeight> {
        let schema = searcher.schema();
        let field_entry = schema.get_field_entry(self.field);
        let has_positions = field_entry
            .field_type()
            .get_index_record_option()
            .map(IndexRecordOption::has_positions)
            .unwrap_or(false);
        if !has_positions {
            let field_name = field_entry.name();
            return Err(crate::TantivyError::SchemaError(format!(
                "Applied phrase query on field {:?}, which does not have positions indexed",
                field_name
            )));
        }
        let terms = self.phrase_terms();
        let bm25_weight = Bm25Weight::for_terms(searcher, &terms)?;
        let mut weight = PhraseWeight::new(self.phrase_terms.clone(), bm25_weight, scoring_enabled);
        if self.slop > 0 {
            weight.slop(self.slop);
        }
        Ok(weight)
    }
}

impl Query for PhraseQuery {
    /// Create the weight associated to a query.
    ///
    /// See [`Weight`](./trait.Weight.html).
    fn weight(&self, searcher: &Searcher, scoring_enabled: bool) -> crate::Result<Box<dyn Weight>> {
        let phrase_weight = self.phrase_weight(searcher, scoring_enabled)?;
        Ok(Box::new(phrase_weight))
    }

    fn query_terms(&self, terms: &mut BTreeMap<Term, bool>) {
        for (_, term) in &self.phrase_terms {
            terms.insert(term.clone(), true);
        }
    }
}
