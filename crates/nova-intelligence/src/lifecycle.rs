use std::collections::BTreeMap;

use nova_core::error::{NovaError, Result};

use crate::{IndexRecommendation, WorkloadReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecommendationStatus {
    Proposed,
    Accepted,
    Dismissed,
    Applied,
    Evaluated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvaluationConfig {
    pub min_index_scans: u64,
}

impl Default for EvaluationConfig {
    fn default() -> Self {
        Self { min_index_scans: 3 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactEvaluation {
    pub baseline_collection_scans: u64,
    pub observed_index_scans: u64,
    pub baseline_average_examined: u64,
    pub observed_average_examined: u64,
    pub baseline_average_elapsed_micros: u64,
    pub observed_average_elapsed_micros: u64,
    pub examined_reduction_basis_points: i32,
    pub elapsed_reduction_basis_points: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecommendationRecord {
    pub recommendation: IndexRecommendation,
    pub status: RecommendationStatus,
    pub evaluation: Option<ImpactEvaluation>,
}

/// In-memory, deterministic recommendation decision and evaluation history.
#[derive(Debug, Default)]
pub struct RecommendationLifecycle {
    records: BTreeMap<(String, String), RecommendationRecord>,
}

impl RecommendationLifecycle {
    /// Creates proposed records, rejecting duplicate collection/path pairs.
    ///
    /// # Errors
    /// Returns [`NovaError::InvalidArgument`] for duplicate recommendations.
    pub fn from_recommendations(recommendations: Vec<IndexRecommendation>) -> Result<Self> {
        let mut records = BTreeMap::new();
        for recommendation in recommendations {
            let key = (
                recommendation.collection.clone(),
                recommendation.path.clone(),
            );
            if records
                .insert(
                    key,
                    RecommendationRecord {
                        recommendation,
                        status: RecommendationStatus::Proposed,
                        evaluation: None,
                    },
                )
                .is_some()
            {
                return Err(NovaError::InvalidArgument(
                    "duplicate index recommendation".to_owned(),
                ));
            }
        }
        Ok(Self { records })
    }

    #[must_use]
    pub fn records(&self) -> Vec<&RecommendationRecord> {
        self.records.values().collect()
    }

    /// Accepts a proposed recommendation.
    ///
    /// # Errors
    /// Returns a typed error for a missing recommendation or invalid transition.
    pub fn accept(&mut self, collection: &str, path: &str) -> Result<()> {
        self.transition(
            collection,
            path,
            RecommendationStatus::Proposed,
            RecommendationStatus::Accepted,
        )
    }

    /// Dismisses a proposed recommendation.
    ///
    /// # Errors
    /// Returns a typed error for a missing recommendation or invalid transition.
    pub fn dismiss(&mut self, collection: &str, path: &str) -> Result<()> {
        self.transition(
            collection,
            path,
            RecommendationStatus::Proposed,
            RecommendationStatus::Dismissed,
        )
    }

    /// Marks an accepted recommendation as externally applied.
    ///
    /// This method does not modify the index catalog.
    ///
    /// # Errors
    /// Returns a typed error for a missing recommendation or invalid transition.
    pub fn mark_applied(&mut self, collection: &str, path: &str) -> Result<()> {
        self.transition(
            collection,
            path,
            RecommendationStatus::Accepted,
            RecommendationStatus::Applied,
        )
    }

    /// Evaluates an applied recommendation against a later workload window.
    ///
    /// # Errors
    /// Returns a typed error for an invalid threshold, missing evidence,
    /// insufficient indexed observations, or an invalid lifecycle transition.
    pub fn evaluate(
        &mut self,
        collection: &str,
        path: &str,
        report: &WorkloadReport,
        config: EvaluationConfig,
    ) -> Result<ImpactEvaluation> {
        if config.min_index_scans == 0 {
            return Err(NovaError::InvalidArgument(
                "evaluation observation threshold must be greater than zero".to_owned(),
            ));
        }
        let record = self.record_mut(collection, path)?;
        if record.status != RecommendationStatus::Applied {
            return Err(invalid_transition(record.status, "evaluated"));
        }
        let candidate = report
            .index_candidates
            .iter()
            .find(|candidate| candidate.collection == collection && candidate.path == path)
            .ok_or_else(|| {
                NovaError::NotFound(format!(
                    "post-application telemetry for {collection}.{path}"
                ))
            })?;
        if candidate.index_scans < config.min_index_scans {
            return Err(NovaError::InvalidState(format!(
                "need at least {} index scans, observed {}",
                config.min_index_scans, candidate.index_scans
            )));
        }

        let baseline_scans = record.recommendation.evidence.collection_scans;
        let baseline_examined = average(
            record.recommendation.evidence.collection_scan_examined,
            baseline_scans,
        );
        let baseline_elapsed = average(
            record
                .recommendation
                .evidence
                .collection_scan_elapsed_micros,
            baseline_scans,
        );
        let observed_examined = average(candidate.index_scan_examined, candidate.index_scans);
        let observed_elapsed = average(candidate.index_scan_elapsed_micros, candidate.index_scans);
        let evaluation = ImpactEvaluation {
            baseline_collection_scans: baseline_scans,
            observed_index_scans: candidate.index_scans,
            baseline_average_examined: baseline_examined,
            observed_average_examined: observed_examined,
            baseline_average_elapsed_micros: baseline_elapsed,
            observed_average_elapsed_micros: observed_elapsed,
            examined_reduction_basis_points: reduction_basis_points(
                baseline_examined,
                observed_examined,
            ),
            elapsed_reduction_basis_points: reduction_basis_points(
                baseline_elapsed,
                observed_elapsed,
            ),
        };
        record.evaluation = Some(evaluation.clone());
        record.status = RecommendationStatus::Evaluated;
        Ok(evaluation)
    }

    fn transition(
        &mut self,
        collection: &str,
        path: &str,
        expected: RecommendationStatus,
        next: RecommendationStatus,
    ) -> Result<()> {
        let record = self.record_mut(collection, path)?;
        if record.status != expected {
            return Err(invalid_transition(record.status, status_name(next)));
        }
        record.status = next;
        Ok(())
    }

    fn record_mut(&mut self, collection: &str, path: &str) -> Result<&mut RecommendationRecord> {
        self.records
            .get_mut(&(collection.to_owned(), path.to_owned()))
            .ok_or_else(|| NovaError::NotFound(format!("recommendation for {collection}.{path}")))
    }
}

fn average(total: u64, count: u64) -> u64 {
    total.checked_div(count).unwrap_or(0)
}

fn reduction_basis_points(baseline: u64, observed: u64) -> i32 {
    if baseline == 0 {
        return 0;
    }
    let difference = i128::from(baseline) - i128::from(observed);
    let basis_points = difference
        .saturating_mul(10_000)
        .checked_div(i128::from(baseline))
        .unwrap_or(0);
    i32::try_from(basis_points).unwrap_or(if basis_points.is_negative() {
        i32::MIN
    } else {
        i32::MAX
    })
}

const fn status_name(status: RecommendationStatus) -> &'static str {
    match status {
        RecommendationStatus::Proposed => "proposed",
        RecommendationStatus::Accepted => "accepted",
        RecommendationStatus::Dismissed => "dismissed",
        RecommendationStatus::Applied => "applied",
        RecommendationStatus::Evaluated => "evaluated",
    }
}

fn invalid_transition(current: RecommendationStatus, next: &str) -> NovaError {
    NovaError::InvalidState(format!(
        "cannot mark {} recommendation as {next}",
        status_name(current)
    ))
}

#[cfg(test)]
mod tests {
    use crate::{analyze_workload, RecommendationEvidence, TelemetryAccess, TelemetryEvent};

    use super::*;

    fn recommendation(path: &str) -> IndexRecommendation {
        IndexRecommendation {
            collection: "students".to_owned(),
            path: path.to_owned(),
            evidence: RecommendationEvidence {
                successful_executions: 3,
                collection_scans: 3,
                index_scans: 0,
                collection_scan_examined: 300,
                collection_scan_returned: 30,
                collection_scan_elapsed_micros: 600,
                return_ratio_basis_points: 1_000,
            },
        }
    }

    fn indexed_events(count: usize) -> Vec<TelemetryEvent> {
        (0..count)
            .map(|_| TelemetryEvent {
                fingerprint: "get:students|filter".to_owned(),
                collection: Some("students".to_owned()),
                predicate_paths: vec!["branch".to_owned()],
                index_candidate_paths: vec!["branch".to_owned()],
                access: TelemetryAccess::IndexScan {
                    index: "by_branch".to_owned(),
                },
                examined: 10,
                returned: 10,
                elapsed_micros: 50,
                succeeded: true,
            })
            .collect()
    }

    #[test]
    fn lifecycle_requires_explicit_valid_transitions() {
        let mut lifecycle =
            RecommendationLifecycle::from_recommendations(vec![recommendation("branch")]).unwrap();
        assert_eq!(
            lifecycle.records()[0].status,
            RecommendationStatus::Proposed
        );
        assert!(lifecycle.mark_applied("students", "branch").is_err());
        lifecycle.accept("students", "branch").unwrap();
        lifecycle.mark_applied("students", "branch").unwrap();
        assert_eq!(lifecycle.records()[0].status, RecommendationStatus::Applied);
        assert!(lifecycle.accept("students", "missing").is_err());
    }

    #[test]
    fn dismissal_is_terminal_and_duplicates_are_rejected() {
        let proposal = recommendation("branch");
        assert!(
            RecommendationLifecycle::from_recommendations(vec![proposal.clone(), proposal])
                .is_err()
        );
        let mut lifecycle =
            RecommendationLifecycle::from_recommendations(vec![recommendation("branch")]).unwrap();
        lifecycle.dismiss("students", "branch").unwrap();
        assert!(lifecycle.accept("students", "branch").is_err());
    }

    #[test]
    fn evaluation_compares_baseline_with_indexed_window() {
        let mut lifecycle =
            RecommendationLifecycle::from_recommendations(vec![recommendation("branch")]).unwrap();
        lifecycle.accept("students", "branch").unwrap();
        lifecycle.mark_applied("students", "branch").unwrap();
        let report = analyze_workload(&indexed_events(3));
        let evaluation = lifecycle
            .evaluate("students", "branch", &report, EvaluationConfig::default())
            .unwrap();
        assert_eq!(
            (
                evaluation.baseline_average_examined,
                evaluation.observed_average_examined
            ),
            (100, 10)
        );
        assert_eq!(evaluation.examined_reduction_basis_points, 9_000);
        assert_eq!(evaluation.elapsed_reduction_basis_points, 7_500);
        assert_eq!(
            lifecycle.records()[0].status,
            RecommendationStatus::Evaluated
        );
        assert_eq!(lifecycle.records()[0].evaluation, Some(evaluation));
    }

    #[test]
    fn evaluation_requires_enough_post_application_evidence() {
        let mut lifecycle =
            RecommendationLifecycle::from_recommendations(vec![recommendation("branch")]).unwrap();
        lifecycle.accept("students", "branch").unwrap();
        lifecycle.mark_applied("students", "branch").unwrap();
        let report = analyze_workload(&indexed_events(1));
        assert!(matches!(
            lifecycle.evaluate("students", "branch", &report, EvaluationConfig::default()),
            Err(NovaError::InvalidState(_))
        ));
        assert_eq!(lifecycle.records()[0].status, RecommendationStatus::Applied);
    }

    #[test]
    fn impact_math_reports_regressions_and_handles_zero_baselines() {
        assert_eq!(reduction_basis_points(100, 200), -10_000);
        assert_eq!(reduction_basis_points(0, 10), 0);
    }
}
