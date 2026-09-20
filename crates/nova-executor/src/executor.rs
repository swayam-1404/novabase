use std::cmp::Ordering;

use nova_core::document::Document;
use nova_core::error::{NovaError, Result};
use nova_core::nova_id::NovaId;
use nova_core::nova_value::NovaValue;
use nova_query::{Assignment, Command, ExpressionKind, Path, Query, SortDirection, SortKey, Stage};

use crate::backend::ExecutionBackend;
use crate::evaluate::{
    compare_for_sort, evaluate, evaluate_predicate, into_nova_value, read_path, RuntimeValue,
};

/// Result of executing one `NovaQL` query.
#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionResult {
    /// Documents produced by a `find` pipeline.
    Documents(Vec<Document>),
    /// Identifier generated for an inserted document.
    Inserted(NovaId),
    /// Number of replaced documents.
    Updated(usize),
    /// Number of deleted documents.
    Deleted(usize),
    /// Collection created by the command.
    CollectionCreated(String),
    /// Collection dropped by the command.
    CollectionDropped(String),
}

/// Executes a parsed query against a collection backend.
///
/// Phase 7 uses deterministic collection scans. Index DDL/execution and
/// `explain` require later index/planner phases and return
/// [`NovaError::Unsupported`] until those phases land.
///
/// # Errors
///
/// Returns typed backend errors or [`NovaError::InvalidArgument`] for invalid
/// runtime types, arithmetic, unsupported stage/command combinations, or
/// attempts to store a missing value.
pub fn execute(query: &Query, backend: &mut impl ExecutionBackend) -> Result<ExecutionResult> {
    if query.explain {
        return Err(NovaError::Unsupported(
            "query plans are introduced in Phase 10".to_owned(),
        ));
    }

    match &query.command {
        Command::Find { collection } => {
            validate_stages(&query.stages, CommandKind::Find)?;
            let documents = run_pipeline(backend.scan(collection)?, &query.stages)?;
            Ok(ExecutionResult::Documents(documents))
        }
        Command::Insert {
            collection,
            document,
        } => {
            let ExpressionKind::Object(_) = document.kind else {
                return Err(invalid("insert requires an object expression"));
            };
            let context = Document::new(NovaId::new(0, [0; 8]));
            let RuntimeValue::Object(fields) = evaluate(document, &context)? else {
                return Err(invalid("insert requires an object expression"));
            };
            let mut inserted = Document::generated()?;
            for (name, value) in fields {
                inserted.insert(name, into_nova_value(value)?);
            }
            backend
                .insert(collection, inserted)
                .map(ExecutionResult::Inserted)
        }
        Command::Update { collection } => {
            validate_stages(&query.stages, CommandKind::Update)?;
            let documents = run_pipeline(backend.scan(collection)?, &query.stages)?;
            let count = documents.len();
            for document in documents {
                backend.replace(collection, document)?;
            }
            Ok(ExecutionResult::Updated(count))
        }
        Command::Delete { collection } => {
            validate_stages(&query.stages, CommandKind::Delete)?;
            let documents = run_pipeline(backend.scan(collection)?, &query.stages)?;
            let count = documents.len();
            for document in documents {
                backend.delete(collection, document.id())?;
            }
            Ok(ExecutionResult::Deleted(count))
        }
        Command::CreateCollection { name } => {
            backend.create_collection(name)?;
            Ok(ExecutionResult::CollectionCreated(name.clone()))
        }
        Command::DropCollection { name } => {
            backend.drop_collection(name)?;
            Ok(ExecutionResult::CollectionDropped(name.clone()))
        }
        Command::CreateIndex { .. } | Command::DropIndex { .. } => Err(NovaError::Unsupported(
            "index commands are introduced in Phases 8 and 9".to_owned(),
        )),
    }
}

#[derive(Clone, Copy)]
enum CommandKind {
    Find,
    Update,
    Delete,
}

