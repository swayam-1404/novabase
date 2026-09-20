use std::fmt;

/// Half-open UTF-8 byte range in `NovaQL` source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// Inclusive starting byte offset.
    pub start: usize,
    /// Exclusive ending byte offset.
    pub end: usize,
}

impl Span {
    /// Creates a source span.
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Returns the smallest span containing both inputs.
    #[must_use]
    pub const fn join(self, other: Self) -> Self {
        Self {
            start: if self.start < other.start {
                self.start
            } else {
                other.start
            },
            end: if self.end > other.end {
                self.end
            } else {
                other.end
            },
        }
    }
}

/// Reserved `NovaQL` words. Keyword matching is ASCII case-insensitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    /// `find` command.
    Find,
    /// `insert` command.
    Insert,
    /// `into` preposition.
    Into,
    /// `update` command.
    Update,
    /// `set` clause.
    Set,
    /// `delete` command.
    Delete,
    /// `from` clause.
    From,
    /// `where` filtering clause.
    Where,
    /// `project` projection clause.
    Project,
    /// `sort` ordering clause.
    Sort,
    /// `by` preposition.
    By,
    /// Ascending ordering.
    Asc,
    /// Descending ordering.
    Desc,
    /// Result limit.
    Limit,
    /// Result offset.
    Skip,
    /// `create` command.
    Create,
    /// `drop` command.
    Drop,
    /// Collection object type.
    Collection,
    /// Index object type.
    Index,
    /// `on` preposition.
    On,
    /// Explain command modifier.
    Explain,
    /// Logical conjunction.
    And,
    /// Logical disjunction.
    Or,
    /// Logical negation.
    Not,
    /// Boolean true literal.
    True,
    /// Boolean false literal.
    False,
    /// Null literal.
    Null,
}

/// A lexical `NovaQL` token.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// Non-keyword name.
    Identifier(String),
    /// Reserved word.
    Keyword(Keyword),
    /// Decoded string literal.
    String(String),
    /// Signed-range integer literal. A leading minus is a separate token.
    Integer(i64),
    /// Finite floating-point literal.
    Float(f64),
    /// `{`.
    LeftBrace,
    /// `}`.
    RightBrace,
    /// `[`.
    LeftBracket,
    /// `]`.
    RightBracket,
    /// `(`.
    LeftParen,
    /// `)`.
    RightParen,
    /// `,`.
    Comma,
    /// `:`.
    Colon,
    /// `.`.
    Dot,
    /// `;`.
    Semicolon,
    /// `|` pipeline separator.
    Pipe,
    /// `=`.
    Equal,
    /// `==`.
    EqualEqual,
    /// `!`.
    Bang,
    /// `!=`.
    BangEqual,
    /// `<`.
    Less,
    /// `<=`.
    LessEqual,
    /// `>`.
    Greater,
    /// `>=`.
    GreaterEqual,
    /// `+`.
    Plus,
    /// `-`.
    Minus,
    /// `*`.
    Star,
    /// `/`.
    Slash,
    /// `%`.
    Percent,
    /// End of input.
    Eof,
}

/// A token and its exact source range.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// Token payload or punctuation kind.
    pub kind: TokenKind,
    /// Half-open UTF-8 byte range in the original source.
    pub span: Span,
}

/// Classification of a `NovaQL` lexical error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexErrorKind {
    /// Character has no lexical meaning.
    UnexpectedCharacter(char),
    /// String reached end of input without a closing quote.
    UnterminatedString,
    /// Block comment reached end of input without `*/`.
    UnterminatedBlockComment,
    /// Backslash escape is not supported.
    InvalidEscape(char),
    /// Unicode escape is malformed or not a Unicode scalar value.
    InvalidUnicodeEscape,
    /// Numeric literal is malformed, overflows, or is non-finite.
    InvalidNumber,
}

/// A typed lexical error with the exact offending source range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    /// Error classification.
    pub kind: LexErrorKind,
    /// Offending half-open source range.
    pub span: Span,
}

impl fmt::Display for LexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "NovaQL lexical error at bytes {}..{}: ",
            self.span.start, self.span.end
        )?;
        match self.kind {
            LexErrorKind::UnexpectedCharacter(character) => {
                write!(formatter, "unexpected character {character:?}")
            }
            LexErrorKind::UnterminatedString => formatter.write_str("unterminated string"),
            LexErrorKind::UnterminatedBlockComment => {
                formatter.write_str("unterminated block comment")
            }
            LexErrorKind::InvalidEscape(character) => {
                write!(formatter, "invalid escape \\{character}")
            }
            LexErrorKind::InvalidUnicodeEscape => formatter.write_str("invalid Unicode escape"),
            LexErrorKind::InvalidNumber => formatter.write_str("invalid numeric literal"),
        }
    }
}

impl std::error::Error for LexError {}

