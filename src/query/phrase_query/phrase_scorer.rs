use crate::docset::{DocSet, TERMINATED};
use crate::fieldnorm::FieldNormReader;
use crate::postings::Postings;
use crate::query::bm25::Bm25Weight;
use crate::query::{Intersection, Scorer};
use crate::{DocId, Score};
use std::cmp::Ordering;

struct PostingsWithOffset<TPostings> {
    offset: u32,
    postings: TPostings,
}

impl<TPostings: Postings> PostingsWithOffset<TPostings> {
    pub fn new(segment_postings: TPostings, offset: u32) -> PostingsWithOffset<TPostings> {
        PostingsWithOffset {
            offset,
            postings: segment_postings,
        }
    }

    pub fn positions(&mut self, output: &mut Vec<u32>) {
        self.postings.positions_with_offset(self.offset, output)
    }
}

impl<TPostings: Postings> DocSet for PostingsWithOffset<TPostings> {
    fn advance(&mut self) -> DocId {
        self.postings.advance()
    }

    fn seek(&mut self, target: DocId) -> DocId {
        self.postings.seek(target)
    }

    fn doc(&self) -> DocId {
        self.postings.doc()
    }

    fn size_hint(&self) -> u32 {
        self.postings.size_hint()
    }
}

pub struct PhraseScorer<TPostings: Postings> {
    intersection_docset: Intersection<PostingsWithOffset<TPostings>, PostingsWithOffset<TPostings>>,
    num_terms: usize,
    left: Vec<u32>,
    right: Vec<u32>,
    phrase_count: u32,
    fieldnorm_reader: FieldNormReader,
    similarity_weight: Bm25Weight,
    scoring_enabled: bool,
    slop: u32,
}

/// Returns true iff the two sorted array contain a common element
fn intersection_exists(left: &[u32], right: &[u32]) -> bool {
    let mut left_i = 0;
    let mut right_i = 0;
    while left_i < left.len() && right_i < right.len() {
        let left_val = left[left_i];
        let right_val = right[right_i];
        match left_val.cmp(&right_val) {
            Ordering::Less => {
                left_i += 1;
            }
            Ordering::Equal => {
                return true;
            }
            Ordering::Greater => {
                right_i += 1;
            }
        }
    }
    false
}

fn intersection_count(left: &[u32], right: &[u32]) -> usize {
    let mut left_i = 0;
    let mut right_i = 0;
    let mut count = 0;
    while left_i < left.len() && right_i < right.len() {
        let left_val = left[left_i];
        let right_val = right[right_i];
        match left_val.cmp(&right_val) {
            Ordering::Less => {
                left_i += 1;
            }
            Ordering::Equal => {
                count += 1;
                left_i += 1;
                right_i += 1;
            }
            Ordering::Greater => {
                right_i += 1;
            }
        }
    }
    count
}

/// Intersect twos sorted arrays `left` and `right` and outputs the
/// resulting array in left.
///
/// Returns the length of the intersection
fn intersection(left: &mut [u32], right: &[u32]) -> usize {
    let mut left_i = 0;
    let mut right_i = 0;
    let mut count = 0;
    let left_len = left.len();
    let right_len = right.len();
    while left_i < left_len && right_i < right_len {
        let left_val = left[left_i];
        let right_val = right[right_i];
        match left_val.cmp(&right_val) {
            Ordering::Less => {
                left_i += 1;
            }
            Ordering::Equal => {
                left[count] = left_val;
                count += 1;
                left_i += 1;
                right_i += 1;
            }
            Ordering::Greater => {
                right_i += 1;
            }
        }
    }
    count
}

/// Intersect twos sorted arrays `left` and `right` and outputs the
/// resulting array in left.
///
/// Condition for match is that the value stored in left is less than the value in right,
/// and that the difference between the value in left_begin is within
///
/// Returns the length of the intersection
fn intersection_with_distance(
    left: &mut [u32],
    left_begin: &mut [u32],
    right: &mut [u32],
    max_distance_to_begin: u32,
) -> usize {
    // TODO: Improve variable names?
    let mut left_i = 0;
    let mut right_i = 0;
    let mut count = 0;
    let left_len = left.len();
    let right_len = right.len();
    // Is the current last value guaranteed to be the final value.
    let mut is_temporary = false;
    while left_i < left_len && right_i < right_len {
        let left_val = left[left_i];
        let right_val = right[right_i];
        match left_val.cmp(&right_val) {
            Ordering::Less => {
                if right_val - left_begin[left_i] <= max_distance_to_begin {
                    if is_temporary {
                        // If the value was temporary we have found a closer match.
                        count -= 1;
                    };
                    left[count] = left_val;
                    left_begin[count] = left_begin[left_i];
                    right[count] = right_val;
                    count += 1;
                    left_i += 1;
                    // Still possible to find a closer match.
                    is_temporary = true;
                } else {
                    left_i += 1;
                }
            }
            Ordering::Equal => {
                if is_temporary {
                    // If the value was temporary we have found an.
                    count -= 1;
                    is_temporary = false;
                }
                left[count] = left_val;
                left_begin[count] = left_begin[left_i];
                right[count] = right_val;
                count += 1;
                left_i += 1;
                right_i += 1;
            }
            Ordering::Greater => {
                right_i += 1;
                // Given the constraint that left cannot be greater than right we know that the value in left is
                // final.
                is_temporary = false;
            }
        }
    }
    for i in 0..count {
        left[i] = right[i];
    }
    count
}

