use super::logical_ast::*;
use crate::core::Index;
use crate::query::BooleanQuery;
use crate::query::EmptyQuery;
use crate::query::Occur;
use crate::query::PhraseQuery;
use crate::query::Query;
use crate::query::RangeQuery;
use crate::query::TermQuery;
use crate::query::{AllQuery, BoostQuery};
use crate::schema::{Facet, IndexRecordOption};
use crate::schema::{Field, Schema};
use crate::schema::{FieldType, Term};
use crate::tokenizer::TokenizerManager;
use crate::Score;
use std::borrow::Cow;
use std::collections::HashMap;
use std::num::{ParseFloatError, ParseIntError};
use std::ops::Bound;
use std::str::FromStr;
use tantivy_query_grammar::{UserInputAST, UserInputBound, UserInputLeaf};

/// Possible error that may happen when parsing a query.
#[derive(Debug, PartialEq, Eq, Error)]
pub enum QueryParserError {
    /// Error in the query syntax
    #[error("Syntax Error")]
    SyntaxError,
    /// `FieldDoesNotExist(field_name: String)`
    /// The query references a field that is not in the schema
    #[error("File does not exists: '{0:?}'")]
    FieldDoesNotExist(String),
    /// The query contains a term for a `u64` or `i64`-field, but the value
    /// is neither.
    #[error("Expected a valid integer: '{0:?}'")]
    ExpectedInt(ParseIntError),
    /// The query contains a term for a bytes field, but the value is not valid
    /// base64.
    #[error("Expected base64: '{0:?}'")]
    ExpectedBase64(base64::DecodeError),
    /// The query contains a term for a `f64`-field, but the value
    /// is not a f64.
    #[error("Invalid query: Only excluding terms given")]
    ExpectedFloat(ParseFloatError),
    /// It is forbidden queries that are only "excluding". (e.g. -title:pop)
    #[error("Invalid query: Only excluding terms given")]
    AllButQueryForbidden,
    /// If no default field is declared, running a query without any
    /// field specified is forbbidden.
    #[error("No default field declared and no field specified in query")]
    NoDefaultFieldDeclared,
    /// The field searched for is not declared
    /// as indexed in the schema.
    #[error("The field '{0:?}' is not declared as indexed")]
    FieldNotIndexed(String),
    /// A phrase query was requested for a field that does not
    /// have any positions indexed.
    #[error("The field '{0:?}' does not have positions indexed")]
    FieldDoesNotHavePositionsIndexed(String),
    /// The tokenizer for the given field is unknown
    /// The two argument strings are the name of the field, the name of the tokenizer
    #[error("The tokenizer '{0:?}' for the field '{1:?}' is unknown")]
    UnknownTokenizer(String, String),
    /// The query contains a range query with a phrase as one of the bounds.
    /// Only terms can be used as bounds.
    #[error("A range query cannot have a phrase as one of the bounds")]
    RangeMustNotHavePhrase,
    /// The format for the date field is not RFC 3339 compliant.
    #[error("The date field has an invalid format")]
    DateFormatError(chrono::ParseError),
}

impl From<ParseIntError> for QueryParserError {
    fn from(err: ParseIntError) -> QueryParserError {
        QueryParserError::ExpectedInt(err)
    }
}

impl From<ParseFloatError> for QueryParserError {
    fn from(err: ParseFloatError) -> QueryParserError {
        QueryParserError::ExpectedFloat(err)
    }
}

impl From<chrono::ParseError> for QueryParserError {
    fn from(err: chrono::ParseError) -> QueryParserError {
        QueryParserError::DateFormatError(err)
    }
}

/// Recursively remove empty clause from the AST
///
/// Returns `None` iff the `logical_ast` ended up being empty.
fn trim_ast(logical_ast: LogicalAST) -> Option<LogicalAST> {
    match logical_ast {
        LogicalAST::Clause(children) => {
            let trimmed_children = children
                .into_iter()
                .flat_map(|(occur, child)| {
                    trim_ast(child).map(|trimmed_child| (occur, trimmed_child))
                })
                .collect::<Vec<_>>();
            if trimmed_children.is_empty() {
                None
            } else {
                Some(LogicalAST::Clause(trimmed_children))
            }
        }
        _ => Some(logical_ast),
    }
}

