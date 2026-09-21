use std::collections::BTreeSet;
use std::fmt;

use crate::ast::{
    Assignment, BinaryOperator, Command, Expression, ExpressionKind, Literal, ObjectField, Path,
    Query, SortDirection, SortKey, Stage, UnaryOperator,
};
use crate::{lex, Keyword, LexError, Span, Token, TokenKind};

/// A syntax error with its exact source range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Human-readable description of the violated grammar rule.
    pub message: String,
    /// Offending token or insertion-point source range.
    pub span: Span,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "NovaQL parse error at bytes {}..{}: {}",
            self.span.start, self.span.end, self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// Front-end error produced by either lexical or syntactic analysis.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryError {
    /// Tokenization failed.
    Lex(LexError),
    /// Grammar parsing failed.
    Parse(ParseError),
}

impl fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lex(error) => error.fmt(formatter),
            Self::Parse(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for QueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lex(error) => Some(error),
            Self::Parse(error) => Some(error),
        }
    }
}

/// Lexes and parses exactly one complete `NovaQL` query.
///
/// # Errors
///
/// Returns [`QueryError::Lex`] for malformed tokens or [`QueryError::Parse`]
/// for invalid grammar. Trailing non-comment input is rejected.
pub fn parse(source: &str) -> Result<Query, QueryError> {
    let tokens = lex(source).map_err(QueryError::Lex)?;
    Parser::new(&tokens)
        .parse_query()
        .map_err(QueryError::Parse)
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
}

impl<'a> Parser<'a> {
    const fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn parse_query(mut self) -> Result<Query, ParseError> {
        let start = self.current().span.start;
        let explain = self.consume_keyword(Keyword::Explain);
        let (command, mut stages) = self.parse_command()?;
        while self.consume_simple(&TokenKind::Pipe) {
            if !command.accepts_pipeline() {
                return Err(self.error("this command does not accept pipeline stages"));
            }
            stages.push(self.parse_stage()?);
        }
        if matches!(command, Command::Update { .. })
            && !stages.iter().any(|stage| matches!(stage, Stage::Set(_)))
        {
            return Err(self.error("update requires a `set` pipeline stage"));
        }
        if !matches!(command, Command::Update { .. })
            && stages.iter().any(|stage| matches!(stage, Stage::Set(_)))
        {
            return Err(self.error("`set` is only valid in an update pipeline"));
        }

        self.consume_simple(&TokenKind::Semicolon);
        let end = self.current().span.end;
        self.expect_simple(&TokenKind::Eof, "end of query")?;
        Ok(Query {
            explain,
            command,
            stages,
            span: Span::new(start, end),
        })
    }

    fn parse_command(&mut self) -> Result<(Command, Vec<Stage>), ParseError> {
        match self.current().kind {
            TokenKind::Identifier(_) => self.parse_collection_command(),
            TokenKind::Keyword(Keyword::Create) => {
                self.parse_create().map(|command| (command, vec![]))
            }
            TokenKind::Keyword(Keyword::Drop) => self.parse_drop().map(|command| (command, vec![])),
            _ => {
                Err(self.error("expected `<collection>.<operation>`, `create`, or `drop` command"))
            }
        }
    }

    fn parse_collection_command(&mut self) -> Result<(Command, Vec<Stage>), ParseError> {
        let collection = self.expect_identifier("collection name")?;
        self.expect_simple(&TokenKind::Dot, "`.` after collection name")?;
        match self.current().kind {
            TokenKind::Keyword(Keyword::Get) => {
                self.advance();
                let stages = self
                    .parse_filter_body()?
                    .map_or_else(Vec::new, |expression| vec![Stage::Filter(expression)]);
                Ok((Command::Get { collection }, stages))
            }
            TokenKind::Keyword(Keyword::Insert) => {
                self.advance();
                self.parse_insert(collection)
                    .map(|command| (command, vec![]))
            }
            TokenKind::Keyword(Keyword::Update) => {
                self.advance();
                let stages = self
                    .parse_filter_body()?
                    .map_or_else(Vec::new, |expression| vec![Stage::Filter(expression)]);
                Ok((Command::Update { collection }, stages))
            }
            TokenKind::Keyword(Keyword::Delete) => {
                self.advance();
                let stages = self
                    .parse_filter_body()?
                    .map_or_else(Vec::new, |expression| vec![Stage::Filter(expression)]);
                Ok((Command::Delete { collection }, stages))
            }
            _ => Err(self.error("expected `get`, `insert`, `update`, or `delete` after `.`")),
        }
    }

