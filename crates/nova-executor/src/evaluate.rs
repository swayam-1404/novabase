use std::cmp::Ordering;
use std::collections::BTreeMap;

use nova_core::document::Document;
use nova_core::error::{NovaError, Result};
use nova_core::nova_id::NovaId;
use nova_core::nova_timestamp::NovaTimestamp;
use nova_core::nova_value::NovaValue;
use nova_query::{BinaryOperator, Expression, ExpressionKind, Literal, Path, UnaryOperator};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RuntimeValue {
    Missing,
    Null,
    Boolean(bool),
    Int64(i64),
    Float64(f64),
    String(String),
    Array(Vec<Self>),
    Object(BTreeMap<String, Self>),
    Timestamp(NovaTimestamp),
    NovaId(NovaId),
}

pub(crate) fn evaluate(expression: &Expression, document: &Document) -> Result<RuntimeValue> {
    match &expression.kind {
        ExpressionKind::Literal(literal) => Ok(evaluate_literal(literal)),
        ExpressionKind::Path(path) => Ok(read_path(document, path)),
        ExpressionKind::Array(values) => values
            .iter()
            .map(|value| evaluate(value, document))
            .collect::<Result<Vec<_>>>()
            .map(RuntimeValue::Array),
        ExpressionKind::Object(fields) => fields
            .iter()
            .map(|field| Ok((field.name.clone(), evaluate(&field.value, document)?)))
            .collect::<Result<BTreeMap<_, _>>>()
            .map(RuntimeValue::Object),
        ExpressionKind::Unary { operator, operand } => {
            evaluate_unary(*operator, evaluate(operand, document)?)
        }
        ExpressionKind::Binary {
            left,
            operator,
            right,
        } => evaluate_binary(left, *operator, right, document),
    }
}

pub(crate) fn evaluate_predicate(expression: &Expression, document: &Document) -> Result<bool> {
    match evaluate(expression, document)? {
        RuntimeValue::Boolean(value) => Ok(value),
        other => Err(invalid(format!(
            "where expression must be boolean, got {}",
            type_name(&other)
        ))),
    }
}

pub(crate) fn into_nova_value(value: RuntimeValue) -> Result<NovaValue> {
    match value {
        RuntimeValue::Missing => Err(invalid("missing path cannot be stored as a value")),
        RuntimeValue::Null => Ok(NovaValue::Null),
        RuntimeValue::Boolean(value) => Ok(NovaValue::Boolean(value)),
        RuntimeValue::Int64(value) => Ok(NovaValue::Int64(value)),
        RuntimeValue::Float64(value) if value.is_finite() => Ok(NovaValue::Float64(value)),
        RuntimeValue::Float64(_) => Err(invalid("non-finite arithmetic result")),
        RuntimeValue::String(value) => Ok(NovaValue::String(value)),
        RuntimeValue::Array(values) => values
            .into_iter()
            .map(into_nova_value)
            .collect::<Result<Vec<_>>>()
            .map(NovaValue::Array),
        RuntimeValue::Object(fields) => {
            let mut document = Document::generated()?;
            for (name, value) in fields {
                document.insert(name, into_nova_value(value)?);
            }
            Ok(NovaValue::Document(document))
        }
        RuntimeValue::Timestamp(value) => Ok(NovaValue::Timestamp(value)),
        RuntimeValue::NovaId(value) => Ok(NovaValue::NovaId(value)),
    }
}

pub(crate) fn read_path(document: &Document, path: &Path) -> RuntimeValue {
    if path.segments.as_slice() == ["_id"] {
        return RuntimeValue::NovaId(document.id());
    }
    let parts: Vec<&str> = path.segments.iter().map(String::as_str).collect();
    document
        .lookup_parts(&parts)
        .map_or(RuntimeValue::Missing, RuntimeValue::from)
}

