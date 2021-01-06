//! # Example
//! ```rust
//! use tantivy::tokenizer::*;
//!
//! let tokenizer = analyzer_builder(SimpleTokenizer)
//!   .filter(RemoveLongFilter::limit(5)).build();
//!
//! let mut stream = tokenizer.token_stream("toolong nice");
//! // because `toolong` is more than 5 characters, it is filtered
//! // out of the token stream.
//! assert_eq!(stream.next().unwrap().text, "nice");
//! assert!(stream.next().is_none());
//! ```
//!
use super::{Token, TokenFilter};

/// `RemoveLongFilter` removes tokens that are longer
/// than a given number of bytes (in UTF-8 representation).
///
/// It is especially useful when indexing unconstrained content.
/// e.g. Mail containing base-64 encoded pictures etc.
#[derive(Clone, Debug)]
pub struct RemoveLongFilter {
    limit: usize,
}

impl RemoveLongFilter {
    /// Creates a `RemoveLongFilter` given a limit in bytes of the UTF-8 representation.
    pub fn limit(limit: usize) -> RemoveLongFilter {
        RemoveLongFilter { limit }
    }
}

impl TokenFilter for RemoveLongFilter {
    fn transform(&mut self, token: Token) -> Option<Token> {
        if token.text.len() >= self.limit {
            return None;
        }
        Some(token)
    }
}
