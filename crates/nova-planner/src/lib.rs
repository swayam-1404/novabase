//! Deterministic `NovaQL` query planning and explain output.

#![forbid(unsafe_code)]

use std::fmt;

use nova_index::{IndexDefinition, IndexKey};
use nova_query::{BinaryOperator, Command, Expression, ExpressionKind, Literal, Query, Stage};

/// Physical access path selected for a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessPath {
    /// Command does not read a collection.
    Command,
    /// Deterministic full collection scan.
    CollectionScan {
        /// Scanned collection.
        collection: String,
    },
    /// Exact-key lookup through a persistent index.
    IndexScan {
        /// Indexed collection.
        collection: String,
        /// Selected index name.
        index: String,
        /// Exact lookup key.
        key: IndexKey,
    },
}

/// Inspectable physical plan for one parsed query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryPlan {
    /// Selected physical access path.
    pub access: AccessPath,
    /// Pipeline operator names in execution order.
    pub operators: Vec<&'static str>,
}

impl fmt::Display for QueryPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.access {
            AccessPath::Command => formatter.write_str("Command")?,
            AccessPath::CollectionScan { collection } => {
                write!(formatter, "CollectionScan({collection})")?;
            }
            AccessPath::IndexScan {
                collection,
                index,
                key,
            } => write!(formatter, "IndexScan({collection}, {index}, {key:?})")?,
        }
        for operator in &self.operators {
            write!(formatter, " -> {operator}")?;
        }
        Ok(())
    }
}

/// Produces a deterministic plan from a query and available index definitions.
///
/// The first definition in name order matching an equality term is selected.
/// Predicates remain in the operator pipeline as residual checks, so index
/// selection cannot alter query correctness.
#[must_use]
pub fn plan(query: &Query, definitions: &[IndexDefinition]) -> QueryPlan {
    let collection = command_collection(&query.command);
    let access = collection.map_or(AccessPath::Command, |collection| {
        find_index_scan(collection, &query.stages, definitions).map_or_else(
            || AccessPath::CollectionScan {
                collection: collection.to_owned(),
            },
            |(index, key)| AccessPath::IndexScan {
                collection: collection.to_owned(),
                index,
                key,
            },
        )
    });
    QueryPlan {
        access,
        operators: query.stages.iter().map(stage_name).collect(),
    }
}

fn command_collection(command: &Command) -> Option<&str> {
    match command {
        Command::Get { collection }
        | Command::Update { collection }
        | Command::Delete { collection } => Some(collection),
        _ => None,
    }
}

fn find_index_scan(
    collection: &str,
    stages: &[Stage],
    definitions: &[IndexDefinition],
) -> Option<(String, IndexKey)> {
    let predicate = stages.iter().find_map(|stage| match stage {
        Stage::Filter(expression) => Some(expression),
        _ => None,
    })?;
    let mut candidates: Vec<&IndexDefinition> = definitions
        .iter()
        .filter(|definition| definition.collection == collection)
        .collect();
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    candidates.into_iter().find_map(|definition| {
        equality_key(predicate, &definition.field).map(|key| (definition.name.clone(), key))
    })
}

fn equality_key(expression: &Expression, field: &[String]) -> Option<IndexKey> {
    match &expression.kind {
        ExpressionKind::Binary {
            left,
            operator: BinaryOperator::And,
            right,
        } => equality_key(left, field).or_else(|| equality_key(right, field)),
        ExpressionKind::Binary {
            left,
            operator: BinaryOperator::Equal,
            right,
        } => path_literal(left, right, field).or_else(|| path_literal(right, left, field)),
        _ => None,
    }
}

fn path_literal(path: &Expression, literal: &Expression, field: &[String]) -> Option<IndexKey> {
    let ExpressionKind::Path(path) = &path.kind else {
        return None;
    };
    if path.segments != field {
        return None;
    }
    let ExpressionKind::Literal(literal) = &literal.kind else {
        return None;
    };
    match literal {
        Literal::Integer(value) => Some(IndexKey::Int64(*value)),
        Literal::Float(value) => Some(IndexKey::float64(*value)),
        Literal::String(value) => Some(IndexKey::String(value.clone())),
        Literal::Null | Literal::Boolean(_) => None,
    }
}

const fn stage_name(stage: &Stage) -> &'static str {
    match stage {
        Stage::Filter(_) => "Filter",
        Stage::Project(_) => "Project",
        Stage::Sort(_) => "Sort",
        Stage::Limit(_) => "Limit",
        Stage::Skip(_) => "Skip",
        Stage::Set(_) => "Set",
    }
}

#[cfg(test)]
mod tests {
    use nova_query::parse;

    use super::*;

    fn definition(name: &str, field: &[&str]) -> IndexDefinition {
        IndexDefinition {
            name: name.to_owned(),
            collection: "students".to_owned(),
            field: field.iter().map(|segment| (*segment).to_owned()).collect(),
        }
    }

    #[test]
    fn equality_and_conjunction_choose_deterministic_index() {
        let query =
            parse("students.get { active == true and profile.score == 90 } | sort name | limit 2")
                .unwrap();
        let result = plan(
            &query,
            &[
                definition("z_score", &["profile", "score"]),
                definition("a_score", &["profile", "score"]),
            ],
        );
        assert_eq!(
            result.access,
            AccessPath::IndexScan {
                collection: "students".to_owned(),
                index: "a_score".to_owned(),
                key: IndexKey::Int64(90),
            }
        );
        assert_eq!(result.operators, ["Filter", "Sort", "Limit"]);
    }

    #[test]
    fn unsupported_predicates_and_types_fall_back_to_scan() {
        for source in [
            "students.get { score > 90 }",
            "students.get { active == true }",
            "students.get { score == other }",
        ] {
            let query = parse(source).unwrap();
            assert!(matches!(
                plan(&query, &[definition("by_score", &["score"])]).access,
                AccessPath::CollectionScan { .. }
            ));
        }
    }

    #[test]
    fn explain_text_is_stable() {
        let query = parse("students.get { score == 90 }").unwrap();
        assert_eq!(
            plan(&query, &[definition("by_score", &["score"])]).to_string(),
            "IndexScan(students, by_score, Int64(90)) -> Filter"
        );
    }
}
