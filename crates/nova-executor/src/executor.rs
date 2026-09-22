use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::time::Instant;

use nova_core::document::Document;
use nova_core::error::{NovaError, Result};
use nova_core::nova_id::NovaId;
use nova_core::nova_value::NovaValue;
use nova_intelligence::{TelemetryAccess, TelemetryEvent, TelemetrySink};
use nova_planner::{plan, AccessPath, QueryPlan};
use nova_query::{
    Assignment, BinaryOperator, Command, Expression, ExpressionKind, Literal, Path, Query,
    SortDirection, SortKey, Stage,
};

use crate::backend::ExecutionBackend;
use crate::evaluate::{
    compare_for_sort, evaluate, evaluate_predicate, into_nova_value, read_path, RuntimeValue,
};

/// Result of executing one `NovaQL` query.
#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionResult {
    /// Documents produced by a `get` pipeline.
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
    /// Index created and backfilled by the command.
    IndexCreated(String),
    /// Index dropped by the command.
    IndexDropped(String),
    /// Stable physical plan description produced by `explain`.
    Explained(String),
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
    execute_internal(query, backend, None)
}

/// Executes a query while emitting one best-effort, literal-free observation.
///
/// # Errors
/// Returns the same typed errors as [`execute`]. Telemetry recording cannot
/// change the returned result.
pub fn execute_with_telemetry(
    query: &Query,
    backend: &mut impl ExecutionBackend,
    sink: &dyn TelemetrySink,
) -> Result<ExecutionResult> {
    execute_internal(query, backend, Some(sink))
}

fn execute_internal(
    query: &Query,
    backend: &mut impl ExecutionBackend,
    sink: Option<&dyn TelemetrySink>,
) -> Result<ExecutionResult> {
    let started = Instant::now();
    let physical_plan = plan(query, &backend.index_definitions());
    let mut examined = 0;
    let result = execute_planned(query, backend, &physical_plan, &mut examined);
    if let Some(sink) = sink {
        sink.record(build_event(
            query,
            &physical_plan,
            examined,
            &result,
            started.elapsed().as_micros(),
        ));
    }
    result
}

