//! `NovaQL` front end.
//!
//! Phase 5 provides a total, span-aware lexer. The AST, parser, and semantic
//! validator are introduced in subsequent roadmap phases.

#![forbid(unsafe_code)]

mod lexer;

pub use lexer::{lex, Keyword, LexError, LexErrorKind, Span, Token, TokenKind};