/// Tokenizes a complete `NovaQL` source string.
///
/// The returned stream always ends with [`TokenKind::Eof`]. Whitespace and
/// `//` or `/* ... */` comments are discarded.
///
/// # Errors
///
/// Returns a span-aware [`LexError`] for malformed strings, comments, numbers,
/// escapes, or unexpected characters. Lexing arbitrary UTF-8 input never
/// panics.
pub fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).tokenize()
}

struct Lexer<'a> {
    source: &'a str,
    position: usize,
}

impl<'a> Lexer<'a> {
    const fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
        }
    }

    fn tokenize(mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia()?;
            let start = self.position;
            let Some(character) = self.advance() else {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    span: Span::new(start, start),
                });
                return Ok(tokens);
            };

            let kind = match character {
                '{' => TokenKind::LeftBrace,
                '}' => TokenKind::RightBrace,
                '[' => TokenKind::LeftBracket,
                ']' => TokenKind::RightBracket,
                '(' => TokenKind::LeftParen,
                ')' => TokenKind::RightParen,
                ',' => TokenKind::Comma,
                ':' => TokenKind::Colon,
                '.' => TokenKind::Dot,
                ';' => TokenKind::Semicolon,
                '|' => TokenKind::Pipe,
                '+' => TokenKind::Plus,
                '-' => TokenKind::Minus,
                '*' => TokenKind::Star,
                '/' => TokenKind::Slash,
                '%' => TokenKind::Percent,
                '=' => self.with_optional_equal(TokenKind::EqualEqual, TokenKind::Equal),
                '!' => self.with_optional_equal(TokenKind::BangEqual, TokenKind::Bang),
                '<' => self.with_optional_equal(TokenKind::LessEqual, TokenKind::Less),
                '>' => self.with_optional_equal(TokenKind::GreaterEqual, TokenKind::Greater),
                '"' => self.string(start)?,
                value if value.is_ascii_digit() => self.number(start)?,
                value if is_identifier_start(value) => self.identifier(start),
                unexpected => {
                    return Err(LexError {
                        kind: LexErrorKind::UnexpectedCharacter(unexpected),
                        span: Span::new(start, self.position),
                    });
                }
            };
            tokens.push(Token {
                kind,
                span: Span::new(start, self.position),
            });
        }
    }

    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.advance();
            }
            if self.starts_with("//") {
                while self.peek().is_some_and(|character| character != '\n') {
                    self.advance();
                }
            } else if self.starts_with("/*") {
                let start = self.position;
                self.position += 2;
                while !self.starts_with("*/") {
                    if self.advance().is_none() {
                        return Err(LexError {
                            kind: LexErrorKind::UnterminatedBlockComment,
                            span: Span::new(start, self.position),
                        });
                    }
                }
                self.position += 2;
            } else {
                return Ok(());
            }
        }
    }

    fn string(&mut self, start: usize) -> Result<TokenKind, LexError> {
        let mut value = String::new();
        loop {
            let escape_start = self.position;
            let Some(character) = self.advance() else {
                return Err(LexError {
                    kind: LexErrorKind::UnterminatedString,
                    span: Span::new(start, self.position),
                });
            };
            match character {
                '"' => return Ok(TokenKind::String(value)),
                '\\' => {
                    let Some(escaped) = self.advance() else {
                        return Err(LexError {
                            kind: LexErrorKind::UnterminatedString,
                            span: Span::new(start, self.position),
                        });
                    };
                    match escaped {
                        '"' => value.push('"'),
                        '\\' => value.push('\\'),
                        '/' => value.push('/'),
                        'b' => value.push('\u{0008}'),
                        'f' => value.push('\u{000c}'),
                        'n' => value.push('\n'),
                        'r' => value.push('\r'),
                        't' => value.push('\t'),
                        'u' => value.push(self.unicode_escape(escape_start)?),
                        invalid => {
                            return Err(LexError {
                                kind: LexErrorKind::InvalidEscape(invalid),
                                span: Span::new(escape_start, self.position),
                            });
                        }
                    }
                }
                control if control.is_control() => {
                    return Err(LexError {
                        kind: LexErrorKind::UnexpectedCharacter(control),
                        span: Span::new(escape_start, self.position),
                    });
                }
                value_character => value.push(value_character),
            }
        }
    }

    fn unicode_escape(&mut self, start: usize) -> Result<char, LexError> {
        let high = self.hex_quad(start)?;
        if (0xd800..=0xdbff).contains(&high) {
            if !self.starts_with("\\u") {
                return Err(self.invalid_unicode(start));
            }
            self.position += 2;
            let low = self.hex_quad(start)?;
            if !(0xdc00..=0xdfff).contains(&low) {
                return Err(self.invalid_unicode(start));
            }
            let scalar = 0x1_0000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(low) - 0xdc00);
            return char::from_u32(scalar).ok_or_else(|| self.invalid_unicode(start));
        }
        if (0xdc00..=0xdfff).contains(&high) {
            return Err(self.invalid_unicode(start));
        }
        char::from_u32(u32::from(high)).ok_or_else(|| self.invalid_unicode(start))
    }

    fn hex_quad(&mut self, start: usize) -> Result<u16, LexError> {
        let mut value = 0_u16;
        for _ in 0..4 {
            let Some(digit) = self.advance().and_then(|character| character.to_digit(16)) else {
                return Err(self.invalid_unicode(start));
            };
            value = (value << 4) | u16::try_from(digit).unwrap_or_default();
        }
        Ok(value)
    }

    fn invalid_unicode(&self, start: usize) -> LexError {
        LexError {
            kind: LexErrorKind::InvalidUnicodeEscape,
            span: Span::new(start, self.position),
        }
    }

    fn number(&mut self, start: usize) -> Result<TokenKind, LexError> {
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_digit())
        {
            self.advance();
        }
        let mut is_float = false;
        if self.peek() == Some('.')
            && self
                .peek_second()
                .is_some_and(|value| value.is_ascii_digit())
        {
            is_float = true;
            self.advance();
            while self
                .peek()
                .is_some_and(|character| character.is_ascii_digit())
            {
                self.advance();
            }
        }
        if self
            .peek()
            .is_some_and(|character| matches!(character, 'e' | 'E'))
        {
            is_float = true;
            self.advance();
            if self
                .peek()
                .is_some_and(|character| matches!(character, '+' | '-'))
            {
                self.advance();
            }
            if !self
                .peek()
                .is_some_and(|character| character.is_ascii_digit())
            {
                return Err(self.invalid_number(start));
            }
            while self
                .peek()
                .is_some_and(|character| character.is_ascii_digit())
            {
                self.advance();
            }
        }
        if self.peek().is_some_and(is_identifier_start) {
            while self.peek().is_some_and(is_identifier_continue) {
                self.advance();
            }
            return Err(self.invalid_number(start));
        }

        let text = &self.source[start..self.position];
        if is_float {
            let value = text
                .parse::<f64>()
                .map_err(|_| self.invalid_number(start))?;
            if !value.is_finite() {
                return Err(self.invalid_number(start));
            }
            Ok(TokenKind::Float(value))
        } else {
            text.parse::<i64>()
                .map(TokenKind::Integer)
                .map_err(|_| self.invalid_number(start))
        }
    }

    fn invalid_number(&self, start: usize) -> LexError {
        LexError {
            kind: LexErrorKind::InvalidNumber,
            span: Span::new(start, self.position),
        }
    }

    fn identifier(&mut self, start: usize) -> TokenKind {
        while self.peek().is_some_and(is_identifier_continue) {
            self.advance();
        }
        let text = &self.source[start..self.position];
        keyword(text).map_or_else(
            || TokenKind::Identifier(text.to_owned()),
            TokenKind::Keyword,
        )
    }

    fn with_optional_equal(&mut self, paired: TokenKind, single: TokenKind) -> TokenKind {
        if self.peek() == Some('=') {
            self.advance();
            paired
        } else {
            single
        }
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.source[self.position..].starts_with(prefix)
    }

    fn peek(&self) -> Option<char> {
        self.source[self.position..].chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        self.source[self.position..].chars().nth(1)
    }

    fn advance(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.position += character.len_utf8();
        Some(character)
    }
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character.is_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    character == '_'
        || character.is_alphanumeric()
        || (!character.is_ascii() && !character.is_whitespace() && !character.is_control())
}

