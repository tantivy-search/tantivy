//! # Example
//! ```rust
//! use tantivy::tokenizer::*;
//!
//! let tokenizer = analyzer_builder(SimpleTokenizer)
//!   .filter(StopWordFilter::remove(vec!["the".to_string(), "is".to_string()])).build();
//!
//! let mut stream = tokenizer.token_stream("the fox is crafty");
//! assert_eq!(stream.next().unwrap().text, "fox");
//! assert_eq!(stream.next().unwrap().text, "crafty");
//! assert!(stream.next().is_none());
//! ```
use super::{Token, TokenFilter};
use fnv::FnvHasher;
use std::collections::HashSet;
use std::hash::BuildHasherDefault;

// configure our hashers for SPEED
type StopWordHasher = BuildHasherDefault<FnvHasher>;
type StopWordHashSet = HashSet<String, StopWordHasher>;

/// `TokenFilter` that removes stop words from a token stream
#[derive(Clone)]
pub struct StopWordFilter {
    words: StopWordHashSet,
}

impl StopWordFilter {
    /// Creates a `StopWordFilter` given a list of words to remove
    pub fn remove(words: Vec<String>) -> StopWordFilter {
        let mut set = StopWordHashSet::default();

        for word in words {
            set.insert(word);
        }

        StopWordFilter { words: set }
    }

    fn english() -> StopWordFilter {
        let words: [&'static str; 33] = [
            "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into",
            "is", "it", "no", "not", "of", "on", "or", "such", "that", "the", "their", "then",
            "there", "these", "they", "this", "to", "was", "will", "with",
        ];

        StopWordFilter::remove(words.iter().map(|&s| s.to_string()).collect())
    }
}

impl TokenFilter for StopWordFilter {
    fn transform(&mut self, token: Token) -> Option<Token> {
        if self.words.contains(&token.text) {
            return None;
        }
        Some(token)
    }
}

impl Default for StopWordFilter {
    fn default() -> StopWordFilter {
        StopWordFilter::english()
    }
}