pub(crate) fn compare_for_sort(left: &RuntimeValue, right: &RuntimeValue) -> Result<Ordering> {
    match (left, right) {
        (RuntimeValue::Missing, RuntimeValue::Missing)
        | (RuntimeValue::Null, RuntimeValue::Null) => Ok(Ordering::Equal),
        (RuntimeValue::Missing, _) => Ok(Ordering::Less),
        (_, RuntimeValue::Missing) => Ok(Ordering::Greater),
        (RuntimeValue::Null, _) => Ok(Ordering::Less),
        (_, RuntimeValue::Null) => Ok(Ordering::Greater),
        (RuntimeValue::Boolean(left), RuntimeValue::Boolean(right)) => Ok(left.cmp(right)),
        (RuntimeValue::Int64(left), RuntimeValue::Int64(right)) => Ok(left.cmp(right)),
        (RuntimeValue::Float64(left), RuntimeValue::Float64(right)) => Ok(left.total_cmp(right)),
        (RuntimeValue::String(left), RuntimeValue::String(right)) => Ok(left.cmp(right)),
        (RuntimeValue::Timestamp(left), RuntimeValue::Timestamp(right)) => Ok(left.cmp(right)),
        (RuntimeValue::NovaId(left), RuntimeValue::NovaId(right)) => Ok(left.cmp(right)),
        (RuntimeValue::Array(_), RuntimeValue::Array(_))
        | (RuntimeValue::Object(_), RuntimeValue::Object(_)) => {
            Err(invalid("arrays and documents are not sortable"))
        }
        _ => Err(invalid(format!(
            "cannot sort unlike types {} and {}",
            type_name(left),
            type_name(right)
        ))),
    }
}

fn evaluate_literal(literal: &Literal) -> RuntimeValue {
    match literal {
        Literal::Null => RuntimeValue::Null,
        Literal::Boolean(value) => RuntimeValue::Boolean(*value),
        Literal::Integer(value) => RuntimeValue::Int64(*value),
        Literal::Float(value) => RuntimeValue::Float64(*value),
        Literal::String(value) => RuntimeValue::String(value.clone()),
    }
}

fn evaluate_unary(operator: UnaryOperator, operand: RuntimeValue) -> Result<RuntimeValue> {
    match (operator, operand) {
        (UnaryOperator::Not, RuntimeValue::Boolean(value)) => Ok(RuntimeValue::Boolean(!value)),
        (UnaryOperator::Negate, RuntimeValue::Int64(value)) => value
            .checked_neg()
            .map(RuntimeValue::Int64)
            .ok_or_else(|| invalid("integer negation overflow")),
        (UnaryOperator::Negate, RuntimeValue::Float64(value)) => finite_float(-value),
        (UnaryOperator::Positive, value @ (RuntimeValue::Int64(_) | RuntimeValue::Float64(_))) => {
            Ok(value)
        }
        (operator, value) => Err(invalid(format!(
            "operator {operator:?} does not accept {}",
            type_name(&value)
        ))),
    }
}

fn evaluate_binary(
    left_expression: &Expression,
    operator: BinaryOperator,
    right_expression: &Expression,
    document: &Document,
) -> Result<RuntimeValue> {
    let left = evaluate(left_expression, document)?;
    if operator == BinaryOperator::And {
        let RuntimeValue::Boolean(left) = left else {
            return Err(invalid("left operand of `and` must be boolean"));
        };
        if !left {
            return Ok(RuntimeValue::Boolean(false));
        }
        return boolean_operand(
            &evaluate(right_expression, document)?,
            "right operand of `and`",
        );
    }
    if operator == BinaryOperator::Or {
        let RuntimeValue::Boolean(left) = left else {
            return Err(invalid("left operand of `or` must be boolean"));
        };
        if left {
            return Ok(RuntimeValue::Boolean(true));
        }
        return boolean_operand(
            &evaluate(right_expression, document)?,
            "right operand of `or`",
        );
    }

    let right = evaluate(right_expression, document)?;
    match operator {
        BinaryOperator::Equal => Ok(RuntimeValue::Boolean(left == right)),
        BinaryOperator::NotEqual => Ok(RuntimeValue::Boolean(left != right)),
        BinaryOperator::Less => ordered(&left, &right, Ordering::is_lt),
        BinaryOperator::LessEqual => ordered(&left, &right, Ordering::is_le),
        BinaryOperator::Greater => ordered(&left, &right, Ordering::is_gt),
        BinaryOperator::GreaterEqual => ordered(&left, &right, Ordering::is_ge),
        BinaryOperator::Add
        | BinaryOperator::Subtract
        | BinaryOperator::Multiply
        | BinaryOperator::Divide
        | BinaryOperator::Remainder => arithmetic(left, operator, right),
        BinaryOperator::And | BinaryOperator::Or => unreachable!("handled with short circuit"),
    }
}

fn boolean_operand(value: &RuntimeValue, label: &str) -> Result<RuntimeValue> {
    match value {
        RuntimeValue::Boolean(value) => Ok(RuntimeValue::Boolean(*value)),
        _ => Err(invalid(format!("{label} must be boolean"))),
    }
}

