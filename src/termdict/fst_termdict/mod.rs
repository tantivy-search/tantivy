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
mod streamer;
mod term_info_store;
mod termdict;

pub use self::streamer::{
    TermStreamer, TermStreamerBuilder, TermWithStateStreamer, TermWithStateStreamerBuilder,
};
pub use self::termdict::{TermDictionary, TermDictionaryBuilder};
