# NovaQL Language Contract

NovaQL is NovaDB's document-native query language. It is not SQL and is never
translated into SQL. Its core form is `collection.operation { ... }`, which
maps directly to collections, hierarchical documents, dotted field paths, and
array operators.

SQL databases can expose JSON functions and operators. NovaQL's distinction is
not that SQL is incapable of reading JSON; it is that document concepts are the
language's primary semantics rather than extensions to a relational grammar.

## Lexical syntax

The lexer accepts UTF-8 source and returns tokens with half-open UTF-8 byte
spans. It is total over valid Rust strings: malformed source returns a typed,
spanned `LexError` and never panics. Every successful stream ends in `Eof`.

Whitespace and `//` line comments or `/* ... */` block comments are ignored.
Block comments do not nest. Keywords are ASCII case-insensitive; identifiers
preserve their spelling and may use Unicode alphabetic characters.

### Keywords

```text
get insert update delete set project sort asc desc limit skip
create drop collection index on explain and or not contains true false null
```

The SQL-shaped words `find`, `where`, `from`, `into`, and `by` are not NovaQL
commands or clauses. The pre-document-native Phase 5–7 prototype syntax was
removed before the index phases and is intentionally not accepted as an alias.

### Literals

- Integers are unsigned decimal source forms that must fit `i64`; unary `-` is
  a separate parser operator.
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
and or not contains
```

## Document commands

NovaQL parses exactly one query with an optional trailing semicolon. Any other
trailing token is an error. `explain` may prefix a query, although plan output
is introduced with the Phase 10 planner.

### Read

```text
<collection>.get { <predicate> }
```

An empty predicate reads all documents:

```novaql
students.get {}
```

Scalar and nested-field filtering use the same document paths:

```novaql
students.get {
    branch == "CSE" and address.state == "Odisha"
}
```

Arrays use the document-specific `contains` operator:

```novaql
students.get {
    skills contains "Rust"
}
```

### Insert

```novaql
students.insert {
    name: "Ada",
    branch: "CSE",
    address: {state: "Odisha"},
    skills: ["Rust", "Python"]
}
```

The inserted value must be an object literal. Object keys are identifiers or
quoted strings and cannot be duplicated. Arrays and objects allow a trailing
comma.

### Update and delete

```novaql
students.update {
    skills contains "Rust"
}
| set active = true, profile.reviewed = true

students.delete {
    active == false
}
| limit 100
```

An update requires at least one `set` stage. Empty braces deliberately select
all documents, so bulk update/delete operations are visually explicit.

## Pipeline stages

`get`, `update`, and `delete` accept pipe-separated stages after their brace
predicate:

```text
| project <path>, ...
| sort <path> [asc|desc], ...
| skip <non-negative integer>
| limit <non-negative integer>
| set <path> = <expression>, ...
```

`set` is valid only for updates. The remaining stages execute from left to
right. For example:

```novaql
students.get {
    address.state == "Odisha" and skills contains "Rust"
}
| project name, address.state, skills
| sort cgpa desc
| limit 10;
```

## Collection and index DDL

Catalog operations retain command-first forms because they operate on schema
objects rather than documents:

```text
create collection <name>
drop collection <name>
create index <name> on <collection> (<path>, ...)
drop index <name>
```

Index commands parse now and execute after Phases 8–9.

## Expressions and AST

Expressions support paths, scalar/array/object literals, parentheses, unary
`not`/`!`/`-`/`+`, and left-associative binary operators. From lowest to highest
precedence, the binary groups are:

```text
or
and
== != < <= > >= contains
+ -
* / %
unary not ! - +
```

The parser produces a public typed AST with spans on queries, expressions,
paths, and object fields. Lexical and parse failures remain distinct through
`QueryError`.

## Execution semantics

Collection-scan execution is provided through `ExecutionBackend`. Stages run
left to right, expression evaluation has no implicit type coercion, integer
arithmetic is checked, boolean operators short-circuit, and missing paths are
distinct from null. `contains` requires an array on its left and tests exact
typed equality against each element.

See [`query-execution.md`](query-execution.md) for the complete runtime
contract. Index access remains unavailable until Phases 8–9, and `explain`
remains unavailable until the Phase 10 planner.
