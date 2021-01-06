use super::{Token, Tokenizer};
use crate::schema::FACET_SEP_BYTE;

/// The `FacetTokenizer` process a `Facet` binary representation
/// and emits a token for all of its parent.
///
/// For instance,  `/america/north_america/canada`
/// will emit the three following tokens
///     - `/america/north_america/canada`
///     - `/america/north_america`
///     - `/america`
#[derive(Clone, Debug, Default)]
pub struct FacetTokenizer;

#[derive(Clone, Debug)]
enum State {
    RootFacetNotEmitted,
    UpToPosition(usize), //< we already emitted facet prefix up to &text[..cursor]
    Terminated,
}

#[derive(Clone, Debug)]
pub struct FacetTokenStream {
    text: String,
    state: State,
    token: Token,
}

impl Tokenizer for FacetTokenizer {
    type Iter = FacetTokenStream;
    fn token_stream(&self, text: &str) -> Self::Iter {
        FacetTokenStream {
            text: text.to_string(),
            state: State::RootFacetNotEmitted, //< pos is the first char that has not been processed yet.
            token: Token::default(),
        }
    }
}

impl Iterator for FacetTokenStream {
    type Item = Token;
    fn next(&mut self) -> Option<Self::Item> {
        self.state = match self.state {
            State::RootFacetNotEmitted => {
                if self.text.is_empty() {
                    State::Terminated
                } else {
                    State::UpToPosition(0)
                }
            }
            State::UpToPosition(cursor) => {
                if let Some(next_sep_pos) = self.text.as_bytes()[cursor + 1..]
                    .iter()
                    .position(|&b| b == FACET_SEP_BYTE)
                    .map(|pos| cursor + 1 + pos)
                {
                    let facet_part = &self.text[cursor..next_sep_pos];
                    self.token.text.push_str(facet_part);
                    State::UpToPosition(next_sep_pos)
                } else {
                    let facet_part = &self.text[cursor..];
                    self.token.text.push_str(facet_part);
                    State::Terminated
                }
            }
            State::Terminated => return None,
        };
        Some(self.token.clone())
    }
}

#[cfg(test)]
mod tests {

    use super::FacetTokenizer;
    use crate::schema::Facet;
    use crate::tokenizer::Tokenizer;

    #[test]
    fn test_facet_tokenizer() {
        let facet = Facet::from_path(vec!["top", "a", "b"]);
        let tokens: Vec<_> = FacetTokenizer
            .token_stream(facet.encoded_str())
            .map(|token| {
                Facet::from_encoded(token.text.as_bytes().to_owned())
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0], "/");
        assert_eq!(tokens[1], "/top");
        assert_eq!(tokens[2], "/top/a");
        assert_eq!(tokens[3], "/top/a/b");
    }

    #[test]
    fn test_facet_tokenizer_root_facets() {
        let facet = Facet::root();
        let tokens: Vec<_> = FacetTokenizer
            .token_stream(facet.encoded_str())
            .map(|token| {
                Facet::from_encoded(token.text.as_bytes().to_owned())
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], "/");
    }
}