fn validate_stages(stages: &[Stage], command: CommandKind) -> Result<()> {
    for stage in stages {
        let valid = match command {
            CommandKind::Find => !matches!(stage, Stage::Set(_)),
            CommandKind::Update => !matches!(stage, Stage::Project(_)),
            CommandKind::Delete => !matches!(stage, Stage::Project(_) | Stage::Set(_)),
        };
        if !valid {
            return Err(invalid("pipeline stage is not valid for this command"));
        }
    }
    Ok(())
}

fn run_pipeline(mut documents: Vec<Document>, stages: &[Stage]) -> Result<Vec<Document>> {
    for stage in stages {
        match stage {
            Stage::Where(predicate) => {
                let mut filtered = Vec::with_capacity(documents.len());
                for document in documents {
                    if evaluate_predicate(predicate, &document)? {
                        filtered.push(document);
                    }
                }
                documents = filtered;
            }
            Stage::Project(paths) => {
                documents = documents
                    .iter()
                    .map(|document| project(document, paths))
                    .collect::<Result<Vec<_>>>()?;
            }
            Stage::Sort(keys) => sort_documents(&mut documents, keys)?,
            Stage::Limit(count) => {
                let count = usize::try_from(*count).unwrap_or(usize::MAX);
                documents.truncate(count);
            }
            Stage::Skip(count) => {
                let count = usize::try_from(*count).unwrap_or(usize::MAX);
                documents = documents.into_iter().skip(count).collect();
            }
            Stage::Set(assignments) => {
                for document in &mut documents {
                    apply_assignments(document, assignments)?;
                }
            }
        }
    }
    Ok(documents)
}