/// Tantivy's Query parser
///
/// The language covered by the current parser is extremely simple.
///
/// * simple terms: "e.g.: `Barack Obama` are simply tokenized using
///   tantivy's [`SimpleTokenizer`](../tokenizer/struct.SimpleTokenizer.html), hence
///   becoming `["barack", "obama"]`. The terms are then searched within
///   the default terms of the query parser.
///
///   e.g. If `body` and `title` are default fields, our example terms are
///   `["title:barack", "body:barack", "title:obama", "body:obama"]`.
///   By default, all tokenized and indexed fields are default fields.
///
///   Multiple terms are handled as an `OR` : any document containing at least
///   one of the term will go through the scoring.
///
///   This behavior is slower, but is not a bad idea if the user is sorting
///   by relevance : The user typically just scans through the first few
///   documents in order of decreasing relevance and will stop when the documents
///   are not relevant anymore.
///
///   Switching to a default of `AND` can be done by calling `.set_conjunction_by_default()`.
///
///
/// * boolean operators `AND`, `OR`. `AND` takes precedence over `OR`, so that `a AND b OR c` is interpreted
/// as `(a AND b) OR c`.
///
/// * In addition to the boolean operators, the `-`, `+` can help define. These operators
///   are sufficient to express all queries using boolean operators. For instance `x AND y OR z` can
///   be written (`(+x +y) z`). In addition, these operators can help define "required optional"
///   queries. `(+x y)` matches the same document set as simply `x`, but `y` will help refining the score.
///
/// * negative terms: By prepending a term by a `-`, a term can be excluded
///   from the search. This is useful for disambiguating a query.
///   e.g. `apple -fruit`
///
/// * must terms: By prepending a term by a `+`, a term can be made required for the search.
///
/// * phrase terms: Quoted terms become phrase searches on fields that have positions indexed.
///   e.g., `title:"Barack Obama"` will only find documents that have "barack" immediately followed
///   by "obama".
///
/// * range terms: Range searches can be done by specifying the start and end bound. These can be
///   inclusive or exclusive. e.g., `title:[a TO c}` will find all documents whose title contains
///   a word lexicographically between `a` and `c` (inclusive lower bound, exclusive upper bound).
///   Inclusive bounds are `[]`, exclusive are `{}`.
///
/// * date values: The query parser supports rfc3339 formatted dates. For example "2002-10-02T15:00:00.05Z"
///
/// *  all docs query: A plain `*` will match all documents in the index.
///
/// Parts of the queries can be boosted by appending `^boostfactor`.
/// For instance, `"SRE"^2.0 OR devops^0.4` will boost documents containing `SRE` instead of
/// devops. Negative boosts are not allowed.
///
/// It is also possible to define a boost for a some specific field, at the query parser level.
/// (See [`set_boost(...)`](#method.set_field_boost) ). Typically you may want to boost a title
/// field.
#[derive(Clone)]
pub struct QueryParser {
    schema: Schema,
    default_fields: Vec<Field>,
    conjunction_by_default: bool,
    tokenizer_manager: TokenizerManager,
    boost: HashMap<Field, Score>,
}

fn all_negative(ast: &LogicalAST) -> bool {
    match ast {
        LogicalAST::Leaf(_) => false,
        LogicalAST::Boost(ref child_ast, _) => all_negative(&*child_ast),
        LogicalAST::Clause(children) => children
            .iter()
            .all(|(ref occur, child)| (*occur == Occur::MustNot) || all_negative(child)),
    }
}

impl QueryParser {
    /// Creates a `QueryParser`, given
    /// * schema - index Schema
    /// * default_fields - fields used to search if no field is specifically defined
    ///   in the query.
    pub fn new(
        schema: Schema,
        default_fields: Vec<Field>,
        tokenizer_manager: TokenizerManager,
    ) -> QueryParser {
        QueryParser {
            schema,
            default_fields,
            tokenizer_manager,
            conjunction_by_default: false,
            boost: Default::default(),
        }
    }

    /// Creates a `QueryParser`, given
    ///  * an index
    ///  * a set of default - fields used to search if no field is specifically defined
    ///   in the query.
    pub fn for_index(index: &Index, default_fields: Vec<Field>) -> QueryParser {
        QueryParser::new(index.schema(), default_fields, index.tokenizers().clone())
    }

    /// Set the default way to compose queries to a conjunction.
    ///
    /// By default, the query `happy tax payer` is equivalent to the query
    /// `happy OR tax OR payer`. After calling `.set_conjunction_by_default()`
    /// `happy tax payer` will be interpreted by the parser as `happy AND tax AND payer`.
    pub fn set_conjunction_by_default(&mut self) {
        self.conjunction_by_default = true;
    }

    /// Sets a boost for a specific field.
    ///
    /// The parse query will automatically boost this field.
    ///
    /// If the query defines a query boost through the query language (e.g: `country:France^3.0`),
    /// the two boosts (the one defined in the query, and the one defined in the `QueryParser`)
    /// are multiplied together.
    pub fn set_field_boost(&mut self, field: Field, boost: Score) {
        self.boost.insert(field, boost);
    }

    /// Parse a query
    ///
    /// Note that `parse_query` returns an error if the input
    /// is not a valid query.
    ///
    /// There is currently no lenient mode for the query parser
    /// which makes it a bad choice for a public/broad user search engine.
    ///
    /// Implementing a lenient mode for this query parser is tracked
    /// in [Issue 5](https://github.com/fulmicoton/tantivy/issues/5)
    pub fn parse_query(&self, query: &str) -> Result<Box<dyn Query>, QueryParserError> {
        let logical_ast = self.parse_query_to_logical_ast(query)?;
        Ok(convert_to_query(logical_ast))
    }

    /// Parse the user query into an AST.
    fn parse_query_to_logical_ast(&self, query: &str) -> Result<LogicalAST, QueryParserError> {
        let user_input_ast =
            tantivy_query_grammar::parse_query(query).map_err(|_| QueryParserError::SyntaxError)?;
        self.compute_logical_ast(user_input_ast)
    }

