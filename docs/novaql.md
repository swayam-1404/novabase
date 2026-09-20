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

## Phase 6: AST and parser

Phase 6 parses exactly one query, with an optional trailing semicolon. Any other
trailing token is an error. `explain` may prefix a command. The supported
commands are:

```text
find <collection>
insert into <collection> <object>
update <collection>
delete from <collection>
create collection <name>
drop collection <name>
create index <name> on <collection> (<path>, ...)
drop index <name>
```

`find`, `update`, and `delete` accept pipe-separated stages:

```text
| where <expression>
| project <path>, ...
| sort by <path> [asc|desc], ...
| skip <non-negative integer>
| limit <non-negative integer>
| set <path> = <expression>, ...
```

The `set` stage is valid only for `update`, and an update requires at least one
`set`. Insert requires an object literal. Object keys must be identifiers or
quoted strings and may not be duplicated. Arrays and objects allow a trailing
comma.

Expressions support paths, scalar/array/object literals, parentheses, unary
`not`/`!`/`-`/`+`, and left-associative binary operators. From lowest to highest
precedence, the binary groups are `or`; `and`; comparisons; `+`/`-`; and
`*`/`/`/`%`.

The parser produces a public, strongly typed AST with spans on queries,
expressions, paths, and object fields. Lexical errors remain distinguishable
from parse errors through `QueryError`. Phase 6 does not execute or plan the
AST; that begins in Phase 7.

## Phase 7: execution

Phase 7 executes collection commands and pipelines through an
`ExecutionBackend`. Stages run left to right, expression evaluation has no
implicit type coercion, integer arithmetic is checked, boolean operators
short-circuit, and missing paths remain distinct from null. See
[`query-execution.md`](query-execution.md) for the complete runtime contract.

Index commands remain unsupported until Phases 8–9, and `explain` remains
unsupported until the Phase 10 planner.
