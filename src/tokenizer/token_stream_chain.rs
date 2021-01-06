use crate::tokenizer::Token;

const POSITION_GAP: usize = 2;

pub(crate) struct TokenStreamChain<Inner, Outer> {
    streams_with_offsets: Outer,
    current: Option<(Inner, usize)>,
    position: usize,
    position_shift: usize,
}

impl<'a, Inner, Outer> TokenStreamChain<Inner, Outer>
where
    Inner: Iterator<Item = Token>,
    Outer: Iterator<Item = (Inner, usize)>,
{
    pub fn new(mut streams_with_offsets: Outer) -> TokenStreamChain<Inner, Outer> {
        let current = streams_with_offsets.next();
        TokenStreamChain {
            streams_with_offsets: streams_with_offsets,
            current,
            position: usize::max_value(),
            position_shift: 0,
        }
    }
}

impl<'a, Inner, Outer> Iterator for TokenStreamChain<Inner, Outer>
where
    Inner: Iterator<Item = Token>,
    Outer: Iterator<Item = (Inner, usize)>,
{
    type Item = Token;
    fn next(&mut self) -> Option<Token> {
        while let Some((ref mut token_stream, offset_offset)) = self.current {
            if let Some(mut token) = token_stream.next() {
                token.offset_from += offset_offset;
                token.offset_to += offset_offset;
                token.position += self.position_shift;
                self.position = token.position;
                return Some(token);
            }
            self.position_shift = self.position.wrapping_add(POSITION_GAP);
            self.current = self.streams_with_offsets.next();
        }
        None
    }
}

impl DynTokenStreamChain {
    pub fn from_vec(
        streams_with_offsets: Vec<(Box<dyn Iterator<Item = Token>>, usize)>,
    ) -> impl Iterator<Item = Token> {
        DynTokenStreamChain {
            streams_with_offsets,
            idx: 0,
            position: usize::max_value(),
            position_shift: 0,
        }
    }
}

pub(crate) struct DynTokenStreamChain {
    streams_with_offsets: Vec<(Box<dyn Iterator<Item = Token>>, usize)>,
    idx: usize,
    position: usize,
    position_shift: usize,
}

impl Iterator for DynTokenStreamChain {
    type Item = Token;
    fn next(&mut self) -> Option<Token> {
        while let Some((token_stream, offset_offset)) = self.streams_with_offsets.get_mut(self.idx)
        {
            if let Some(mut token) = token_stream.next() {
                token.offset_from += *offset_offset;
                token.offset_to += *offset_offset;
                token.position += self.position_shift;
                self.position = token.position;
                return Some(token);
            }
            self.idx += 1;
            self.position_shift = self.position.wrapping_add(POSITION_GAP);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::tokenizer::Tokenizer;
    use super::super::SimpleTokenizer;
    use super::*;

    #[test]
    fn test_chain_first_emits_no_tokens() {
        let token_streams = vec![
            (SimpleTokenizer.token_stream(""), 0),
            (SimpleTokenizer.token_stream("hello world"), 0),
        ];
        let mut token_chain = TokenStreamChain::new(token_streams.into_iter());
        let token = token_chain.next();

        let expect = Token {
            offset_from: 0,
            offset_to: 5,
            position: POSITION_GAP - 1,
            text: "hello".into(),
            ..Token::default()
        };
        assert_eq!(token.unwrap(), expect);

        let token = token_chain.next().unwrap();
        assert_eq!(token.text, "world");
        assert_eq!(token.offset_from, 6);
        assert_eq!(token.offset_to, 11);
        assert_eq!(token.position, POSITION_GAP);

        assert!(token_chain.next().is_none());
    }
}
