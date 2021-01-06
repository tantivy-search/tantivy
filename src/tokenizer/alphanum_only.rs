//! # Example
//! ```rust
//! use tantivy::tokenizer::*;
//!
//! let tokenizer = analyzer_builder(RawTokenizer)
//!   .filter(AlphaNumOnlyFilter).build();
//!
//! let mut stream = tokenizer.token_stream("hello there");
//! // is none because the raw filter emits one token that
//! // contains a space
//! assert!(stream.next().is_none());
//!
//! let tokenizer = analyzer_builder(SimpleTokenizer)
//!   .filter(AlphaNumOnlyFilter).build();
//!
//! let mut stream = tokenizer.token_stream("hello there 💣");
//! assert!(stream.next().is_some());
//! assert!(stream.next().is_some());
//! // the "emoji" is dropped because its not an alphanum
//! assert!(stream.next().is_none());
//! ```
use super::{Token, TokenFilter};

/// `TokenFilter` that removes all tokens that contain non
/// ascii alphanumeric characters.
#[derive(Clone, Debug, Default)]
pub struct AlphaNumOnlyFilter;

impl TokenFilter for AlphaNumOnlyFilter {
    fn transform(&mut self, token: Token) -> Option<Token> {
        if token.text.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Some(token);
        }
        None
    }
}
