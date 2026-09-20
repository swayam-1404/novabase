//! `NovaQL` front end.
//!
//! Phase 5 provides a total, span-aware lexer. The AST, parser, and semantic
//! validator are introduced in subsequent roadmap phases.

#![forbid(unsafe_code)]

mod ast;
mod lexer;
mod parser;

pub use ast::{
    Assignment, BinaryOperator, Command, Expression, ExpressionKind, Literal, ObjectField, Path,
    Query, SortDirection, SortKey, Stage, UnaryOperator,
};
pub use lexer::{lex, Keyword, LexError, LexErrorKind, Span, Token, TokenKind};
pub use parser::{parse, ParseError, QueryError};