    fn parse_filter_body(&mut self) -> Result<Option<Expression>, ParseError> {
        self.expect_simple(&TokenKind::LeftBrace, "`{` before document predicate")?;
        if self.consume_simple(&TokenKind::RightBrace) {
            return Ok(None);
        }
        let expression = self.parse_expression(1)?;
        self.expect_simple(&TokenKind::RightBrace, "`}` after document predicate")?;
        Ok(Some(expression))
    }

    fn parse_insert(&mut self, collection: String) -> Result<Command, ParseError> {
        let document = self.parse_expression(1)?;
        if !matches!(document.kind, ExpressionKind::Object(_)) {
            return Err(ParseError {
                message: "insert requires an object literal".to_owned(),
                span: document.span,
            });
        }
        Ok(Command::Insert {
            collection,
            document,
        })
    }

    fn parse_create(&mut self) -> Result<Command, ParseError> {
        self.advance();
        if self.consume_keyword(Keyword::Collection) {
            return Ok(Command::CreateCollection {
                name: self.expect_identifier("collection name")?,
            });
        }
        if self.consume_keyword(Keyword::Index) {
            let name = self.expect_identifier("index name")?;
            self.expect_keyword(Keyword::On, "`on` after index name")?;
            let collection = self.expect_identifier("indexed collection name")?;
            self.expect_simple(&TokenKind::LeftParen, "`(` before indexed fields")?;
            let fields = self.parse_path_list()?;
            self.expect_simple(&TokenKind::RightParen, "`)` after indexed fields")?;
            return Ok(Command::CreateIndex {
                name,
                collection,
                fields,
            });
        }
        Err(self.error("expected `collection` or `index` after `create`"))
    }

    fn parse_drop(&mut self) -> Result<Command, ParseError> {
        self.advance();
        if self.consume_keyword(Keyword::Collection) {
            return Ok(Command::DropCollection {
                name: self.expect_identifier("collection name")?,
            });
        }
        if self.consume_keyword(Keyword::Index) {
            return Ok(Command::DropIndex {
                name: self.expect_identifier("index name")?,
            });
        }
        Err(self.error("expected `collection` or `index` after `drop`"))
    }

    fn parse_stage(&mut self) -> Result<Stage, ParseError> {
        match self.current().kind {
            TokenKind::Keyword(Keyword::Project) => {
                self.advance();
                Ok(Stage::Project(self.parse_path_list()?))
            }
            TokenKind::Keyword(Keyword::Sort) => self.parse_sort(),
            TokenKind::Keyword(Keyword::Limit) => {
                self.advance();
                Ok(Stage::Limit(
                    self.expect_nonnegative_integer("limit count")?,
                ))
            }
            TokenKind::Keyword(Keyword::Skip) => {
                self.advance();
                Ok(Stage::Skip(self.expect_nonnegative_integer("skip count")?))
            }
            TokenKind::Keyword(Keyword::Set) => {
                self.advance();
                Ok(Stage::Set(self.parse_assignments()?))
            }
            _ => Err(self.error("expected `project`, `sort`, `limit`, `skip`, or `set` after `|`")),
        }
    }

    fn parse_sort(&mut self) -> Result<Stage, ParseError> {
        self.advance();
        let mut keys = Vec::new();
        loop {
            let path = self.parse_path()?;
            let direction = if self.consume_keyword(Keyword::Desc) {
                SortDirection::Descending
            } else {
                self.consume_keyword(Keyword::Asc);
                SortDirection::Ascending
            };
            keys.push(SortKey { path, direction });
            if !self.consume_simple(&TokenKind::Comma) {
                break;
            }
        }
        Ok(Stage::Sort(keys))
    }

    fn parse_assignments(&mut self) -> Result<Vec<Assignment>, ParseError> {
        let mut assignments = Vec::new();
        loop {
            let path = self.parse_path()?;
            self.expect_simple(&TokenKind::Equal, "`=` in assignment")?;
            let value = self.parse_expression(1)?;
            assignments.push(Assignment { path, value });
            if !self.consume_simple(&TokenKind::Comma) {
                break;
            }
        }
        Ok(assignments)
    }

    fn parse_path_list(&mut self) -> Result<Vec<Path>, ParseError> {
        let mut paths = vec![self.parse_path()?];
        while self.consume_simple(&TokenKind::Comma) {
            paths.push(self.parse_path()?);
        }
        Ok(paths)
    }