    fn resolve_field_name(&self, field_name: &str) -> Result<Field, QueryParserError> {
        self.schema
            .get_field(field_name)
            .ok_or_else(|| QueryParserError::FieldDoesNotExist(String::from(field_name)))
    }

    fn compute_logical_ast(
        &self,
        user_input_ast: UserInputAST,
    ) -> Result<LogicalAST, QueryParserError> {
        let ast = self.compute_logical_ast_with_occur(user_input_ast)?;
        if let LogicalAST::Clause(children) = &ast {
            if children.is_empty() {
                return Ok(ast);
            }
        }
        if all_negative(&ast) {
            return Err(QueryParserError::AllButQueryForbidden);
        }
        Ok(ast)
    }

    fn compute_terms_for_string(
        &self,
        field: Field,
        phrase: &str,
    ) -> Result<Vec<(usize, Term)>, QueryParserError> {
        let field_entry = self.schema.get_field_entry(field);
        let field_type = field_entry.field_type();
        if !field_type.is_indexed() {
            let field_name = field_entry.name().to_string();
            return Err(QueryParserError::FieldNotIndexed(field_name));
        }
        match field_type {
            FieldType::I64(_) => {
                let val: i64 = i64::from_str(phrase)?;
                let term = Term::from_field_i64(field, val);
                Ok(vec![(0, term)])
            }
            FieldType::F64(_) => {
                let val: f64 = f64::from_str(phrase)?;
                let term = Term::from_field_f64(field, val);
                Ok(vec![(0, term)])
            }
            FieldType::Date(_) => match chrono::DateTime::parse_from_rfc3339(phrase) {
                Ok(x) => Ok(vec![(
                    0,
                    Term::from_field_date(field, &x.with_timezone(&chrono::Utc)),
                )]),
                Err(e) => Err(QueryParserError::DateFormatError(e)),
            },
            FieldType::U64(_) => {
                let val: u64 = u64::from_str(phrase)?;
                let term = Term::from_field_u64(field, val);
                Ok(vec![(0, term)])
            }
            FieldType::Str(str_options) => {
                if let Some(option) = str_options.get_indexing_options() {
                    let tokenizer =
                        self.tokenizer_manager
                            .get(option.tokenizer())
                            .ok_or_else(|| {
                                QueryParserError::UnknownTokenizer(
                                    field_entry.name().to_string(),
                                    option.tokenizer().to_string(),
                                )
                            })?;
                    let token_stream = tokenizer.token_stream(phrase);
                    let terms: Vec<_> = token_stream
                        .map(|token| {
                            let term = Term::from_field_text(field, &token.text);
                            (token.position, term)
                        })
                        .collect();
                    if terms.len() <= 1 {
                        Ok(terms)
                    } else {
                        let field_entry = self.schema.get_field_entry(field);
                        let field_type = field_entry.field_type();
                        if let Some(index_record_option) = field_type.get_index_record_option() {
                            if index_record_option.has_positions() {
                                Ok(terms)
                            } else {
                                let fieldname = self.schema.get_field_name(field).to_string();
                                Err(QueryParserError::FieldDoesNotHavePositionsIndexed(
                                    fieldname,
                                ))
                            }
                        } else {
                            let fieldname = self.schema.get_field_name(field).to_string();
                            Err(QueryParserError::FieldNotIndexed(fieldname))
                        }
                    }
                } else {
                    // This should have been seen earlier really.
                    Err(QueryParserError::FieldNotIndexed(
                        field_entry.name().to_string(),
                    ))
                }
            }
            FieldType::HierarchicalFacet => {
                let facet = Facet::from_text(phrase);
                Ok(vec![(0, Term::from_field_text(field, facet.encoded_str()))])
            }
            FieldType::Bytes(_) => {
                let bytes = base64::decode(phrase).map_err(QueryParserError::ExpectedBase64)?;
                let term = Term::from_field_bytes(field, &bytes);
                Ok(vec![(0, term)])
            }
        }
    }

    fn compute_logical_ast_for_leaf(
        &self,
        field: Field,
        phrase: &str,
    ) -> Result<Option<LogicalLiteral>, QueryParserError> {
        let terms = self.compute_terms_for_string(field, phrase)?;
        match &terms[..] {
            [] => Ok(None),
            [(_, term)] => Ok(Some(LogicalLiteral::Term(term.clone()))),
            _ => Ok(Some(LogicalLiteral::Phrase(terms.clone()))),
        }
    }

    fn default_occur(&self) -> Occur {
        if self.conjunction_by_default {
            Occur::Must
        } else {
            Occur::Should
        }
    }

    fn resolve_bound(
        &self,
        field: Field,
        bound: &UserInputBound,
    ) -> Result<Bound<Term>, QueryParserError> {
        if bound.term_str() == "*" {
            return Ok(Bound::Unbounded);
        }
        let terms = self.compute_terms_for_string(field, bound.term_str())?;
        if terms.len() != 1 {
            return Err(QueryParserError::RangeMustNotHavePhrase);
        }
        let (_, term) = terms.into_iter().next().unwrap();
        match *bound {
            UserInputBound::Inclusive(_) => Ok(Bound::Included(term)),
            UserInputBound::Exclusive(_) => Ok(Bound::Excluded(term)),
            UserInputBound::Unbounded => Ok(Bound::Unbounded),
        }
    }

