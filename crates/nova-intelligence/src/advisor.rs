use std::collections::BTreeSet;

use nova_core::error::{NovaError, Result};
use nova_index::IndexDefinition;

use crate::WorkloadReport;

const BASIS_POINTS: u128 = 10_000;

/// Evidence thresholds used by the deterministic index advisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvisorConfig {
    pub min_successful_executions: u64,
    pub min_collection_scans: u64,
    pub min_collection_scan_examined: u64,
    pub max_return_ratio_basis_points: u16,
}

impl Default for AdvisorConfig {
    fn default() -> Self {
        Self {
            min_successful_executions: 3,
            min_collection_scans: 3,
            min_collection_scan_examined: 100,
            max_return_ratio_basis_points: 2_500,
        }
    }
}

/// Exact, inspectable evidence supporting an index recommendation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecommendationEvidence {
    pub successful_executions: u64,
    pub collection_scans: u64,
    pub index_scans: u64,
    pub collection_scan_examined: u64,
    pub collection_scan_returned: u64,
    pub collection_scan_elapsed_micros: u64,
    pub return_ratio_basis_points: u16,
}

/// Advisory-only proposal for a single-field index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecommendation {
    pub collection: String,
    pub path: String,
    pub evidence: RecommendationEvidence,
}

/// Produces deterministic, evidence-based single-field index proposals.
///
/// Only equality paths usable by the current planner are considered. Existing
/// collection/path pairs are excluded. The function never changes the catalog.
///
/// # Errors
/// Returns [`NovaError::InvalidArgument`] when a threshold is zero or the
/// configured return ratio is greater than 100%.
pub fn recommend_indexes(
    report: &WorkloadReport,
    existing_indexes: &[IndexDefinition],
    config: AdvisorConfig,
) -> Result<Vec<IndexRecommendation>> {
    validate_config(config)?;
    let existing: BTreeSet<(String, String)> = existing_indexes
        .iter()
        .map(|definition| (definition.collection.clone(), definition.field.join(".")))
        .collect();
    let mut recommendations = Vec::new();

    for candidate in &report.index_candidates {
        if existing.contains(&(candidate.collection.clone(), candidate.path.clone()))
            || candidate.successful_executions < config.min_successful_executions
            || candidate.collection_scans < config.min_collection_scans
            || candidate.collection_scan_examined < config.min_collection_scan_examined
        {
            continue;
        }
        let ratio = ratio_basis_points(
            candidate.collection_scan_returned,
            candidate.collection_scan_examined,
        );
        if ratio > config.max_return_ratio_basis_points {
            continue;
        }
        recommendations.push(IndexRecommendation {
            collection: candidate.collection.clone(),
            path: candidate.path.clone(),
            evidence: RecommendationEvidence {
                successful_executions: candidate.successful_executions,
                collection_scans: candidate.collection_scans,
                index_scans: candidate.index_scans,
                collection_scan_examined: candidate.collection_scan_examined,
                collection_scan_returned: candidate.collection_scan_returned,
                collection_scan_elapsed_micros: candidate.collection_scan_elapsed_micros,
                return_ratio_basis_points: ratio,
            },
        });
    }
    Ok(recommendations)
}

fn validate_config(config: AdvisorConfig) -> Result<()> {
    if config.min_successful_executions == 0
        || config.min_collection_scans == 0
        || config.min_collection_scan_examined == 0
    {
        return Err(NovaError::InvalidArgument(
            "advisor count thresholds must be greater than zero".to_owned(),
        ));
    }
    if u128::from(config.max_return_ratio_basis_points) > BASIS_POINTS {
        return Err(NovaError::InvalidArgument(
            "advisor return ratio must not exceed 10000 basis points".to_owned(),
        ));
    }
    Ok(())
}

fn ratio_basis_points(returned: u64, examined: u64) -> u16 {
    if examined == 0 {
        return 10_000;
    }
    let ratio = u128::from(returned)
        .saturating_mul(BASIS_POINTS)
        .checked_div(u128::from(examined))
        .unwrap_or(BASIS_POINTS)
        .min(BASIS_POINTS);
    u16::try_from(ratio).unwrap_or(10_000)
}

#[cfg(test)]
mod tests {
    use crate::{analyze_workload, TelemetryAccess, TelemetryEvent};

    use super::*;

    fn observations(path: &str, returned: usize, count: usize) -> Vec<TelemetryEvent> {
        (0..count)
            .map(|_| TelemetryEvent {
                fingerprint: "get:students|filter".to_owned(),
                collection: Some("students".to_owned()),
                predicate_paths: vec![path.to_owned()],
                index_candidate_paths: vec![path.to_owned()],
                access: TelemetryAccess::CollectionScan,
                examined: 100,
                returned,
                elapsed_micros: 50,
                succeeded: true,
            })
            .collect()
    }

    #[test]
    fn recommends_selective_repeated_collection_scans_with_evidence() {
        let report = analyze_workload(&observations("branch", 10, 3));
        let recommendations = recommend_indexes(&report, &[], AdvisorConfig::default()).unwrap();
        assert_eq!(recommendations.len(), 1);
        let recommendation = &recommendations[0];
        assert_eq!(recommendation.collection, "students");
        assert_eq!(recommendation.path, "branch");
        assert_eq!(recommendation.evidence.collection_scans, 3);
        assert_eq!(recommendation.evidence.collection_scan_examined, 300);
        assert_eq!(recommendation.evidence.return_ratio_basis_points, 1_000);
    }

    #[test]
    fn skips_existing_sparse_and_unselective_candidates() {
        let mut events = observations("branch", 10, 3);
        events.extend(observations("name", 5, 2));
        events.extend(observations("active", 90, 3));
        let report = analyze_workload(&events);
        let existing = [IndexDefinition {
            name: "by_branch".to_owned(),
            collection: "students".to_owned(),
            field: vec!["branch".to_owned()],
        }];
        assert!(
            recommend_indexes(&report, &existing, AdvisorConfig::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rejects_invalid_thresholds() {
        let config = AdvisorConfig {
            min_collection_scans: 0,
            ..AdvisorConfig::default()
        };
        assert!(matches!(
            recommend_indexes(&WorkloadReport::default(), &[], config),
            Err(NovaError::InvalidArgument(_))
        ));
        let config = AdvisorConfig {
            max_return_ratio_basis_points: 10_001,
            ..AdvisorConfig::default()
        };
        assert!(recommend_indexes(&WorkloadReport::default(), &[], config).is_err());
    }
}
