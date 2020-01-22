//! # Example
//! ```rust
//! use tantivy::tokenizer::*;
//! use analyzer::{SimpleTokenizer, RemoveLongFilter};
//!
//! let tokenizer = Box::new(SimpleTokenizer)
//!   .filter(RemoveLongFilter::limit(5));
//!
//! let mut stream = tokenizer.token_stream("toolong nice");
//! // because `toolong` is more than 5 characters, it is filtered
//! // out of the token stream.
//! assert_eq!(stream.next().unwrap().text, "nice");
//! assert!(stream.next().is_none());
//! ```
//!
use super::{Token, TokenFilter, TokenStream};

/// `RemoveLongFilter` removes tokens that are longer
/// than a given number of bytes (in UTF-8 representation).
///
/// It is especially useful when indexing unconstrained content.
/// e.g. Mail containing base-64 encoded pictures etc.
#[derive(Clone)]
pub struct RemoveLongFilter {
    length_limit: usize,
}

impl RemoveLongFilter {
    /// Creates a `RemoveLongFilter` given a limit in bytes of the UTF-8 representation.
    pub fn limit(length_limit: usize) -> RemoveLongFilter {
        RemoveLongFilter { length_limit }
    }
}

impl<'a> RemoveLongFilterStream<'a> {
    fn predicate(&self, token: &Token) -> bool {
        token.text.len() < self.token_length_limit
    }

    fn wrap(token_length_limit: usize, tail: Box<dyn TokenStream + 'a>) -> RemoveLongFilterStream {
        RemoveLongFilterStream {
            token_length_limit,
            tail,
        }
    }
}

impl TokenFilter for RemoveLongFilter {
    fn transform<'a>(&self, token_stream: Box<dyn TokenStream + 'a>) -> Box<dyn TokenStream + 'a> {
        Box::new(RemoveLongFilterStream::wrap(
            self.length_limit,
            token_stream,
        ))
    }

    fn box_clone<'a>(&self) -> Box<dyn TokenFilter + 'a> {
        Box::new(self.clone())
    }
}

pub struct RemoveLongFilterStream<'a> {
    token_length_limit: usize,
    tail: Box<dyn TokenStream + 'a>,
}

impl<'a> TokenStream for RemoveLongFilterStream<'a> {
    fn advance(&mut self) -> bool {
        while self.tail.advance() {
            if self.predicate(self.tail.token()) {
                return true;
            }
        }
        false
    }

    fn token(&self) -> &Token {
        self.tail.token()
    }

    fn token_mut(&mut self) -> &mut Token {
        self.tail.token_mut()
    }
}
