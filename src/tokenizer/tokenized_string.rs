use crate::tokenizer::{Token, TokenStreamChain};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Struct representing pre-tokenized text
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct PreTokenizedString {
    /// Original text
    pub text: String,
    /// Tokens derived from the text
    pub tokens: Vec<Token>,
}

impl Ord for PreTokenizedString {
    fn cmp(&self, other: &Self) -> Ordering {
        self.text.cmp(&other.text)
    }
}

impl PartialOrd for PreTokenizedString {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// TokenStream implementation which wraps PreTokenizedString
pub struct PreTokenizedStream {
    tokenized_string: PreTokenizedString,
    current_token: usize,
}

impl From<PreTokenizedString> for PreTokenizedStream {
    fn from(s: PreTokenizedString) -> PreTokenizedStream {
        PreTokenizedStream {
            tokenized_string: s,
            current_token: 0,
        }
    }
}

impl PreTokenizedStream {
    /// Creates a TokenStream from PreTokenizedString array
    pub fn chain_tokenized_strings<'a>(
        tok_strings: &'a [&PreTokenizedString],
    ) -> impl Iterator<Item = Token> + 'a {
        let streams_with_offsets = tok_strings.iter().scan(0, |total_offset, tok_string| {
            let next = Some((
                PreTokenizedStream::from((*tok_string).to_owned()),
                *total_offset,
            ));
            if let Some(last_token) = tok_string.tokens.last() {
                *total_offset += last_token.offset_to;
            }
            next
        });
        TokenStreamChain::new(streams_with_offsets)
    }
}

impl Iterator for PreTokenizedStream {
    type Item = Token;
    fn next(&mut self) -> Option<Token> {
        let token = self.tokenized_string.tokens.get(self.current_token)?;
        self.current_token += 1;
        Some(token.clone())
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    use crate::tokenizer::Token;

    #[test]
    fn test_tokenized_stream() {
        let tok_text = PreTokenizedString {
            text: String::from("A a"),
            tokens: vec![
                Token {
                    offset_from: 0,
                    offset_to: 1,
                    position: 0,
                    text: String::from("A"),
                    position_length: 1,
                },
                Token {
                    offset_from: 2,
                    offset_to: 3,
                    position: 1,
                    text: String::from("a"),
                    position_length: 1,
                },
            ],
        };

        let mut token_stream = PreTokenizedStream::from(tok_text.clone());

        for expected_token in tok_text.tokens {
            assert_eq!(token_stream.next().unwrap(), expected_token);
        }
        assert!(token_stream.next().is_none());
    }

    #[test]
    fn test_chain_tokenized_strings() {
        let tok_text = PreTokenizedString {
            text: String::from("A a"),
            tokens: vec![
                Token {
                    offset_from: 0,
                    offset_to: 1,
                    position: 0,
                    text: String::from("A"),
                    position_length: 1,
                },
                Token {
                    offset_from: 2,
                    offset_to: 3,
                    position: 1,
                    text: String::from("a"),
                    position_length: 1,
                },
            ],
        };

        let chain_parts = vec![&tok_text, &tok_text];

        let mut token_stream = PreTokenizedStream::chain_tokenized_strings(&chain_parts[..]);

        let expected_tokens = vec![
            Token {
                offset_from: 0,
                offset_to: 1,
                position: 0,
                text: String::from("A"),
                position_length: 1,
            },
            Token {
                offset_from: 2,
                offset_to: 3,
                position: 1,
                text: String::from("a"),
                position_length: 1,
            },
            Token {
                offset_from: 3,
                offset_to: 4,
                position: 3,
                text: String::from("A"),
                position_length: 1,
            },
            Token {
                offset_from: 5,
                offset_to: 6,
                position: 4,
                text: String::from("a"),
                position_length: 1,
            },
        ];

        for expected_token in expected_tokens {
            assert_eq!(token_stream.next().unwrap(), expected_token);
        }
        assert!(token_stream.next().is_none());
    }
}
