use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use nova_executor::{execute, execute_with_telemetry, IndexedBackend, MemoryBackend};
use nova_intelligence::{
    analyze_workload, explain_recommendation, recommend_indexes, AdvisorConfig, AssistantLimits,
    EvaluationConfig, InMemoryTelemetry, RecommendationLifecycle,
};
use nova_query::parse;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "novadb-hardening-{}-{sequence}",
            std::process::id()
        )))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn workload_recommendation_application_and_evaluation_are_end_to_end() {
    let directory = TestDirectory::new();
    let mut backend = IndexedBackend::open(MemoryBackend::new(), &directory.0).unwrap();
    execute(&parse("create collection students").unwrap(), &mut backend).unwrap();
    for index in 0..10 {
        let branch = if index == 0 { "CSE" } else { "ECE" };
        let source = format!("students.insert {{ branch: \"{branch}\", score: {index} }}");
        execute(&parse(&source).unwrap(), &mut backend).unwrap();
    }

    let telemetry = InMemoryTelemetry::new();
    let query = parse("students.get { branch == \"CSE\" }").unwrap();
    for _ in 0..3 {
        execute_with_telemetry(&query, &mut backend, &telemetry).unwrap();
    }
    let baseline = analyze_workload(&telemetry.snapshot().unwrap());
    let recommendations = recommend_indexes(
        &baseline,
        &backend.indexes().definitions(),
        AdvisorConfig {
            min_successful_executions: 3,
            min_collection_scans: 3,
            min_collection_scan_examined: 1,
            max_return_ratio_basis_points: 2_500,
        },
    )
    .unwrap();
    assert_eq!(recommendations.len(), 1);

    let explanation =
        explain_recommendation(&recommendations[0], None, None, AssistantLimits::default())
            .unwrap();
    let mut lifecycle = RecommendationLifecycle::from_recommendations(recommendations).unwrap();
    lifecycle.accept("students", "branch").unwrap();
    execute(&parse(&explanation.suggested_novaql).unwrap(), &mut backend).unwrap();
    lifecycle.mark_applied("students", "branch").unwrap();

    telemetry.clear().unwrap();
    for _ in 0..3 {
        execute_with_telemetry(&query, &mut backend, &telemetry).unwrap();
    }
    let indexed = analyze_workload(&telemetry.snapshot().unwrap());
    assert_eq!(indexed.index_candidates[0].index_scans, 3);
    let evaluation = lifecycle
        .evaluate("students", "branch", &indexed, EvaluationConfig::default())
        .unwrap();
    assert_eq!(evaluation.baseline_average_examined, 10);
    assert_eq!(evaluation.observed_average_examined, 1);
    assert_eq!(evaluation.examined_reduction_basis_points, 9_000);
}
