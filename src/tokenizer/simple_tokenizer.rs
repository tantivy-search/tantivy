use super::{Token, Tokenizer};

/// Tokenize the text by splitting on whitespaces and punctuation.
#[derive(Clone, Debug)]
pub struct SimpleTokenizer;
impl Tokenizer for SimpleTokenizer {
    type Iter = SimpleTokenizerStream;
    fn token_stream(&self, text: &str) -> Self::Iter {
        let vec: Vec<_> = text.char_indices().collect();
        SimpleTokenizerStream {
            text: text.to_string(),
            chars: vec.into_iter(),
            position: usize::max_value(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SimpleTokenizerStream {
    text: String,
    chars: std::vec::IntoIter<(usize, char)>,
    position: usize,
}

impl SimpleTokenizerStream {
    // search for the end of the current token.
    fn search_token_end(&mut self) -> usize {
        (&mut self.chars)
            .filter(|&(_, c)| !c.is_alphanumeric())
            .map(|(offset, _)| offset)
            .next()
            .unwrap_or_else(|| self.text.len())
    }
}

impl Iterator for SimpleTokenizerStream {
    type Item = Token;
    fn next(&mut self) -> Option<Self::Item> {
        self.position = self.position.wrapping_add(1);
        while let Some((offset_from, c)) = self.chars.next() {
            if c.is_alphanumeric() {
                let offset_to = self.search_token_end();
                let token = Token {
                    text: self.text[offset_from..offset_to].into(),
                    offset_from,
                    offset_to,
                    position: self.position,
                    ..Default::default()
                };
                return Some(token);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let mut empty = SimpleTokenizer.token_stream("");
        assert_eq!(empty.next(), None);
    }

    #[test]
    fn simple_tokenizer() {
        let mut simple = SimpleTokenizer.token_stream("tokenizer hello world");
        assert_eq!(simple.next().unwrap().text, "tokenizer");
        assert_eq!(simple.next().unwrap().text, "hello");
        assert_eq!(simple.next().unwrap().text, "world");
    }
}