fn execute_planned(
    query: &Query,
    backend: &mut impl ExecutionBackend,
    physical_plan: &QueryPlan,
    examined: &mut usize,
) -> Result<ExecutionResult> {
    if query.explain {
        return Ok(ExecutionResult::Explained(physical_plan.to_string()));
    }
    match &query.command {
        Command::Get { collection } => {
            validate_stages(&query.stages, CommandKind::Get)?;
            let documents = run_pipeline(
                access_documents(backend, physical_plan, collection, examined)?,
                &query.stages,
            )?;
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
            let documents = run_pipeline(
                access_documents(backend, physical_plan, collection, examined)?,
                &query.stages,
            )?;
            let count = documents.len();
            for document in documents {
                backend.replace(collection, document)?;
            }
            Ok(ExecutionResult::Updated(count))
        }
        Command::Delete { collection } => {
            validate_stages(&query.stages, CommandKind::Delete)?;
            let documents = run_pipeline(
                access_documents(backend, physical_plan, collection, examined)?,
                &query.stages,
            )?;
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
        Command::CreateIndex {
            name,
            collection,
            fields,
        } => {
            let [field] = fields.as_slice() else {
                return Err(NovaError::Unsupported(
                    "composite index keys are not defined in index format v1".to_owned(),
                ));
            };
            backend.create_index(name, collection, &field.segments)?;
            Ok(ExecutionResult::IndexCreated(name.clone()))
        }
        Command::DropIndex { name } => {
            backend.drop_index(name)?;
            Ok(ExecutionResult::IndexDropped(name.clone()))
        }
    }
}

fn access_documents(
    backend: &mut impl ExecutionBackend,
    physical_plan: &QueryPlan,
    collection: &str,
    examined: &mut usize,
) -> Result<Vec<Document>> {
    let documents = match &physical_plan.access {
        AccessPath::IndexScan { index, key, .. } => backend.scan_index(collection, index, key),
        AccessPath::CollectionScan { .. } => backend.scan(collection),
        AccessPath::Command => Err(NovaError::Internal(
            "data command received command-only plan".to_owned(),
        )),
    }?;
    *examined = documents.len();
    Ok(documents)
}

fn build_event(
    query: &Query,
    physical_plan: &QueryPlan,
    examined: usize,
    result: &Result<ExecutionResult>,
    elapsed_micros: u128,
) -> TelemetryEvent {
    let returned = result.as_ref().map_or(0, result_count);
    let access = match &physical_plan.access {
        AccessPath::Command => TelemetryAccess::Command,
        AccessPath::CollectionScan { .. } => TelemetryAccess::CollectionScan,
        AccessPath::IndexScan { index, .. } => TelemetryAccess::IndexScan {
            index: index.clone(),
        },
    };
    TelemetryEvent {
        fingerprint: fingerprint(query),
        collection: command_collection(&query.command).map(str::to_owned),
        predicate_paths: predicate_paths(&query.stages),
        index_candidate_paths: index_candidate_paths(&query.stages),
        access,
        examined,
        returned,
        elapsed_micros: u64::try_from(elapsed_micros).unwrap_or(u64::MAX),
        succeeded: result.is_ok(),
    }
}

fn result_count(result: &ExecutionResult) -> usize {
    match result {
        ExecutionResult::Documents(documents) => documents.len(),
        ExecutionResult::Inserted(_) => 1,
        ExecutionResult::Updated(count) | ExecutionResult::Deleted(count) => *count,
        ExecutionResult::CollectionCreated(_)
        | ExecutionResult::CollectionDropped(_)
        | ExecutionResult::IndexCreated(_)
        | ExecutionResult::IndexDropped(_)
        | ExecutionResult::Explained(_) => 0,
    }
}

fn fingerprint(query: &Query) -> String {
    let command = match &query.command {
        Command::Get { collection } => format!("get:{collection}"),
        Command::Insert { collection, .. } => format!("insert:{collection}"),
        Command::Update { collection } => format!("update:{collection}"),
        Command::Delete { collection } => format!("delete:{collection}"),
        Command::CreateCollection { .. } => "create_collection".to_owned(),
        Command::DropCollection { .. } => "drop_collection".to_owned(),
        Command::CreateIndex { .. } => "create_index".to_owned(),
        Command::DropIndex { .. } => "drop_index".to_owned(),
    };
    query.stages.iter().fold(command, |mut value, stage| {
        let name = match stage {
            Stage::Filter(_) => "filter",
            Stage::Project(_) => "project",
            Stage::Sort(_) => "sort",
            Stage::Limit(_) => "limit",
            Stage::Skip(_) => "skip",
            Stage::Set(_) => "set",
        };
        value.push('|');
        value.push_str(name);
        value
    })
}

fn command_collection(command: &Command) -> Option<&str> {
    match command {
        Command::Get { collection }
        | Command::Insert { collection, .. }
        | Command::Update { collection }
        | Command::Delete { collection }
        | Command::CreateIndex { collection, .. } => Some(collection),
        Command::CreateCollection { .. }
        | Command::DropCollection { .. }
        | Command::DropIndex { .. } => None,
    }
}

fn predicate_paths(stages: &[Stage]) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for expression in stages.iter().filter_map(|stage| match stage {
        Stage::Filter(expression) => Some(expression),
        _ => None,
    }) {
        collect_paths(expression, &mut paths);
    }
    paths.into_iter().collect()
}

fn index_candidate_paths(stages: &[Stage]) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for expression in stages.iter().filter_map(|stage| match stage {
        Stage::Filter(expression) => Some(expression),
        _ => None,
    }) {
        collect_index_candidates(expression, &mut paths);
    }
    paths.into_iter().collect()
}

fn collect_index_candidates(expression: &Expression, paths: &mut BTreeSet<String>) {
    let ExpressionKind::Binary {
        left,
        operator,
        right,
    } = &expression.kind
    else {
        return;
    };
    if *operator == BinaryOperator::And {
        collect_index_candidates(left, paths);
        collect_index_candidates(right, paths);
    } else if *operator == BinaryOperator::Equal {
        if let Some(path) =
            indexable_path_literal(left, right).or_else(|| indexable_path_literal(right, left))
        {
            paths.insert(path);
        }
    }
}

