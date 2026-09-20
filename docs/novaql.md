# NovaQL Language Contract

NovaQL is NovaDB's own document query language. It is not SQL and is never
translated into SQL. This document records only syntax that has shipped; later
phases extend it as the parser and executor land.

## Phase 5: lexical syntax

The lexer accepts UTF-8 source and returns tokens with half-open UTF-8 byte
spans. It is total over valid Rust strings: malformed source returns a typed,
spanned `LexError` and never panics. Every successful stream ends in `Eof`.

Whitespace and `//` line comments or `/* ... */` block comments are ignored.
Block comments do not nest. Keywords are ASCII case-insensitive; identifiers
preserve their spelling and may use Unicode alphabetic characters.

### Keywords

```text
find insert into update set delete from where project sort by asc desc
limit skip create drop collection index on explain and or not true false null
```

These words are reserved. They establish the vocabulary for later parser
phases; their presence does not mean every command is executable yet.

### Literals

- Integers are unsigned decimal source forms that must fit `i64`; unary `-` is
  a separate token interpreted by the parser.
- Floats use decimal and/or exponent notation (`8.5`, `1e3`, `2.5E-2`) and must
  produce a finite `f64`.
- Double-quoted strings allow JSON escapes: `\"`, `\\`, `\/`, `\b`, `\f`,
  `\n`, `\r`, `\t`, and four-hex-digit `\uXXXX`. Valid UTF-16 surrogate pairs
  are combined; unpaired surrogates are errors.
- `true`, `false`, and `null` are keyword literals.

### Punctuation and operators

```text
{ } [ ] ( ) , : . ; |
= == ! != < <= > >= + - * / %
```

The pipe token supports NovaQL's document-processing style. For example, the
Phase 6 grammar is intended to parse forms such as:

```text
find students
| where cgpa >= 8.5 and active == true
| project name, cgpa
| sort by cgpa desc
| limit 10;
```

This example describes the token vocabulary and planned grammar shape only;
Phase 5 does not build an AST or execute a query.