    fn resolved_fields(
        &self,
        given_field: &Option<String>,
    ) -> Result<Cow<'_, [Field]>, QueryParserError> {
        match given_field {
            None => {
                if self.default_fields.is_empty() {
                    Err(QueryParserError::NoDefaultFieldDeclared)
                } else {
                    Ok(Cow::from(&self.default_fields[..]))
                }
            }
            Some(field) => Ok(Cow::from(vec![self.resolve_field_name(&*field)?])),
        }
    }

    fn compute_logical_ast_with_occur(
        &self,
        user_input_ast: UserInputAST,
    ) -> Result<LogicalAST, QueryParserError> {
        match user_input_ast {
            UserInputAST::Clause(sub_queries) => {
                let default_occur = self.default_occur();
                let mut logical_sub_queries: Vec<(Occur, LogicalAST)> = Vec::new();
                for (occur_opt, sub_ast) in sub_queries {
                    let sub_ast = self.compute_logical_ast_with_occur(sub_ast)?;
                    let occur = occur_opt.unwrap_or(default_occur);
                    logical_sub_queries.push((occur, sub_ast));
                }
                Ok(LogicalAST::Clause(logical_sub_queries))
            }
            UserInputAST::Boost(ast, boost) => {
                let ast = self.compute_logical_ast_with_occur(*ast)?;
                Ok(ast.boost(boost as Score))
            }
            UserInputAST::Leaf(leaf) => self.compute_logical_ast_from_leaf(*leaf),
        }
    }

    fn field_boost(&self, field: Field) -> Score {
        self.boost.get(&field).cloned().unwrap_or(1.0)
    }

    fn compute_logical_ast_from_leaf(
        &self,
        leaf: UserInputLeaf,
    ) -> Result<LogicalAST, QueryParserError> {
        match leaf {
            UserInputLeaf::Literal(literal) => {
                let term_phrases: Vec<(Field, String)> = match literal.field_name {
                    Some(ref field_name) => {
                        let field = self.resolve_field_name(field_name)?;
                        vec![(field, literal.phrase.clone())]
                    }
                    None => {
                        if self.default_fields.is_empty() {
                            return Err(QueryParserError::NoDefaultFieldDeclared);
                        } else {
                            self.default_fields
                                .iter()
                                .map(|default_field| (*default_field, literal.phrase.clone()))
                                .collect::<Vec<(Field, String)>>()
                        }
                    }
                };
                let mut asts: Vec<LogicalAST> = Vec::new();
                for (field, phrase) in term_phrases {
                    if let Some(ast) = self.compute_logical_ast_for_leaf(field, &phrase)? {
                        // Apply some field specific boost defined at the query parser level.
                        let boost = self.field_boost(field);
                        asts.push(LogicalAST::Leaf(Box::new(ast)).boost(boost));
                    }
                }
                let result_ast: LogicalAST = if asts.len() == 1 {
                    asts.into_iter().next().unwrap()
                } else {
                    LogicalAST::Clause(asts.into_iter().map(|ast| (Occur::Should, ast)).collect())
                };
                Ok(result_ast)
            }
            UserInputLeaf::All => Ok(LogicalAST::Leaf(Box::new(LogicalLiteral::All))),
            UserInputLeaf::Range {
                field,
                lower,
                upper,
            } => {
                let fields = self.resolved_fields(&field)?;
                let mut clauses = fields
                    .iter()
                    .map(|&field| {
                        let boost = self.field_boost(field);
                        let field_entry = self.schema.get_field_entry(field);
                        let value_type = field_entry.field_type().value_type();
                        let logical_ast = LogicalAST::Leaf(Box::new(LogicalLiteral::Range {
                            field,
                            value_type,
                            lower: self.resolve_bound(field, &lower)?,
                            upper: self.resolve_bound(field, &upper)?,
                        }));
                        Ok(logical_ast.boost(boost))
                    })
                    .collect::<Result<Vec<_>, QueryParserError>>()?;
                let result_ast = if clauses.len() == 1 {
                    clauses.pop().unwrap()
                } else {
                    LogicalAST::Clause(
                        clauses
                            .into_iter()
                            .map(|clause| (Occur::Should, clause))
                            .collect(),
                    )
                };
                Ok(result_ast)
            }
        }
    }
}

fn convert_literal_to_query(logical_literal: LogicalLiteral) -> Box<dyn Query> {
    match logical_literal {
        LogicalLiteral::Term(term) => Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs)),
        LogicalLiteral::Phrase(term_with_offsets) => {
            Box::new(PhraseQuery::new_with_offset(term_with_offsets))
        }
        LogicalLiteral::Range {
            field,
            value_type,
            lower,
            upper,
        } => Box::new(RangeQuery::new_term_bounds(
            field, value_type, &lower, &upper,
        )),
        LogicalLiteral::All => Box::new(AllQuery),
    }
}

