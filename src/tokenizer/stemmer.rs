use super::{Token, TokenFilter};
use rust_stemmers::{self, Algorithm};
use serde::{Deserialize, Serialize};

/// Available stemmer languages.
#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Copy, Clone)]
#[allow(missing_docs)]
pub enum Language {
    Arabic,
    Danish,
    Dutch,
    English,
    Finnish,
    French,
    German,
    Greek,
    Hungarian,
    Italian,
    Norwegian,
    Portuguese,
    Romanian,
    Russian,
    Spanish,
    Swedish,
    Tamil,
    Turkish,
}

impl Language {
    fn algorithm(self) -> Algorithm {
        use self::Language::*;
        match self {
            Arabic => Algorithm::Arabic,
            Danish => Algorithm::Danish,
            Dutch => Algorithm::Dutch,
            English => Algorithm::English,
            Finnish => Algorithm::Finnish,
            French => Algorithm::French,
            German => Algorithm::German,
            Greek => Algorithm::Greek,
            Hungarian => Algorithm::Hungarian,
            Italian => Algorithm::Italian,
            Norwegian => Algorithm::Norwegian,
            Portuguese => Algorithm::Portuguese,
            Romanian => Algorithm::Romanian,
            Russian => Algorithm::Russian,
            Spanish => Algorithm::Spanish,
            Swedish => Algorithm::Swedish,
            Tamil => Algorithm::Tamil,
            Turkish => Algorithm::Turkish,
        }
    }
}

/// `Stemmer` token filter. Several languages are supported, see `Language` for the available
/// languages.
/// Tokens are expected to be lowercased beforehand.
#[derive(Clone)]
pub struct Stemmer {
    stemmer: rust_stemmers::Stemmer,
}

impl Stemmer {
    /// Creates a new Stemmer `TokenFilter` for a given language algorithm.
    pub fn new(language: Language) -> Stemmer {
        let stemmer = rust_stemmers::Stemmer::create(language.algorithm());
        Stemmer { stemmer }
    }
}

impl Default for Stemmer {
    /// Creates a new Stemmer `TokenFilter` for English.
    fn default() -> Self {
        Stemmer::new(Language::English)
    }
}

impl TokenFilter for Stemmer {
    fn transform(&mut self, mut token: Token) -> Option<Token> {
        // TODO remove allocation
        let stemmed_str: String = self.stemmer.stem(&token.text).into_owned();
        // TODO remove clear
        token.text.clear();
        token.text.push_str(&stemmed_str);
        Some(token)
    }
}