fn sort_documents(documents: &mut [Document], keys: &[SortKey]) -> Result<()> {
    let decorated: Vec<Vec<RuntimeValue>> = documents
        .iter()
        .map(|document| {
            keys.iter()
                .map(|key| read_path(document, &key.path))
                .collect()
        })
        .collect();

    for key_index in 0..keys.len() {
        let values: Vec<&RuntimeValue> =
            decorated.iter().map(|values| &values[key_index]).collect();
        validate_sort_values(&values)?;
    }

    let mut order: Vec<usize> = (0..documents.len()).collect();
    order.sort_by(|left_index, right_index| {
        for (key_index, key) in keys.iter().enumerate() {
            let ordering = compare_for_sort(
                &decorated[*left_index][key_index],
                &decorated[*right_index][key_index],
            )
            .unwrap_or(Ordering::Equal);
            let ordering = match key.direction {
                SortDirection::Ascending => ordering,
                SortDirection::Descending => ordering.reverse(),
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    });

    let original = documents.to_vec();
    for (destination, source) in order.into_iter().enumerate() {
        documents[destination] = original[source].clone();
    }
    Ok(())
}

fn validate_sort_values(values: &[&RuntimeValue]) -> Result<()> {
    for value in values {
        compare_for_sort(value, value)?;
    }
    if let Some(reference) = values
        .iter()
        .copied()
        .find(|value| !matches!(value, RuntimeValue::Missing | RuntimeValue::Null))
    {
        for value in values {
            if !matches!(value, RuntimeValue::Missing | RuntimeValue::Null) {
                compare_for_sort(reference, value)?;
            }
        }
    }
    Ok(())
}

fn apply_assignments(document: &mut Document, assignments: &[Assignment]) -> Result<()> {
    for assignment in assignments {
        let value = into_nova_value(evaluate(&assignment.value, document)?)?;
        set_path(document, &assignment.path, value)?;
    }
    Ok(())
}

fn set_path(document: &mut Document, path: &Path, value: NovaValue) -> Result<()> {
    if path
        .segments
        .first()
        .is_some_and(|segment| segment == "_id")
    {
        return Err(invalid("the implicit `_id` field cannot be updated"));
    }
    set_segments(document, &path.segments, value)
}

fn set_segments(document: &mut Document, segments: &[String], value: NovaValue) -> Result<()> {
    let Some((first, rest)) = segments.split_first() else {
        return Err(invalid("field path cannot be empty"));
    };
    if rest.is_empty() {
        document.insert(first.clone(), value);
        return Ok(());
    }
    if !document.contains_key(first) {
        document.insert(first.clone(), NovaValue::Document(Document::generated()?));
    }
    let nested = document
        .get_mut(first)
        .and_then(|value| match value {
            NovaValue::Document(document) => Some(document),
            _ => None,
        })
        .ok_or_else(|| invalid(format!("path segment {first:?} is not a document")))?;
    set_segments(nested, rest, value)
}

fn project(source: &Document, paths: &[Path]) -> Result<Document> {
    let mut projected = Document::new(source.id());
    for path in paths {
        if path.segments.as_slice() == ["_id"] {
            continue;
        }
        project_segments(source, &mut projected, &path.segments)?;
    }
    Ok(projected)
}

fn project_segments(source: &Document, target: &mut Document, segments: &[String]) -> Result<()> {
    let Some((first, rest)) = segments.split_first() else {
        return Ok(());
    };
    let Some(source_value) = source.get(first) else {
        return Ok(());
    };
    if rest.is_empty() {
        target.insert(first.clone(), source_value.clone());
        return Ok(());
    }
    let NovaValue::Document(source_nested) = source_value else {
        return Ok(());
    };
    if !target.contains_key(first) {
        target.insert(
            first.clone(),
            NovaValue::Document(Document::new(source_nested.id())),
        );
    }
    let target_nested = target
        .get_mut(first)
        .and_then(|value| match value {
            NovaValue::Document(document) => Some(document),
            _ => None,
        })
        .ok_or_else(|| invalid("projection path conflicts with a scalar field"))?;
    project_segments(source_nested, target_nested, rest)
}

fn invalid(message: impl Into<String>) -> NovaError {
    NovaError::InvalidArgument(format!("query execution: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use nova_core::nova_timestamp::NovaTimestamp;
    use nova_query::parse;

    use super::*;
    use crate::MemoryBackend;

    fn id(seed: u8) -> NovaId {
        NovaId::new(u64::from(seed), [seed; 8])
    }

    fn student(seed: u8, name: &str, score: i64, active: bool, city: &str) -> Document {
        let mut address = Document::new(id(seed + 20));
        address.insert("city", city.into());
        address.insert("country", "India".into());

        let mut document = Document::new(id(seed));
        document.insert("name", name.into());
        document.insert("score", score.into());
        document.insert("active", active.into());
        document.insert("address", address.into());
        document.insert(
            "created",
            NovaTimestamp::from_millis(i64::from(seed)).into(),
        );
        document
    }

    fn execute_source(source: &str, backend: &mut MemoryBackend) -> Result<ExecutionResult> {
        execute(&parse(source).unwrap(), backend)
    }

    fn seeded_backend() -> MemoryBackend {
        let mut backend = MemoryBackend::new();
        backend.create_collection("students").unwrap();
        for document in [
            student(1, "Ada", 91, true, "Pune"),
            student(2, "Bob", 72, false, "Delhi"),
            student(3, "Chandra", 88, true, "Pune"),
            student(4, "Devi", 95, true, "Kolkata"),
        ] {
            backend.insert("students", document).unwrap();
        }
        backend
    }

    #[test]
    fn find_filters_sorts_projects_skips_and_limits() {
        let mut backend = seeded_backend();
        let result = execute_source(
            "find students | where active == true and score >= 80 | sort by score desc | skip 1 | limit 1 | project name, address.city",
            &mut backend,
        )
        .unwrap();
        let ExecutionResult::Documents(documents) = result else {
            panic!("expected documents")
        };
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].get("name"), Some(&NovaValue::from("Ada")));
        assert_eq!(
            documents[0].lookup("address.city"),
            Some(&NovaValue::from("Pune"))
        );
        assert!(documents[0].lookup("address.country").is_none());
        assert!(documents[0].get("score").is_none());
    }

    #[test]
    fn insert_update_delete_and_collection_ddl_execute() {
        let mut backend = MemoryBackend::new();
        assert_eq!(
            execute_source("create collection users", &mut backend).unwrap(),
            ExecutionResult::CollectionCreated("users".to_owned())
        );
        let inserted = execute_source(
            r#"insert into users {name: "Ada", score: 40, profile: {city: "Pune"}}"#,
            &mut backend,
        )
        .unwrap();
        assert!(matches!(inserted, ExecutionResult::Inserted(_)));

        assert_eq!(
            execute_source(
                "update users | where name == \"Ada\" | set score = score + 2, profile.active = true",
                &mut backend,
            )
            .unwrap(),
            ExecutionResult::Updated(1)
        );
        let ExecutionResult::Documents(documents) =
            execute_source("find users", &mut backend).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(documents[0].get("score"), Some(&NovaValue::Int64(42)));
        assert_eq!(
            documents[0].lookup("profile.active"),
            Some(&NovaValue::Boolean(true))
        );

        assert_eq!(
            execute_source("delete from users | where score == 42", &mut backend).unwrap(),
            ExecutionResult::Deleted(1)
        );
        assert_eq!(backend.collection_len("users"), Some(0));
        assert_eq!(
            execute_source("drop collection users", &mut backend).unwrap(),
            ExecutionResult::CollectionDropped("users".to_owned())
        );
    }

    #[test]
    fn missing_null_and_id_semantics_are_explicit() {
        let mut backend = seeded_backend();
        let ExecutionResult::Documents(missing) = execute_source(
            "find students | where missing != null | limit 1",
            &mut backend,
        )
        .unwrap() else {
            unreachable!()
        };
        assert_eq!(missing.len(), 1);

        let ExecutionResult::Documents(ids) =
            execute_source("find students | where _id == _id", &mut backend).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(ids.len(), 4);
        let query = format!("find students | where _id == \"{}\"", id(1));
        assert_eq!(
            execute_source(&query, &mut backend).unwrap(),
            ExecutionResult::Documents(Vec::new())
        );
    }

    #[test]
    fn runtime_type_errors_do_not_panic() {
        let cases = [
            "find students | where score",
            "find students | where score + 1.0 > 2",
            "find students | where score / 0 > 1",
            "find students | sort by address",
            "update students | set _id = 1",
            "update students | set address.city.name = \"x\"",
        ];
        for source in cases {
            let mut backend = seeded_backend();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute_source(source, &mut backend)
            }));
            assert!(result.is_ok(), "executor panicked for {source:?}");
            assert!(result.unwrap().is_err(), "executor accepted {source:?}");
        }
    }

    #[test]
    fn unsupported_future_features_are_typed_errors() {
        let mut backend = seeded_backend();
        assert!(matches!(
            execute_source("explain find students", &mut backend),
            Err(NovaError::Unsupported(_))
        ));
        assert!(matches!(
            execute_source("create index by_score on students (score)", &mut backend),
            Err(NovaError::Unsupported(_))
        ));
    }

    #[test]
    fn duplicate_insert_does_not_overwrite_memory_backend() {
        let mut backend = MemoryBackend::new();
        backend.create_collection("students").unwrap();
        let original = student(1, "Ada", 90, true, "Pune");
        let mut duplicate = student(1, "Changed", 0, false, "Delhi");
        duplicate.set_id(original.id());
        backend.insert("students", original.clone()).unwrap();
        assert!(matches!(
            backend.create_collection("students"),
            Err(NovaError::AlreadyExists(_))
        ));
        assert!(matches!(
            backend.insert("students", duplicate),
            Err(NovaError::AlreadyExists(_))
        ));
        assert_eq!(backend.scan("students").unwrap(), vec![original]);
    }
}
