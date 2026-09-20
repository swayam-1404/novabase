use crate::Span;

/// A complete parsed `NovaQL` query.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// Whether the command was prefixed with `explain`.
    pub explain: bool,
    /// The command that begins the query.
    pub command: Command,
    /// Ordered pipeline stages following the command.
    pub stages: Vec<Stage>,
    /// Source range occupied by the complete query.
    pub span: Span,
}

/// Top-level `NovaQL` command.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Reads documents from a collection.
    Find {
        /// Collection name.
        collection: String,
    },
    /// Inserts one document literal into a collection.
    Insert {
        /// Collection name.
        collection: String,
        /// Object expression representing the document fields.
        document: Expression,
    },
    /// Updates documents selected by optional pipeline stages.
    Update {
        /// Collection name.
        collection: String,
    },
    /// Deletes documents selected by optional pipeline stages.
    Delete {
        /// Collection name.
        collection: String,
    },
    /// Creates a collection.
    CreateCollection {
        /// Collection name.
        name: String,
    },
    /// Drops a collection.
    DropCollection {
        /// Collection name.
        name: String,
    },
    /// Creates a named index over one or more document paths.
    CreateIndex {
        /// Index name.
        name: String,
        /// Indexed collection.
        collection: String,
        /// Indexed paths in declaration order.
        fields: Vec<Path>,
    },
    /// Drops a named index.
    DropIndex {
        /// Index name.
        name: String,
    },
}

impl Command {
    pub(crate) const fn accepts_pipeline(&self) -> bool {
        matches!(
            self,
            Self::Find { .. } | Self::Update { .. } | Self::Delete { .. }
        )
    }
}

/// One pipeline transformation.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// Filters documents by a boolean expression.
    Where(Expression),
    /// Retains the listed document paths.
    Project(Vec<Path>),
    /// Orders documents by one or more keys.
    Sort(Vec<SortKey>),
    /// Caps the number of returned documents.
    Limit(u64),
    /// Skips a number of documents.
    Skip(u64),
    /// Assigns expressions to document paths.
    Set(Vec<Assignment>),
}

/// One sort path and direction.
#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    /// Document path to sort by.
    pub path: Path,
    /// Ordering direction.
    pub direction: SortDirection,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    /// Ascending, the default.
    Ascending,
    /// Descending.
    Descending,
}

/// One update assignment.
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    /// Destination document path.
    pub path: Path,
    /// Value expression.
    pub value: Expression,
}

/// A dotted document field path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    /// Individual path segments.
    pub segments: Vec<String>,
    /// Full path source range.
    pub span: Span,
}

/// A spanned expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    /// Expression payload.
    pub kind: ExpressionKind,
    /// Full expression source range.
    pub span: Span,
}

/// Expression payload variants.
#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionKind {
    /// Scalar literal.
    Literal(Literal),
    /// Dotted document path.
    Path(Path),
    /// Ordered array literal.
    Array(Vec<Expression>),
    /// Ordered object fields.
    Object(Vec<ObjectField>),
    /// Prefix unary operation.
    Unary {
        /// Operator.
        operator: UnaryOperator,
        /// Operand.
        operand: Box<Expression>,
    },
    /// Infix binary operation.
    Binary {
        /// Left operand.
        left: Box<Expression>,
        /// Operator.
        operator: BinaryOperator,
        /// Right operand.
        right: Box<Expression>,
    },
}

/// Scalar literal value.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// Null.
    Null,
    /// Boolean.
    Boolean(bool),
    /// Signed integer.
    Integer(i64),
    /// Finite float.
    Float(f64),
    /// UTF-8 string.
    String(String),
}

/// One object-literal field.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectField {
    /// Field name.
    pub name: String,
    /// Field value.
    pub value: Expression,
    /// Complete `name: value` source range.
    pub span: Span,
}

/// Prefix unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    /// Logical negation (`not` or `!`).
    Not,
    /// Numeric negation.
    Negate,
    /// Numeric identity.
    Positive,
}

/// Infix binary operator in increasing groups of precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    /// Logical disjunction.
    Or,
    /// Logical conjunction.
    And,
    /// Equality.
    Equal,
    /// Inequality.
    NotEqual,
    /// Less than.
    Less,
    /// Less than or equal.
    LessEqual,
    /// Greater than.
    Greater,
    /// Greater than or equal.
    GreaterEqual,
    /// Addition.
    Add,
    /// Subtraction.
    Subtract,
    /// Multiplication.
    Multiply,
    /// Division.
    Divide,
    /// Remainder.
    Remainder,
}
