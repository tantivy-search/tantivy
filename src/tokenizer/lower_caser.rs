use super::{Token, TokenFilter};
use std::mem;

impl TokenFilter for LowerCaser {
    fn transform(&mut self, mut token: Token) -> Option<Token> {
        if token.text.is_ascii() {
            // fast track for ascii.
            token.text.make_ascii_lowercase();
        } else {
            to_lowercase_unicode(&token.text, &mut self.buffer);
            mem::swap(&mut token.text, &mut self.buffer);
        }
        Some(token)
    }
}

/// Token filter that lowercase terms.
#[derive(Clone, Debug, Default)]
pub struct LowerCaser {
    buffer: String,
}

impl LowerCaser {
    /// Initialize the `LowerCaser`
    pub fn new() -> Self {
        LowerCaser {
            buffer: String::with_capacity(100),
        }
    }
}

// writes a lowercased version of text into output.
fn to_lowercase_unicode(text: &String, output: &mut String) {
    output.clear();
    for c in text.chars() {
        // Contrary to the std, we do not take care of sigma special case.
        // This will have an normalizationo effect, which is ok for search.
        output.extend(c.to_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::{analyzer_builder, LowerCaser, SimpleTokenizer, TextAnalyzerT};

    #[test]
    fn test_to_lower_case() {
        assert_eq!(lowercase_helper("Русский текст"), vec!["русский", "текст"]);
    }

    fn lowercase_helper(text: &str) -> Vec<String> {
        analyzer_builder(SimpleTokenizer)
            .filter(LowerCaser::new())
            .build()
            .token_stream(text)
            .map(|token| {
                let Token { text, .. } = token;
                text
            })
            .collect()
    }

    #[test]
    fn test_lowercaser() {
        assert_eq!(lowercase_helper("Tree"), vec!["tree"]);
        assert_eq!(lowercase_helper("Русский"), vec!["русский"]);
    }
}