impl<TPostings: Postings> PhraseScorer<TPostings> {
    pub fn new(
        term_postings: Vec<(usize, TPostings)>,
        similarity_weight: Bm25Weight,
        fieldnorm_reader: FieldNormReader,
        scoring_enabled: bool,
        slop: u32,
    ) -> PhraseScorer<TPostings> {
        let max_offset = term_postings
            .iter()
            .map(|&(offset, _)| offset)
            .max()
            .unwrap_or(0);
        let num_docsets = term_postings.len();
        let postings_with_offsets = term_postings
            .into_iter()
            .map(|(offset, postings)| {
                PostingsWithOffset::new(postings, (max_offset - offset) as u32)
            })
            .collect::<Vec<_>>();
        let mut scorer = PhraseScorer {
            intersection_docset: Intersection::new(postings_with_offsets),
            num_terms: num_docsets,
            left: Vec::with_capacity(100),
            right: Vec::with_capacity(100),
            phrase_count: 0u32,
            similarity_weight,
            fieldnorm_reader,
            scoring_enabled,
            slop,
        };
        if scorer.doc() != TERMINATED && !scorer.phrase_match() {
            scorer.advance();
        }
        scorer
    }

    pub fn phrase_count(&self) -> u32 {
        self.phrase_count
    }

    fn phrase_match(&mut self) -> bool {
        // Need to add support for slop in phrase_count and phrase_exists.
        if self.scoring_enabled {
            let count = self.compute_phrase_count();
            self.phrase_count = count;
            count > 0u32
        } else {
            self.phrase_exists()
        }
    }

    fn phrase_exists(&mut self) -> bool {
        let intersection_len = if self.has_slop() {
            self.compute_match_with_slop()
        } else {
            self.compute_match()
        };
        intersection_exists(&self.left[..intersection_len], &self.right[..])
    }

    fn compute_phrase_count(&mut self) -> u32 {
        let intersection_len = if self.has_slop() {
            self.compute_match_with_slop()
        } else {
            self.compute_match()
        };
        intersection_count(&self.left[..intersection_len], &self.right[..]) as u32
    }

    /// Computes match without slop.
    fn compute_match(&mut self) -> usize {
        {
            self.intersection_docset
                .docset_mut_specialized(0)
                .positions(&mut self.left);
        }
        let mut intersection_len = self.left.len();
        for i in 1..self.num_terms - 1 {
            {
                self.intersection_docset
                    .docset_mut_specialized(i)
                    .positions(&mut self.right);
            }
            intersection_len = intersection(&mut self.left[..intersection_len], &self.right[..]);
            if intersection_len == 0 {
                return 0;
            }
        }

        self.intersection_docset
            .docset_mut_specialized(self.num_terms - 1)
            .positions(&mut self.right);
        intersection_len
    }

    // Computes match with slop.
    fn compute_match_with_slop(&mut self) -> usize {
        {
            self.intersection_docset
                .docset_mut_specialized(0)
                .positions(&mut self.left);
        }
        let mut intersection_len = self.left.len();
        // We'll increment the values to be equal to the next match in the right array to achieve ordered slop.
        let mut left_begin_vec = self.left.clone();
        let left_begin = &mut left_begin_vec[..];
        for i in 1..self.num_terms {
            {
                self.intersection_docset
                    .docset_mut_specialized(i)
                    .positions(&mut self.right);
            }
            intersection_len = intersection_with_distance(
                &mut self.left[..intersection_len],
                &mut left_begin[..intersection_len],
                &mut self.right[..],
                self.slop,
            );
            // Update the left to be equal to the right. Merge the initial left.
            if intersection_len == 0 {
                return 0;
            }
        }
        self.intersection_docset
            .docset_mut_specialized(self.num_terms - 1)
            .positions(&mut self.right);
        intersection_len
    }

    fn has_slop(&self) -> bool {
        self.slop > 0
    }
}

impl<TPostings: Postings> DocSet for PhraseScorer<TPostings> {
    fn advance(&mut self) -> DocId {
        loop {
            let doc = self.intersection_docset.advance();
            if doc == TERMINATED || self.phrase_match() {
                return doc;
            }
        }
    }