fn convert_to_query(logical_ast: LogicalAST) -> Box<dyn Query> {
    match trim_ast(logical_ast) {
        Some(LogicalAST::Clause(trimmed_clause)) => {
            let occur_subqueries = trimmed_clause
                .into_iter()
                .map(|(occur, subquery)| (occur, convert_to_query(subquery)))
                .collect::<Vec<_>>();
            assert!(
                !occur_subqueries.is_empty(),
                "Should not be empty after trimming"
            );
            Box::new(BooleanQuery::new(occur_subqueries))
        }
        Some(LogicalAST::Leaf(trimmed_logical_literal)) => {
            convert_literal_to_query(*trimmed_logical_literal)
        }
        Some(LogicalAST::Boost(ast, boost)) => {
            let query = convert_to_query(*ast);
            let boosted_query = BoostQuery::new(query, boost);
            Box::new(boosted_query)
        }
        None => Box::new(EmptyQuery),
    }
}

#[cfg(test)]
mod test {
    use super::super::logical_ast::*;
    use super::*;
    use crate::query::Query;
    use crate::schema::Field;
    use crate::schema::{IndexRecordOption, TextFieldIndexing, TextOptions};
    use crate::schema::{Schema, Term, INDEXED, STORED, STRING, TEXT};
    use crate::tokenizer::{analyzer_builder, LowerCaser, SimpleTokenizer, StopWordFilter};
    use crate::Index;
    use matches::assert_matches;

