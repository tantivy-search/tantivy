/*!
The term dictionary main role is to associate the sorted [`Term`s](../struct.Term.html) to
a [`TermInfo`](../postings/struct.TermInfo.html) struct that contains some meta-information
about the term.

Internally, the term dictionary relies on the `fst` crate to store
a sorted mapping that associate each term to its rank in the lexicographical order.
For instance, in a dictionary containing the sorted terms "abba", "bjork", "blur" and "donovan",
the `TermOrdinal` are respectively `0`, `1`, `2`, and `3`.

For `u64`-terms, tantivy explicitely uses a `BigEndian` representation to ensure that the
lexicographical order matches the natural order of integers.

`i64`-terms are transformed to `u64` using a continuous mapping `val ⟶ val - i64::min_value()`
and then treated as a `u64`.

`f64`-terms are transformed to `u64` using a mapping that preserve order, and are then treated
as `u64`.

A second datastructure makes it possible to access a [`TermInfo`](../postings/struct.TermInfo.html).
*/

use tantivy_fst::automaton::AlwaysMatch;

mod fst_termdict;
use fst_termdict as termdict;

mod merger;

#[cfg(test)]
mod tests;

/// Position of the term in the sorted list of terms.
pub type TermOrdinal = u64;

/// The term dictionary contains all of the terms in
/// `tantivy index` in a sorted manner.
pub type TermDictionary = self::termdict::TermDictionary;

/// Builder for the new term dictionary.
///
/// Inserting must be done in the order of the `keys`.
pub type TermDictionaryBuilder<W> = self::termdict::TermDictionaryBuilder<W>;

/// Given a list of sorted term streams,
/// returns an iterator over sorted unique terms.
///
/// The item yield is actually a pair with
/// - the term
/// - a slice with the ordinal of the segments containing
/// the terms.
pub type TermMerger<'a> = self::merger::TermMerger<'a>;

/// `TermStreamer` acts as a cursor over a range of terms of a segment.
/// Terms are guaranteed to be sorted.
pub type TermStreamer<'a, A = AlwaysMatch> = self::termdict::TermStreamer<'a, A>;

/// `TermWithStateStreamer` acts as a cursor over a range of terms of a segment.
/// Terms are guaranteed to be sorted.
pub type TermWithStateStreamer<'a, A = AlwaysMatch> = self::termdict::TermWithStateStreamer<'a, A>;