fn keyword(value: &str) -> Option<Keyword> {
    const KEYWORDS: &[(&str, Keyword)] = &[
        ("find", Keyword::Find),
        ("insert", Keyword::Insert),
        ("into", Keyword::Into),
        ("update", Keyword::Update),
        ("set", Keyword::Set),
        ("delete", Keyword::Delete),
        ("from", Keyword::From),
        ("where", Keyword::Where),
        ("project", Keyword::Project),
        ("sort", Keyword::Sort),
        ("by", Keyword::By),
        ("asc", Keyword::Asc),
        ("desc", Keyword::Desc),
        ("limit", Keyword::Limit),
        ("skip", Keyword::Skip),
        ("create", Keyword::Create),
        ("drop", Keyword::Drop),
        ("collection", Keyword::Collection),
        ("index", Keyword::Index),
        ("on", Keyword::On),
        ("explain", Keyword::Explain),
        ("and", Keyword::And),
        ("or", Keyword::Or),
        ("not", Keyword::Not),
        ("true", Keyword::True),
        ("false", Keyword::False),
        ("null", Keyword::Null),
    ];
    KEYWORDS
        .iter()
        .find_map(|(word, keyword)| value.eq_ignore_ascii_case(word).then_some(*keyword))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        lex(source)
            .unwrap()
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    #[test]
    fn lexes_a_representative_novaql_pipeline() {
        assert_eq!(
            kinds("find students | where cgpa >= 8.5 and active == true | sort by cgpa desc | limit 10;"),
            vec![
                TokenKind::Keyword(Keyword::Find),
                TokenKind::Identifier("students".to_owned()),
                TokenKind::Pipe,
                TokenKind::Keyword(Keyword::Where),
                TokenKind::Identifier("cgpa".to_owned()),
                TokenKind::GreaterEqual,
                TokenKind::Float(8.5),
                TokenKind::Keyword(Keyword::And),
                TokenKind::Identifier("active".to_owned()),
                TokenKind::EqualEqual,
                TokenKind::Keyword(Keyword::True),
                TokenKind::Pipe,
                TokenKind::Keyword(Keyword::Sort),
                TokenKind::Keyword(Keyword::By),
                TokenKind::Identifier("cgpa".to_owned()),
                TokenKind::Keyword(Keyword::Desc),
                TokenKind::Pipe,
                TokenKind::Keyword(Keyword::Limit),
                TokenKind::Integer(10),
                TokenKind::Semicolon,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_document_literals_punctuation_and_operators() {
        let source = r#"insert into users {name: "Ada", scores: [1, -2, 3.0], ok: !false}; a.b != 0 + 2 * 4 / 2 % 3 <= 9 > 1 = 1"#;
        let tokens = kinds(source);
        for expected in [
            TokenKind::LeftBrace,
            TokenKind::RightBrace,
            TokenKind::LeftBracket,
            TokenKind::RightBracket,
            TokenKind::Colon,
            TokenKind::Comma,
            TokenKind::Minus,
            TokenKind::Bang,
            TokenKind::Dot,
            TokenKind::BangEqual,
            TokenKind::Plus,
            TokenKind::Star,
            TokenKind::Slash,
            TokenKind::Percent,
            TokenKind::LessEqual,
            TokenKind::Greater,
            TokenKind::Equal,
        ] {
            assert!(tokens.contains(&expected), "missing {expected:?}");
        }
    }

    #[test]
    fn strings_decode_json_escapes_unicode_and_surrogate_pairs() {
        assert_eq!(
            kinds(r#""line\nquote\" slash\\ snowman \u2603 planet \uD83C\uDF0D""#),
            vec![
                TokenKind::String("line\nquote\" slash\\ snowman ☃ planet 🌍".to_owned()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unicode_identifiers_and_byte_spans_are_preserved() {
        let source = "find विद्यार्थी";
        let tokens = lex(source).unwrap();
        assert_eq!(tokens[1].kind, TokenKind::Identifier("विद्यार्थी".to_owned()));
        assert_eq!(
            &source[tokens[1].span.start..tokens[1].span.end],
            "विद्यार्थी"
        );
        assert_eq!(
            tokens.last().unwrap().span,
            Span::new(source.len(), source.len())
        );
    }

    #[test]
    fn keywords_are_case_insensitive_but_identifiers_keep_case() {
        assert_eq!(
            kinds("FiNd Students TRUE null"),
            vec![
                TokenKind::Keyword(Keyword::Find),
                TokenKind::Identifier("Students".to_owned()),
                TokenKind::Keyword(Keyword::True),
                TokenKind::Keyword(Keyword::Null),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn comments_and_whitespace_are_discarded() {
        assert_eq!(
            kinds("find // one line\n users /* block */ | limit 1"),
            vec![
                TokenKind::Keyword(Keyword::Find),
                TokenKind::Identifier("users".to_owned()),
                TokenKind::Pipe,
                TokenKind::Keyword(Keyword::Limit),
                TokenKind::Integer(1),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn numeric_forms_are_typed() {
        assert_eq!(
            kinds("0 127 0.25 1e3 2.5E-2 1.foo"),
            vec![
                TokenKind::Integer(0),
                TokenKind::Integer(127),
                TokenKind::Float(0.25),
                TokenKind::Float(1_000.0),
                TokenKind::Float(0.025),
                TokenKind::Integer(1),
                TokenKind::Dot,
                TokenKind::Identifier("foo".to_owned()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn malformed_input_returns_spanned_errors_without_panicking() {
        let cases = [
            "@",
            "\"unterminated",
            "\"bad\\q\"",
            "\"\\u12xz\"",
            "/* open",
            "1e",
            "1e9999",
            "123abc",
        ];
        for source in cases {
            let result = std::panic::catch_unwind(|| lex(source));
            assert!(result.is_ok(), "lexer panicked for {source:?}");
            let error = result.unwrap().unwrap_err();
            assert!(error.span.start <= error.span.end);
            assert!(error.span.end <= source.len());
        }
    }
}