    fn seek(&mut self, target: DocId) -> DocId {
        debug_assert!(target >= self.doc());
        let doc = self.intersection_docset.seek(target);
        if doc == TERMINATED || self.phrase_match() {
            return doc;
        }
        self.advance()
    }

    fn doc(&self) -> DocId {
        self.intersection_docset.doc()
    }

    fn size_hint(&self) -> u32 {
        self.intersection_docset.size_hint()
    }
}

impl<TPostings: Postings> Scorer for PhraseScorer<TPostings> {
    fn score(&mut self) -> Score {
        let doc = self.doc();
        let fieldnorm_id = self.fieldnorm_reader.fieldnorm_id(doc);
        self.similarity_weight
            .score(fieldnorm_id, self.phrase_count)
    }
}

#[cfg(test)]
mod tests {
    use super::{intersection, intersection_count, intersection_with_distance};

    fn test_intersection_sym(left: &[u32], right: &[u32], expected: &[u32]) {
        test_intersection_aux(left, right, expected, 0);
        test_intersection_aux(right, left, expected, 0);
    }

    fn test_intersection_aux(left: &[u32], right: &[u32], expected: &[u32], slop: u32) {
        let mut left_vec = Vec::from(left);
        let left_mut = &mut left_vec[..];
        if slop == 0 {
            let left_mut = &mut left_vec[..];
            assert_eq!(intersection_count(left_mut, right), expected.len());
            let count = intersection(left_mut, right);
            assert_eq!(&left_mut[..count], expected);
            return;
        }
        let mut right_vec = Vec::from(right);
        let right_mut = &mut right_vec[..];
        let mut left_begin_vec = Vec::from(left);
        let left_begin_mut = &mut left_begin_vec[..];
        let count = intersection_with_distance(left_mut, left_begin_mut, right_mut, slop);
        assert_eq!(&left_mut[..count], expected);
    }

    #[test]
    fn test_intersection() {
        test_intersection_sym(&[1], &[1], &[1]);
        test_intersection_sym(&[1], &[2], &[]);
        test_intersection_sym(&[], &[2], &[]);
        test_intersection_sym(&[5, 7], &[1, 5, 10, 12], &[5]);
        test_intersection_sym(&[1, 5, 6, 9, 10, 12], &[6, 8, 9, 12], &[6, 9, 12]);
    }
    #[test]
    fn test_slop() {
        // The slop is not symetric. It does not allow for the phrase to be out of order.
        test_intersection_aux(&[1], &[2], &[2], 1);
        test_intersection_aux(&[1], &[3], &[], 1);
        test_intersection_aux(&[1], &[3], &[3], 2);
        test_intersection_aux(&[], &[2], &[], 100000);
        test_intersection_aux(&[5, 7, 11], &[1, 5, 10, 12], &[5, 12], 1);
        test_intersection_aux(&[1, 5, 6, 9, 10, 12], &[6, 8, 9, 12], &[6, 9, 12], 1);
        test_intersection_aux(&[1, 5, 6, 9, 10, 12], &[6, 8, 9, 12], &[6, 9, 12], 10);
    }

    fn test_merge(
        left: &[u32],
        right: &[u32],
        left_begin: &[u32],
        expected_left: &[u32],
        expected_left_begin: &[u32],
        slop: u32,
    ) {
        let mut left_vec = Vec::from(left);
        let left_mut = &mut left_vec[..];
        let mut right_vec = Vec::from(right);
        let right_mut = &mut right_vec[..];
        let mut left_begin_vec = Vec::from(left_begin);
        let left_begin_mut = &mut left_begin_vec[..];
        let count = intersection_with_distance(left_mut, left_begin_mut, right_mut, slop);
        assert_eq!(&left_mut[..count], expected_left);
        assert_eq!(&left_begin_mut[..count], expected_left_begin);
    }

    #[test]
    fn test_merge_slop() {
        test_merge(&[1, 2], &[1], &[0, 1], &[1], &[0], 1);
        test_merge(&[3], &[4], &[2], &[4], &[2], 2);
        test_merge(&[3], &[4], &[2], &[4], &[2], 2);
        test_merge(
            &[1, 5, 6, 9, 10, 12],
            &[6, 8, 9, 12],
            &[0, 1, 2, 3, 4, 5],
            &[6, 9, 12],
            &[2, 3, 5],
            10,
        );
    }
}

#[cfg(all(test, feature = "unstable"))]
mod bench {

    use super::{intersection, intersection_count};
    use test::Bencher;

    #[bench]
    fn bench_intersection_short(b: &mut Bencher) {
        b.iter(|| {
            let mut left = [1, 5, 10, 12];
            let right = [5, 7];
            intersection(&mut left, &right);
        });
    }

    #[bench]
    fn bench_intersection_count_short(b: &mut Bencher) {
        b.iter(|| {
            let left = [1, 5, 10, 12];
            let right = [5, 7];
            intersection_count(&left, &right);
        });
    }
}