    fn parse_path(&mut self) -> Result<Path, ParseError> {
        let token = self.take_identifier("field path")?;
        let start = token.span;
        let TokenKind::Identifier(first) = token.kind else {
            unreachable!("take_identifier returned a non-identifier")
        };
        let mut segments = vec![first];
        let mut end = start;
        while self.consume_simple(&TokenKind::Dot) {
            let segment = self.take_identifier("path segment after `.`")?;
            end = segment.span;
            let TokenKind::Identifier(value) = segment.kind else {
                unreachable!("take_identifier returned a non-identifier")
            };
            segments.push(value);
        }
        Ok(Path {
            segments,
            span: start.join(end),
        })
    }

    fn parse_expression(&mut self, minimum_precedence: u8) -> Result<Expression, ParseError> {
        let mut left = self.parse_unary()?;
        while let Some((operator, precedence)) = binary_operator(&self.current().kind) {
            if precedence < minimum_precedence {
                break;
            }
            self.advance();
            let right = self.parse_expression(precedence + 1)?;
            let span = left.span.join(right.span);
            left = Expression {
                kind: ExpressionKind::Binary {
                    left: Box::new(left),
                    operator,
                    right: Box::new(right),
                },
                span,
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expression, ParseError> {
        let operator = match self.current().kind {
            TokenKind::Keyword(Keyword::Not) | TokenKind::Bang => Some(UnaryOperator::Not),
            TokenKind::Minus => Some(UnaryOperator::Negate),
            TokenKind::Plus => Some(UnaryOperator::Positive),
            _ => None,
        };
        if let Some(operator) = operator {
            let start = self.advance().span;
            let operand = self.parse_unary()?;
            let span = start.join(operand.span);
            return Ok(Expression {
                kind: ExpressionKind::Unary {
                    operator,
                    operand: Box::new(operand),
                },
                span,
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expression, ParseError> {
        let token = self.advance();
        let expression = match token.kind {
            TokenKind::Integer(value) => literal(Literal::Integer(value), token.span),
            TokenKind::Float(value) => literal(Literal::Float(value), token.span),
            TokenKind::String(value) => literal(Literal::String(value), token.span),
            TokenKind::Keyword(Keyword::True) => literal(Literal::Boolean(true), token.span),
            TokenKind::Keyword(Keyword::False) => literal(Literal::Boolean(false), token.span),
            TokenKind::Keyword(Keyword::Null) => literal(Literal::Null, token.span),
            TokenKind::Identifier(first) => self.parse_path_expression(first, token.span)?,
            TokenKind::LeftParen => {
                let mut inner = self.parse_expression(1)?;
                let end = self.expect_simple(&TokenKind::RightParen, "`)` after expression")?;
                inner.span = token.span.join(end.span);
                inner
            }
            TokenKind::LeftBracket => self.parse_array(token.span)?,
            TokenKind::LeftBrace => self.parse_object(token.span)?,
            _ => {
                return Err(ParseError {
                    message: "expected expression".to_owned(),
                    span: token.span,
                });
            }
        };
        Ok(expression)
    }

    fn parse_path_expression(
        &mut self,
        first: String,
        start: Span,
    ) -> Result<Expression, ParseError> {
        let mut segments = vec![first];
        let mut end = start;
        while self.consume_simple(&TokenKind::Dot) {
            let segment = self.take_identifier("path segment after `.`")?;
            end = segment.span;
            let TokenKind::Identifier(value) = segment.kind else {
                unreachable!("take_identifier returned a non-identifier")
            };
            segments.push(value);
        }
        let path = Path {
            segments,
            span: start.join(end),
        };
        Ok(Expression {
            span: path.span,
            kind: ExpressionKind::Path(path),
        })
    }

    fn parse_array(&mut self, start: Span) -> Result<Expression, ParseError> {
        let mut values = Vec::new();
        if !self.at_simple(&TokenKind::RightBracket) {
            loop {
                values.push(self.parse_expression(1)?);
                if !self.consume_simple(&TokenKind::Comma) {
                    break;
                }
                if self.at_simple(&TokenKind::RightBracket) {
                    break;
                }
            }
        }
        let end = self.expect_simple(&TokenKind::RightBracket, "`]` after array")?;
        Ok(Expression {
            kind: ExpressionKind::Array(values),
            span: start.join(end.span),
        })
    }

    fn parse_object(&mut self, start: Span) -> Result<Expression, ParseError> {
        let mut fields = Vec::new();
        let mut names = BTreeSet::new();
        if !self.at_simple(&TokenKind::RightBrace) {
            loop {
                let name_token = self.advance();
                let (TokenKind::Identifier(name) | TokenKind::String(name)) = name_token.kind
                else {
                    return Err(ParseError {
                        message: "expected identifier or string object key".to_owned(),
                        span: name_token.span,
                    });
                };
                if !names.insert(name.clone()) {
                    return Err(ParseError {
                        message: format!("duplicate object field {name:?}"),
                        span: name_token.span,
                    });
                }
                self.expect_simple(&TokenKind::Colon, "`:` after object key")?;
                let value = self.parse_expression(1)?;
                let span = name_token.span.join(value.span);
                fields.push(ObjectField { name, value, span });
                if !self.consume_simple(&TokenKind::Comma) {
                    break;
                }
                if self.at_simple(&TokenKind::RightBrace) {
                    break;
                }
            }
        }
        let end = self.expect_simple(&TokenKind::RightBrace, "`}` after object")?;
        Ok(Expression {
            kind: ExpressionKind::Object(fields),
            span: start.join(end.span),
        })
    }

    fn expect_nonnegative_integer(&mut self, expected: &str) -> Result<u64, ParseError> {
        let token = self.advance();
        match token.kind {
            TokenKind::Integer(value) => u64::try_from(value).map_err(|_| ParseError {
                message: format!("expected non-negative {expected}"),
                span: token.span,
            }),
            _ => Err(ParseError {
                message: format!("expected {expected}"),
                span: token.span,
            }),
        }
    }

    fn expect_identifier(&mut self, expected: &str) -> Result<String, ParseError> {
        let token = self.take_identifier(expected)?;
        let TokenKind::Identifier(value) = token.kind else {
            unreachable!("take_identifier returned a non-identifier")
        };
        Ok(value)
    }

    fn take_identifier(&mut self, expected: &str) -> Result<Token, ParseError> {
        if matches!(self.current().kind, TokenKind::Identifier(_)) {
            Ok(self.advance())
        } else {
            Err(self.error(format!("expected {expected}")))
        }
    }

    fn expect_keyword(&mut self, keyword: Keyword, expected: &str) -> Result<Token, ParseError> {
        if self.consume_keyword(keyword) {
            Ok(self.tokens[self.position - 1].clone())
        } else {
            Err(self.error(format!("expected {expected}")))
        }
    }

    fn consume_keyword(&mut self, keyword: Keyword) -> bool {
        if self.current().kind == TokenKind::Keyword(keyword) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn expect_simple(&mut self, kind: &TokenKind, expected: &str) -> Result<Token, ParseError> {
        if self.at_simple(kind) {
            Ok(self.advance())
        } else {
            Err(self.error(format!("expected {expected}")))
        }
    }

    fn consume_simple(&mut self, kind: &TokenKind) -> bool {
        if self.at_simple(kind) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn at_simple(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(&self.current().kind) == std::mem::discriminant(kind)
    }

    fn current(&self) -> &Token {
        &self.tokens[self.position]
    }

    fn advance(&mut self) -> Token {
        let token = self.current().clone();
        if !matches!(token.kind, TokenKind::Eof) {
            self.position += 1;
        }
        token
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            span: self.current().span,
        }
    }
}

fn literal(value: Literal, span: Span) -> Expression {
    Expression {
        kind: ExpressionKind::Literal(value),
        span,
    }
}

fn binary_operator(kind: &TokenKind) -> Option<(BinaryOperator, u8)> {
    match kind {
        TokenKind::Keyword(Keyword::Or) => Some((BinaryOperator::Or, 1)),
        TokenKind::Keyword(Keyword::And) => Some((BinaryOperator::And, 2)),
        TokenKind::Equal | TokenKind::EqualEqual => Some((BinaryOperator::Equal, 3)),
        TokenKind::BangEqual => Some((BinaryOperator::NotEqual, 3)),
        TokenKind::Less => Some((BinaryOperator::Less, 3)),
        TokenKind::LessEqual => Some((BinaryOperator::LessEqual, 3)),
        TokenKind::Greater => Some((BinaryOperator::Greater, 3)),
        TokenKind::GreaterEqual => Some((BinaryOperator::GreaterEqual, 3)),
        TokenKind::Keyword(Keyword::Contains) => Some((BinaryOperator::Contains, 3)),
        TokenKind::Plus => Some((BinaryOperator::Add, 4)),
        TokenKind::Minus => Some((BinaryOperator::Subtract, 4)),
        TokenKind::Star => Some((BinaryOperator::Multiply, 5)),
        TokenKind::Slash => Some((BinaryOperator::Divide, 5)),
        TokenKind::Percent => Some((BinaryOperator::Remainder, 5)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(source: &str) -> Query {
        parse(source).unwrap()
    }

    #[test]
    fn parses_get_pipeline_and_expression_precedence() {
        let query = parsed(
            "explain students.get { cgpa >= 8.5 and skills contains \"Rust\" or rank + 2 * 3 < 10 } | project name, address.city | sort cgpa desc, name asc | skip 5 | limit 10;",
        );
        assert!(query.explain);
        assert_eq!(
            query.command,
            Command::Get {
                collection: "students".to_owned()
            }
        );
        assert_eq!(query.stages.len(), 5);
        assert!(matches!(query.stages[0], Stage::Filter(_)));
        let Stage::Filter(expression) = &query.stages[0] else {
            unreachable!()
        };
        let ExpressionKind::Binary { operator, .. } = expression.kind else {
            panic!("expected top-level binary expression")
        };
        assert_eq!(operator, BinaryOperator::Or);
        assert!(matches!(query.stages[1], Stage::Project(ref paths) if paths.len() == 2));
        assert!(matches!(query.stages[2], Stage::Sort(ref keys) if keys.len() == 2));
        assert_eq!(query.stages[3], Stage::Skip(5));
        assert_eq!(query.stages[4], Stage::Limit(10));
    }

    #[test]
    fn parses_insert_with_nested_document_and_arrays() {
        let query = parsed(
            r#"users.insert {name: "Ada", active: true, scores: [1, -2, 3.5], address: {city: "Pune"}, nothing: null};"#,
        );
        let Command::Insert {
            collection,
            document,
        } = query.command
        else {
            panic!("expected insert")
        };
        assert_eq!(collection, "users");
        assert!(matches!(document.kind, ExpressionKind::Object(ref fields) if fields.len() == 5));
    }

    #[test]
    fn parses_update_and_delete_commands() {
        let update =
            parsed(r"users.update { profile.age >= 18 } | set active = true, score = score + 1");
        assert!(matches!(update.command, Command::Update { .. }));
        assert!(matches!(update.stages[1], Stage::Set(ref values) if values.len() == 2));

        let delete = parsed("users.delete { active == false } | limit 100");
        assert!(matches!(delete.command, Command::Delete { .. }));
        assert_eq!(delete.stages.len(), 2);
    }

    #[test]
    fn parses_collection_and_index_ddl() {
        assert!(matches!(
            parsed("create collection students").command,
            Command::CreateCollection { .. }
        ));
        assert!(matches!(
            parsed("drop collection students").command,
            Command::DropCollection { .. }
        ));
        let index = parsed("create index by_cgpa on students (cgpa, address.city)");
        assert!(matches!(
            index.command,
            Command::CreateIndex { ref fields, .. } if fields.len() == 2
        ));
        assert!(matches!(
            parsed("drop index by_cgpa").command,
            Command::DropIndex { .. }
        ));
    }

    #[test]
    fn parentheses_and_unary_operators_override_precedence() {
        let query = parsed("values.get { not (a + b) * -c >= +10 }");
        let Stage::Filter(expression) = &query.stages[0] else {
            unreachable!()
        };
        assert!(matches!(
            expression.kind,
            ExpressionKind::Binary {
                operator: BinaryOperator::GreaterEqual,
                ..
            }
        ));
    }

    #[test]
    fn rejects_invalid_or_trailing_syntax_with_spans() {
        let cases = [
            "",
            "find students",
            "students",
            "students.get",
            "students.get { active",
            "users.insert 42",
            "users.insert {a: 1, a: 2}",
            "users.update { active }",
            "students.get {} | set active = true",
            "create index x on users ()",
            "students.get {} trailing",
            "students.get {};;",
            "users.delete",
            "students.get {} | limit -1",
            "students.get {} | sort by score",
        ];
        for source in cases {
            let result = std::panic::catch_unwind(|| parse(source));
            assert!(result.is_ok(), "parser panicked for {source:?}");
            let error = result.unwrap().unwrap_err();
            match error {
                QueryError::Lex(error) => assert!(error.span.end <= source.len()),
                QueryError::Parse(error) => assert!(error.span.end <= source.len()),
            }
        }
    }

    #[test]
    fn lexical_errors_are_preserved() {
        assert!(matches!(
            parse("students.get { @ }"),
            Err(QueryError::Lex(_))
        ));
    }
}