    fn make_schema() -> Schema {
        let mut schema_builder = Schema::builder();
        let text_field_indexing = TextFieldIndexing::default()
            .set_tokenizer("en_with_stop_words")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions);
        let text_options = TextOptions::default()
            .set_indexing_options(text_field_indexing)
            .set_stored();
        schema_builder.add_text_field("title", TEXT);
        schema_builder.add_text_field("text", TEXT);
        schema_builder.add_i64_field("signed", INDEXED);
        schema_builder.add_u64_field("unsigned", INDEXED);
        schema_builder.add_text_field("notindexed_text", STORED);
        schema_builder.add_text_field("notindexed_u64", STORED);
        schema_builder.add_text_field("notindexed_i64", STORED);
        schema_builder.add_text_field("nottokenized", STRING);
        schema_builder.add_text_field("with_stop_words", text_options);
        schema_builder.add_date_field("date", INDEXED);
        schema_builder.add_f64_field("float", INDEXED);
        schema_builder.add_facet_field("facet");
        schema_builder.add_bytes_field("bytes", INDEXED);
        schema_builder.add_bytes_field("bytes_not_indexed", STORED);
        schema_builder.build()
    }

    fn make_query_parser() -> QueryParser {
        let schema = make_schema();
        let default_fields: Vec<Field> = vec!["title", "text"]
            .into_iter()
            .flat_map(|field_name| schema.get_field(field_name))
            .collect();
        let tokenizer_manager = TokenizerManager::default();
        tokenizer_manager.register(
            "en_with_stop_words",
            analyzer_builder(SimpleTokenizer)
                .filter(LowerCaser::new())
                .filter(StopWordFilter::remove(vec!["the".to_string()]))
                .build(),
        );
        QueryParser::new(schema, default_fields, tokenizer_manager)
    }

    fn parse_query_to_logical_ast(
        query: &str,
        default_conjunction: bool,
    ) -> Result<LogicalAST, QueryParserError> {
        let mut query_parser = make_query_parser();
        if default_conjunction {
            query_parser.set_conjunction_by_default();
        }
        query_parser.parse_query_to_logical_ast(query)
    }

    fn test_parse_query_to_logical_ast_helper(
        query: &str,
        expected: &str,
        default_conjunction: bool,
    ) {
        let query = parse_query_to_logical_ast(query, default_conjunction).unwrap();
        let query_str = format!("{:?}", query);
        assert_eq!(query_str, expected);
    }

    #[test]
    pub fn test_parse_query_facet() {
        let query_parser = make_query_parser();
        let query = query_parser.parse_query("facet:/root/branch/leaf").unwrap();
        assert_eq!(
            format!("{:?}", query),
            "TermQuery(Term(field=11,bytes=[114, 111, 111, 116, 0, 98, 114, 97, 110, 99, 104, 0, 108, 101, 97, 102]))"
        );
    }

    #[test]
    pub fn test_parse_query_with_boost() {
        let mut query_parser = make_query_parser();
        let schema = make_schema();
        let text_field = schema.get_field("text").unwrap();
        query_parser.set_field_boost(text_field, 2.0);
        let query = query_parser.parse_query("text:hello").unwrap();
        assert_eq!(
            format!("{:?}", query),
            "Boost(query=TermQuery(Term(field=1,bytes=[104, 101, 108, 108, 111])), boost=2)"
        );
    }

    #[test]
    pub fn test_parse_query_range_with_boost() {
        let mut query_parser = make_query_parser();
        let schema = make_schema();
        let title_field = schema.get_field("title").unwrap();
        query_parser.set_field_boost(title_field, 2.0);
        let query = query_parser.parse_query("title:[A TO B]").unwrap();
        assert_eq!(
            format!("{:?}", query),
            "Boost(query=RangeQuery { field: Field(0), value_type: Str, left_bound: Included([97]), right_bound: Included([98]) }, boost=2)"
        );
    }

    #[test]
    pub fn test_parse_query_with_default_boost_and_custom_boost() {
        let mut query_parser = make_query_parser();
        let schema = make_schema();
        let text_field = schema.get_field("text").unwrap();
        query_parser.set_field_boost(text_field, 2.0);
        let query = query_parser.parse_query("text:hello^2").unwrap();
        assert_eq!(
            format!("{:?}", query),
            "Boost(query=Boost(query=TermQuery(Term(field=1,bytes=[104, 101, 108, 108, 111])), boost=2), boost=2)"
        );
    }

    #[test]
    pub fn test_parse_nonindexed_field_yields_error() {
        let query_parser = make_query_parser();

        let is_not_indexed_err = |query: &str| {
            let result: Result<Box<dyn Query>, QueryParserError> = query_parser.parse_query(query);
            if let Err(QueryParserError::FieldNotIndexed(field_name)) = result {
                Some(field_name.clone())
            } else {
                None
            }
        };

        assert_eq!(
            is_not_indexed_err("notindexed_text:titi"),
            Some(String::from("notindexed_text"))
        );
        assert_eq!(
            is_not_indexed_err("notindexed_u64:23424"),
            Some(String::from("notindexed_u64"))
        );
        assert_eq!(
            is_not_indexed_err("notindexed_i64:-234324"),
            Some(String::from("notindexed_i64"))
        );
    }

    #[test]
    pub fn test_parse_query_untokenized() {
        test_parse_query_to_logical_ast_helper(
            "nottokenized:\"wordone wordtwo\"",
            "Term(field=7,bytes=[119, 111, 114, 100, 111, 110, \
             101, 32, 119, 111, 114, 100, 116, 119, 111])",
            false,
        );
    }

    #[test]
    pub fn test_parse_query_empty() {
        test_parse_query_to_logical_ast_helper("", "<emptyclause>", false);
        test_parse_query_to_logical_ast_helper(" ", "<emptyclause>", false);
        let query_parser = make_query_parser();
        let query_result = query_parser.parse_query("");
        let query = query_result.unwrap();
        assert_eq!(format!("{:?}", query), "EmptyQuery");
    }

    #[test]
    pub fn test_parse_query_ints() {
        let query_parser = make_query_parser();
        assert!(query_parser.parse_query("signed:2324").is_ok());
        assert!(query_parser.parse_query("signed:\"22\"").is_ok());
        assert!(query_parser.parse_query("signed:\"-2234\"").is_ok());
        assert!(query_parser
            .parse_query("signed:\"-9999999999999\"")
            .is_ok());
        assert!(query_parser.parse_query("signed:\"a\"").is_err());
        assert!(query_parser.parse_query("signed:\"2a\"").is_err());
        assert!(query_parser
            .parse_query("signed:\"18446744073709551615\"")
            .is_err());
        assert!(query_parser.parse_query("unsigned:\"2\"").is_ok());
        assert!(query_parser.parse_query("unsigned:\"-2\"").is_err());
        assert!(query_parser
            .parse_query("unsigned:\"18446744073709551615\"")
            .is_ok());
        assert!(query_parser.parse_query("float:\"3.1\"").is_ok());
        assert!(query_parser.parse_query("float:\"-2.4\"").is_ok());
        assert!(query_parser.parse_query("float:\"2.1.2\"").is_err());
        assert!(query_parser.parse_query("float:\"2.1a\"").is_err());
        assert!(query_parser
            .parse_query("float:\"18446744073709551615.0\"")
            .is_ok());
        test_parse_query_to_logical_ast_helper(
            "unsigned:2324",
            "Term(field=3,bytes=[0, 0, 0, 0, 0, 0, 9, 20])",
            false,
        );

        test_parse_query_to_logical_ast_helper(
            "signed:-2324",
            &format!(
                "{:?}",
                Term::from_field_i64(Field::from_field_id(2u32), -2324)
            ),
            false,
        );

        test_parse_query_to_logical_ast_helper(
            "float:2.5",
            &format!(
                "{:?}",
                Term::from_field_f64(Field::from_field_id(10u32), 2.5)
            ),
            false,
        );
    }

    #[test]
    fn test_parse_bytes() {
        test_parse_query_to_logical_ast_helper(
            "bytes:YnVidQ==",
            "Term(field=12,bytes=[98, 117, 98, 117])",
            false,
        );
    }

    #[test]
    fn test_parse_bytes_not_indexed() {
        let error = parse_query_to_logical_ast("bytes_not_indexed:aaa", false).unwrap_err();
        assert!(matches!(error, QueryParserError::FieldNotIndexed(_)));
    }

    #[test]
    fn test_parse_bytes_phrase() {
        test_parse_query_to_logical_ast_helper(
            "bytes:\"YnVidQ==\"",
            "Term(field=12,bytes=[98, 117, 98, 117])",
            false,
        );
    }

    #[test]
    fn test_parse_bytes_invalid_base64() {
        let base64_err: QueryParserError =
            parse_query_to_logical_ast("bytes:aa", false).unwrap_err();
        assert!(matches!(base64_err, QueryParserError::ExpectedBase64(_)));
    }

    #[test]
    fn test_parse_query_to_ast_ab_c() {
        test_parse_query_to_logical_ast_helper(
            "(+title:a +title:b) title:c",
            "((+Term(field=0,bytes=[97]) +Term(field=0,bytes=[98])) Term(field=0,bytes=[99]))",
            false,
        );
        test_parse_query_to_logical_ast_helper(
            "(+title:a +title:b) title:c",
            "(+(+Term(field=0,bytes=[97]) +Term(field=0,bytes=[98])) +Term(field=0,bytes=[99]))",
            true,
        );
    }

    #[test]
    pub fn test_parse_query_to_ast_single_term() {
        test_parse_query_to_logical_ast_helper(
            "title:toto",
            "Term(field=0,bytes=[116, 111, 116, 111])",
            false,
        );
        test_parse_query_to_logical_ast_helper(
            "+title:toto",
            "Term(field=0,bytes=[116, 111, 116, 111])",
            false,
        );
        test_parse_query_to_logical_ast_helper(
            "+title:toto -titi",
            "(+Term(field=0,bytes=[116, 111, 116, 111]) \
             -(Term(field=0,bytes=[116, 105, 116, 105]) \
             Term(field=1,bytes=[116, 105, 116, 105])))",
            false,
        );
    }

    #[test]
    fn test_single_negative_term() {
        assert_matches!(
            parse_query_to_logical_ast("-title:toto", false),
            Err(QueryParserError::AllButQueryForbidden)
        );
    }

    #[test]
    pub fn test_parse_query_to_ast_two_terms() {
        test_parse_query_to_logical_ast_helper(
            "title:a b",
            "(Term(field=0,bytes=[97]) (Term(field=0,bytes=[98]) Term(field=1,bytes=[98])))",
            false,
        );
        test_parse_query_to_logical_ast_helper(
            "title:\"a b\"",
            "\"[(0, Term(field=0,bytes=[97])), \
             (1, Term(field=0,bytes=[98]))]\"",
            false,
        );
    }

    #[test]
    pub fn test_parse_query_to_ast_ranges() {
        test_parse_query_to_logical_ast_helper(
            "title:[a TO b]",
            "(Included(Term(field=0,bytes=[97])) TO Included(Term(field=0,bytes=[98])))",
            false,
        );
        test_parse_query_to_logical_ast_helper(
            "[a TO b]",
            "((Included(Term(field=0,bytes=[97])) TO \
             Included(Term(field=0,bytes=[98]))) \
             (Included(Term(field=1,bytes=[97])) TO \
             Included(Term(field=1,bytes=[98]))))",
            false,
        );
        test_parse_query_to_logical_ast_helper(
            "title:{titi TO toto}",
            "(Excluded(Term(field=0,bytes=[116, 105, 116, 105])) TO \
             Excluded(Term(field=0,bytes=[116, 111, 116, 111])))",
            false,
        );
        test_parse_query_to_logical_ast_helper(
            "title:{* TO toto}",
            "(Unbounded TO Excluded(Term(field=0,bytes=[116, 111, 116, 111])))",
            false,
        );
        test_parse_query_to_logical_ast_helper(
            "title:{titi TO *}",
            "(Excluded(Term(field=0,bytes=[116, 105, 116, 105])) TO Unbounded)",
            false,
        );
        test_parse_query_to_logical_ast_helper(
            "signed:{-5 TO 3}",
            "(Excluded(Term(field=2,bytes=[127, 255, 255, 255, 255, 255, 255, 251])) TO \
             Excluded(Term(field=2,bytes=[128, 0, 0, 0, 0, 0, 0, 3])))",
            false,
        );
        test_parse_query_to_logical_ast_helper(
            "float:{-1.5 TO 1.5}",
            "(Excluded(Term(field=10,bytes=[64, 7, 255, 255, 255, 255, 255, 255])) TO \
             Excluded(Term(field=10,bytes=[191, 248, 0, 0, 0, 0, 0, 0])))",
            false,
        );

        test_parse_query_to_logical_ast_helper("*", "*", false);
    }

    #[test]
    pub fn test_query_parser_field_does_not_exist() {
        let query_parser = make_query_parser();
        assert_matches!(
            query_parser.parse_query("boujou:\"18446744073709551615\""),
            Err(QueryParserError::FieldDoesNotExist(_))
        );
    }

    #[test]
    pub fn test_query_parser_field_not_indexed() {
        let query_parser = make_query_parser();
        assert_matches!(
            query_parser.parse_query("notindexed_text:\"18446744073709551615\""),
            Err(QueryParserError::FieldNotIndexed(_))
        );
    }

    #[test]
    pub fn test_unknown_tokenizer() {
        let mut schema_builder = Schema::builder();
        let text_field_indexing = TextFieldIndexing::default()
            .set_tokenizer("nonexistingtokenizer")
            .set_index_option(IndexRecordOption::Basic);
        let text_options = TextOptions::default().set_indexing_options(text_field_indexing);
        let title = schema_builder.add_text_field("title", text_options);
        let schema = schema_builder.build();
        let default_fields = vec![title];
        let tokenizer_manager = TokenizerManager::default();
        let query_parser = QueryParser::new(schema, default_fields, tokenizer_manager);
        assert_matches!(
            query_parser.parse_query("title:\"happy tax payer\""),
            Err(QueryParserError::UnknownTokenizer(_, _))
        );
    }

    #[test]
    pub fn test_query_parser_no_positions() {
        let mut schema_builder = Schema::builder();
        let text_field_indexing = TextFieldIndexing::default()
            .set_tokenizer("customtokenizer")
            .set_index_option(IndexRecordOption::Basic);
        let text_options = TextOptions::default().set_indexing_options(text_field_indexing);
        let title = schema_builder.add_text_field("title", text_options);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        index
            .tokenizers()
            .register("customtokenizer", SimpleTokenizer);
        let query_parser = QueryParser::for_index(&index, vec![title]);
        assert_eq!(
            query_parser.parse_query("title:\"happy tax\"").unwrap_err(),
            QueryParserError::FieldDoesNotHavePositionsIndexed("title".to_string())
        );
    }

    #[test]
    pub fn test_query_parser_expected_int() {
        let query_parser = make_query_parser();
        assert_matches!(
            query_parser.parse_query("unsigned:18a"),
            Err(QueryParserError::ExpectedInt(_))
        );
        assert!(query_parser.parse_query("unsigned:\"18\"").is_ok());
        assert_matches!(
            query_parser.parse_query("signed:18b"),
            Err(QueryParserError::ExpectedInt(_))
        );
        assert!(query_parser.parse_query("float:\"1.8\"").is_ok());
        assert_matches!(
            query_parser.parse_query("float:1.8a"),
            Err(QueryParserError::ExpectedFloat(_))
        );
    }

    #[test]
    pub fn test_query_parser_expected_date() {
        let query_parser = make_query_parser();
        assert_matches!(
            query_parser.parse_query("date:18a"),
            Err(QueryParserError::DateFormatError(_))
        );
        assert!(query_parser
            .parse_query("date:\"1985-04-12T23:20:50.52Z\"")
            .is_ok());
    }

    #[test]
    pub fn test_query_parser_not_empty_but_no_tokens() {
        let query_parser = make_query_parser();
        assert!(query_parser.parse_query(" !, ").is_ok());
        assert!(query_parser.parse_query("with_stop_words:the").is_ok());
    }

    #[test]
    pub fn test_parse_query_single_negative_term_through_error() {
        assert_matches!(
            parse_query_to_logical_ast("-title:toto", true),
            Err(QueryParserError::AllButQueryForbidden)
        );
        assert_matches!(
            parse_query_to_logical_ast("-title:toto", false),
            Err(QueryParserError::AllButQueryForbidden)
        );
    }

    #[test]
    pub fn test_parse_query_to_ast_conjunction() {
        test_parse_query_to_logical_ast_helper(
            "title:toto",
            "Term(field=0,bytes=[116, 111, 116, 111])",
            true,
        );
        test_parse_query_to_logical_ast_helper(
            "+title:toto",
            "Term(field=0,bytes=[116, 111, 116, 111])",
            true,
        );
        test_parse_query_to_logical_ast_helper(
            "+title:toto -titi",
            "(+Term(field=0,bytes=[116, 111, 116, 111]) \
             -(Term(field=0,bytes=[116, 105, 116, 105]) \
             Term(field=1,bytes=[116, 105, 116, 105])))",
            true,
        );
        test_parse_query_to_logical_ast_helper(
            "title:a b",
            "(+Term(field=0,bytes=[97]) \
             +(Term(field=0,bytes=[98]) \
             Term(field=1,bytes=[98])))",
            true,
        );
        test_parse_query_to_logical_ast_helper(
            "title:\"a b\"",
            "\"[(0, Term(field=0,bytes=[97])), \
             (1, Term(field=0,bytes=[98]))]\"",
            true,
        );
    }

    #[test]
    pub fn test_query_parser_hyphen() {
        test_parse_query_to_logical_ast_helper(
            "title:www-form-encoded",
            "\"[(0, Term(field=0,bytes=[119, 119, 119])), (1, Term(field=0,bytes=[102, 111, 114, 109])), (2, Term(field=0,bytes=[101, 110, 99, 111, 100, 101, 100]))]\"",
            false
        );
    }

    #[test]
    fn test_and_default_regardless_of_default_conjunctive() {
        for &default_conjunction in &[false, true] {
            test_parse_query_to_logical_ast_helper(
                "title:a AND title:b",
                "(+Term(field=0,bytes=[97]) +Term(field=0,bytes=[98]))",
                default_conjunction,
            );
        }
    }

    #[test]
    fn test_or_default_conjunctive() {
        for &default_conjunction in &[false, true] {
            test_parse_query_to_logical_ast_helper(
                "title:a OR title:b",
                "(Term(field=0,bytes=[97]) Term(field=0,bytes=[98]))",
                default_conjunction,
            );
        }
    }
}