fn ordered(
    left: &RuntimeValue,
    right: &RuntimeValue,
    predicate: impl FnOnce(Ordering) -> bool,
) -> Result<RuntimeValue> {
    let ordering = match (left, right) {
        (RuntimeValue::Int64(left), RuntimeValue::Int64(right)) => left.cmp(right),
        (RuntimeValue::Float64(left), RuntimeValue::Float64(right)) => left.total_cmp(right),
        (RuntimeValue::String(left), RuntimeValue::String(right)) => left.cmp(right),
        (RuntimeValue::Timestamp(left), RuntimeValue::Timestamp(right)) => left.cmp(right),
        (RuntimeValue::NovaId(left), RuntimeValue::NovaId(right)) => left.cmp(right),
        _ => {
            return Err(invalid(format!(
                "cannot order {} and {}",
                type_name(left),
                type_name(right)
            )));
        }
    };
    Ok(RuntimeValue::Boolean(predicate(ordering)))
}

fn arithmetic(
    left: RuntimeValue,
    operator: BinaryOperator,
    right: RuntimeValue,
) -> Result<RuntimeValue> {
    match (left, right) {
        (RuntimeValue::Int64(left), RuntimeValue::Int64(right)) => {
            let value = match operator {
                BinaryOperator::Add => left.checked_add(right),
                BinaryOperator::Subtract => left.checked_sub(right),
                BinaryOperator::Multiply => left.checked_mul(right),
                BinaryOperator::Divide => left.checked_div(right),
                BinaryOperator::Remainder => left.checked_rem(right),
                _ => unreachable!("arithmetic called with non-arithmetic operator"),
            };
            value
                .map(RuntimeValue::Int64)
                .ok_or_else(|| invalid("integer arithmetic overflow or division by zero"))
        }
        (RuntimeValue::Float64(left), RuntimeValue::Float64(right)) => {
            if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder)
                && right == 0.0
            {
                return Err(invalid("floating-point division by zero"));
            }
            let value = match operator {
                BinaryOperator::Add => left + right,
                BinaryOperator::Subtract => left - right,
                BinaryOperator::Multiply => left * right,
                BinaryOperator::Divide => left / right,
                BinaryOperator::Remainder => left % right,
                _ => unreachable!("arithmetic called with non-arithmetic operator"),
            };
            finite_float(value)
        }
        (left, right) => Err(invalid(format!(
            "arithmetic requires matching numeric types, got {} and {}",
            type_name(&left),
            type_name(&right)
        ))),
    }
}

fn finite_float(value: f64) -> Result<RuntimeValue> {
    if value.is_finite() {
        Ok(RuntimeValue::Float64(value))
    } else {
        Err(invalid("non-finite arithmetic result"))
    }
}

fn type_name(value: &RuntimeValue) -> &'static str {
    match value {
        RuntimeValue::Missing => "missing",
        RuntimeValue::Null => "null",
        RuntimeValue::Boolean(_) => "boolean",
        RuntimeValue::Int64(_) => "int64",
        RuntimeValue::Float64(_) => "float64",
        RuntimeValue::String(_) => "string",
        RuntimeValue::Array(_) => "array",
        RuntimeValue::Object(_) => "document",
        RuntimeValue::Timestamp(_) => "timestamp",
        RuntimeValue::NovaId(_) => "nova_id",
    }
}

fn invalid(message: impl Into<String>) -> NovaError {
    NovaError::InvalidArgument(format!("query execution: {}", message.into()))
}

impl From<&NovaValue> for RuntimeValue {
    fn from(value: &NovaValue) -> Self {
        match value {
            NovaValue::Null => Self::Null,
            NovaValue::Boolean(value) => Self::Boolean(*value),
            NovaValue::Int64(value) => Self::Int64(*value),
            NovaValue::Float64(value) => Self::Float64(*value),
            NovaValue::String(value) => Self::String(value.clone()),
            NovaValue::Array(values) => Self::Array(values.iter().map(Self::from).collect()),
            NovaValue::Document(document) => Self::Object(
                document
                    .iter()
                    .map(|(key, value)| (key.to_owned(), Self::from(value)))
                    .collect(),
            ),
            NovaValue::Timestamp(value) => Self::Timestamp(*value),
            NovaValue::NovaId(value) => Self::NovaId(*value),
        }
    }
}