fn indexable_path_literal(path: &Expression, literal: &Expression) -> Option<String> {
    let ExpressionKind::Path(path) = &path.kind else {
        return None;
    };
    match &literal.kind {
        ExpressionKind::Literal(Literal::Integer(_) | Literal::Float(_) | Literal::String(_)) => {
            Some(path.segments.join("."))
        }
        _ => None,
    }
}

fn collect_paths(expression: &Expression, paths: &mut BTreeSet<String>) {
    match &expression.kind {
        ExpressionKind::Path(path) => {
            paths.insert(path.segments.join("."));
        }
        ExpressionKind::Array(values) => {
            for value in values {
                collect_paths(value, paths);
            }
        }
        ExpressionKind::Object(fields) => {
            for field in fields {
                collect_paths(&field.value, paths);
            }
        }
        ExpressionKind::Unary { operand, .. } => collect_paths(operand, paths),
        ExpressionKind::Binary { left, right, .. } => {
            collect_paths(left, paths);
            collect_paths(right, paths);
        }
        ExpressionKind::Literal(_) => {}
    }
}

#[derive(Clone, Copy)]
enum CommandKind {
    Get,
    Update,
    Delete,
}

fn validate_stages(stages: &[Stage], command: CommandKind) -> Result<()> {
    for stage in stages {
        let valid = match command {
            CommandKind::Get => !matches!(stage, Stage::Set(_)),
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
            Stage::Filter(predicate) => {
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
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    use nova_core::nova_timestamp::NovaTimestamp;
    use nova_index::IndexKey;
    use nova_intelligence::{InMemoryTelemetry, TelemetryAccess};
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
        document.insert(
            "skills",
            NovaValue::Array(vec![
                if active { "Rust" } else { "Python" }.into(),
                "Databases".into(),
            ]),
        );
        document.insert("address", address.into());
        document.insert(
            "created",
            NovaTimestamp::from_millis(i64::from(seed)).into(),
        );
        document
    }

    fn execute_source(
        source: &str,
        backend: &mut impl ExecutionBackend,
    ) -> Result<ExecutionResult> {
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
    fn get_filters_sorts_projects_skips_and_limits() {
        let mut backend = seeded_backend();
        let result = execute_source(
            "students.get { active == true and score >= 80 } | sort score desc | skip 1 | limit 1 | project name, address.city",
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

        let ExecutionResult::Documents(with_rust) = execute_source(
            "students.get { skills contains \"Rust\" and score > 90 }",
            &mut backend,
        )
        .unwrap() else {
            unreachable!()
        };
        assert_eq!(with_rust.len(), 2);
    }

    #[test]
    fn insert_update_delete_and_collection_ddl_execute() {
        let mut backend = MemoryBackend::new();
        assert_eq!(
            execute_source("create collection users", &mut backend).unwrap(),
            ExecutionResult::CollectionCreated("users".to_owned())
        );
        let inserted = execute_source(
            r#"users.insert {name: "Ada", score: 40, profile: {city: "Pune"}}"#,
            &mut backend,
        )
        .unwrap();
        assert!(matches!(inserted, ExecutionResult::Inserted(_)));

        assert_eq!(
            execute_source(
                "users.update { name == \"Ada\" } | set score = score + 2, profile.active = true",
                &mut backend,
            )
            .unwrap(),
            ExecutionResult::Updated(1)
        );
        let ExecutionResult::Documents(documents) =
            execute_source("users.get {}", &mut backend).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(documents[0].get("score"), Some(&NovaValue::Int64(42)));
        assert_eq!(
            documents[0].lookup("profile.active"),
            Some(&NovaValue::Boolean(true))
        );

        assert_eq!(
            execute_source("users.delete { score == 42 }", &mut backend).unwrap(),
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
        let ExecutionResult::Documents(missing) =
            execute_source("students.get { missing != null } | limit 1", &mut backend).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(missing.len(), 1);

        let ExecutionResult::Documents(ids) =
            execute_source("students.get { _id == _id }", &mut backend).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(ids.len(), 4);
        let query = format!("students.get {{ _id == \"{}\" }}", id(1));
        assert_eq!(
            execute_source(&query, &mut backend).unwrap(),
            ExecutionResult::Documents(Vec::new())
        );
    }

    #[test]
    fn runtime_type_errors_do_not_panic() {
        let cases = [
            "students.get { score }",
            "students.get { score + 1.0 > 2 }",
            "students.get { score / 0 > 1 }",
            "students.get { name contains \"A\" }",
            "students.get {} | sort address",
            "students.update {} | set _id = 1",
            "students.update {} | set address.city.name = \"x\"",
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
    fn explain_and_backend_capabilities_are_explicit() {
        let mut backend = seeded_backend();
        assert_eq!(
            execute_source("explain students.get {}", &mut backend).unwrap(),
            ExecutionResult::Explained("CollectionScan(students)".to_owned())
        );
        assert!(matches!(
            execute_source("create index by_score on students (score)", &mut backend),
            Err(NovaError::Unsupported(_))
        ));
    }

    #[test]
    fn indexed_backend_executes_ddl_backfill_and_crud_maintenance() {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT_DIRECTORY.fetch_add(1, AtomicOrdering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "novadb-executor-index-{}-{sequence}",
            std::process::id()
        ));
        let mut backend = crate::IndexedBackend::open(seeded_backend(), &directory).unwrap();
        assert_eq!(
            execute_source("create index by_score on students (score)", &mut backend).unwrap(),
            ExecutionResult::IndexCreated("by_score".to_owned())
        );
        assert_eq!(
            backend
                .indexes()
                .lookup("by_score", &IndexKey::Int64(91))
                .unwrap(),
            vec![id(1)]
        );
        assert_eq!(
            execute_source(
                "explain students.get { score == 91 } | project name",
                &mut backend
            )
            .unwrap(),
            ExecutionResult::Explained(
                "IndexScan(students, by_score, Int64(91)) -> Filter -> Project".to_owned()
            )
        );
        assert_eq!(
            execute_source(
                "students.update { name == \"Ada\" } | set score = 99",
                &mut backend
            )
            .unwrap(),
            ExecutionResult::Updated(1)
        );
        assert!(backend
            .indexes()
            .lookup("by_score", &IndexKey::Int64(91))
            .unwrap()
            .is_empty());
        assert_eq!(
            backend
                .indexes()
                .lookup("by_score", &IndexKey::Int64(99))
                .unwrap(),
            vec![id(1)]
        );
        execute_source("students.delete { name == \"Ada\" }", &mut backend).unwrap();
        assert!(backend
            .indexes()
            .lookup("by_score", &IndexKey::Int64(99))
            .unwrap()
            .is_empty());
        assert_eq!(
            execute_source("drop index by_score", &mut backend).unwrap(),
            ExecutionResult::IndexDropped("by_score".to_owned())
        );
        assert!(matches!(
            execute_source(
                "create index composite on students (score, name)",
                &mut backend
            ),
            Err(NovaError::Unsupported(_))
        ));
        drop(backend);
        fs::remove_dir_all(directory).unwrap();
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

    #[test]
    fn telemetry_is_literal_free_and_observes_success_and_failure() {
        let sink = InMemoryTelemetry::new();
        let mut backend = seeded_backend();
        let query = parse("students.get { score == 91 } | limit 1").unwrap();
        execute_with_telemetry(&query, &mut backend, &sink).unwrap();
        let range = parse("students.get { score >= 80 }").unwrap();
        execute_with_telemetry(&range, &mut backend, &sink).unwrap();
        let bad = parse("students.get { score }").unwrap();
        assert!(execute_with_telemetry(&bad, &mut backend, &sink).is_err());
        let events = sink.snapshot().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].fingerprint, "get:students|filter|limit");
        assert!(!events[0].fingerprint.contains("91"));
        assert_eq!(events[0].predicate_paths, ["score"]);
        assert_eq!(events[0].index_candidate_paths, ["score"]);
        assert_eq!(events[0].access, TelemetryAccess::CollectionScan);
        assert_eq!((events[0].examined, events[0].returned), (4, 1));
        assert!(events[0].succeeded);
        assert!(events[1].index_candidate_paths.is_empty());
        assert!(!events[2].succeeded);
    }
}
