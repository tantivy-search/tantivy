use super::{Token, Tokenizer};

/// For each value of the field, emit a single unprocessed token.
#[derive(Clone, Debug, Default)]
pub struct RawTokenizer;

#[derive(Clone, Debug)]
pub struct RawTokenStream {
    token: Option<Token>,
}

impl Tokenizer for RawTokenizer {
    type Iter = RawTokenStream;
    fn token_stream(&self, text: &str) -> Self::Iter {
        let token = Token {
            offset_from: 0,
            offset_to: text.len(),
            position: 0,
            text: text.to_string(),
            position_length: 1,
        };
        RawTokenStream { token: Some(token) }
    }
}

impl Iterator for RawTokenStream {
    type Item = Token;
    fn next(&mut self) -> Option<Token> {
        self.token.take()
    }
}
