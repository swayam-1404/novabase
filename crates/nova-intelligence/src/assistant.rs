use std::fmt::Write;

use nova_core::error::{NovaError, Result};

use crate::{ImpactEvaluation, IndexRecommendation};

/// Runtime limits for the optional explanation provider boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssistantLimits {
    pub max_schema_bytes: usize,
    pub max_output_bytes: usize,
}

impl Default for AssistantLimits {
    fn default() -> Self {
        Self {
            max_schema_bytes: 1_024,
            max_output_bytes: 4_096,
        }
    }
}

/// Injected optional language-model or other narrative provider.
pub trait ExplanationProvider: Send + Sync {
    /// Returns display-only prose for a grounded prompt.
    ///
    /// # Errors
    /// Provider failures must be returned as typed errors.
    fn explain(&self, prompt: &str) -> Result<String>;
}

/// Grounded output whose optional narrative is never executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantExplanation {
    pub deterministic_summary: String,
    pub suggested_novaql: String,
    pub optional_narrative: Option<String>,
}

/// Builds a grounded recommendation explanation, optionally adding provider prose.
///
/// The provider receives schema names and aggregate evidence only. Its output is
/// bounded and returned as display-only text; this function never parses or
/// executes provider output.
///
/// # Errors
/// Returns a typed error for unsafe schema names, invalid limits, oversized or
/// empty provider output, or a provider failure.
pub fn explain_recommendation(
    recommendation: &IndexRecommendation,
    evaluation: Option<&ImpactEvaluation>,
    provider: Option<&dyn ExplanationProvider>,
    limits: AssistantLimits,
) -> Result<AssistantExplanation> {
    validate_limits(limits)?;
    validate_schema(recommendation, limits.max_schema_bytes)?;
    let deterministic_summary = deterministic_summary(recommendation, evaluation);
    let suggested_novaql = suggested_novaql(recommendation);
    let optional_narrative = if let Some(provider) = provider {
        let output = provider.explain(&grounded_prompt(
            recommendation,
            evaluation,
            &suggested_novaql,
        ))?;
        if output.trim().is_empty() {
            return Err(NovaError::InvalidArgument(
                "assistant output cannot be empty".to_owned(),
            ));
        }
        if output.len() > limits.max_output_bytes {
            return Err(NovaError::InvalidArgument(format!(
                "assistant output exceeds {} bytes",
                limits.max_output_bytes
            )));
        }
        Some(output)
    } else {
        None
    };
    Ok(AssistantExplanation {
        deterministic_summary,
        suggested_novaql,
        optional_narrative,
    })
}

fn validate_limits(limits: AssistantLimits) -> Result<()> {
    if limits.max_schema_bytes == 0 || limits.max_output_bytes == 0 {
        return Err(NovaError::InvalidArgument(
            "assistant limits must be greater than zero".to_owned(),
        ));
    }
    Ok(())
}

fn validate_schema(recommendation: &IndexRecommendation, max_bytes: usize) -> Result<()> {
    let schema_bytes = recommendation
        .collection
        .len()
        .saturating_add(recommendation.path.len());
    if schema_bytes > max_bytes {
        return Err(NovaError::InvalidArgument(format!(
            "assistant schema context exceeds {max_bytes} bytes"
        )));
    }
    if !valid_identifier(&recommendation.collection)
        || recommendation
            .path
            .split('.')
            .any(|part| !valid_identifier(part))
    {
        return Err(NovaError::InvalidArgument(
            "assistant recommendation contains an invalid schema identifier".to_owned(),
        ));
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character == '_' || character.is_alphanumeric())
}

fn deterministic_summary(
    recommendation: &IndexRecommendation,
    evaluation: Option<&ImpactEvaluation>,
) -> String {
    let evidence = &recommendation.evidence;
    let mut summary = format!(
        "{} collection scans examined {} documents and returned {} for {}.{} ({} basis points).",
        evidence.collection_scans,
        evidence.collection_scan_examined,
        evidence.collection_scan_returned,
        recommendation.collection,
        recommendation.path,
        evidence.return_ratio_basis_points
    );
    if let Some(evaluation) = evaluation {
        write!(
            summary,
            " After indexing, {} observations changed average examined documents from {} to {} and average elapsed time from {} to {} microseconds.",
            evaluation.observed_index_scans,
            evaluation.baseline_average_examined,
            evaluation.observed_average_examined,
            evaluation.baseline_average_elapsed_micros,
            evaluation.observed_average_elapsed_micros
        )
        .expect("writing to a String cannot fail");
    }
    summary
}

fn suggested_novaql(recommendation: &IndexRecommendation) -> String {
    let mut name = String::from("nie_");
    for character in recommendation
        .collection
        .chars()
        .chain(std::iter::once('_'))
        .chain(recommendation.path.chars())
    {
        if character.is_ascii_alphanumeric() {
            name.push(character.to_ascii_lowercase());
        } else {
            name.push('_');
        }
    }
    format!(
        "create index {name} on {} ({})",
        recommendation.collection, recommendation.path
    )
}

fn grounded_prompt(
    recommendation: &IndexRecommendation,
    evaluation: Option<&ImpactEvaluation>,
    suggested_novaql: &str,
) -> String {
    let evidence = &recommendation.evidence;
    let evaluation_facts = evaluation.map_or_else(
        || "No post-application evaluation is available.".to_owned(),
        |evaluation| {
            format!(
                "Post-index observations: {}; average examined: {} -> {}; average elapsed microseconds: {} -> {}.",
                evaluation.observed_index_scans,
                evaluation.baseline_average_examined,
                evaluation.observed_average_examined,
                evaluation.baseline_average_elapsed_micros,
                evaluation.observed_average_elapsed_micros
            )
        },
    );
    format!(
        "Explain this advisory NovaDB index recommendation using only the supplied facts. Do not invent performance claims and do not issue instructions to execute automatically. Collection: {}. Path: {}. Collection scans: {}. Examined: {}. Returned: {}. Return ratio basis points: {}. Suggested NovaQL: {suggested_novaql}. {evaluation_facts}",
        recommendation.collection,
        recommendation.path,
        evidence.collection_scans,
        evidence.collection_scan_examined,
        evidence.collection_scan_returned,
        evidence.return_ratio_basis_points
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::RecommendationEvidence;

    use super::*;

    fn recommendation() -> IndexRecommendation {
        IndexRecommendation {
            collection: "students".to_owned(),
            path: "address.state".to_owned(),
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

    #[derive(Default)]
    struct RecordingProvider {
        prompts: Mutex<Vec<String>>,
        output: String,
    }

    impl ExplanationProvider for RecordingProvider {
        fn explain(&self, prompt: &str) -> Result<String> {
            self.prompts.lock().unwrap().push(prompt.to_owned());
            Ok(self.output.clone())
        }
    }

    #[test]
    fn deterministic_mode_needs_no_provider() {
        let explanation =
            explain_recommendation(&recommendation(), None, None, AssistantLimits::default())
                .unwrap();
        assert!(explanation.deterministic_summary.contains("300"));
        assert_eq!(
            explanation.suggested_novaql,
            "create index nie_students_address_state on students (address.state)"
        );
        assert_eq!(explanation.optional_narrative, None);
    }

    #[test]
    fn provider_receives_grounded_aggregates_and_output_is_display_only() {
        let provider = RecordingProvider {
            prompts: Mutex::new(Vec::new()),
            output: "This field is selective; review the proposed index.".to_owned(),
        };
        let explanation = explain_recommendation(
            &recommendation(),
            None,
            Some(&provider),
            AssistantLimits::default(),
        )
        .unwrap();
        assert_eq!(
            explanation.optional_narrative.as_deref(),
            Some("This field is selective; review the proposed index.")
        );
        let prompts = provider.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("Collection scans: 3"));
        assert!(prompts[0].contains("Do not invent performance claims"));
    }

    #[test]
    fn invalid_context_and_unbounded_outputs_are_rejected() {
        let mut invalid = recommendation();
        invalid.path = "address.state; drop collection students".to_owned();
        assert!(explain_recommendation(&invalid, None, None, AssistantLimits::default()).is_err());

        let provider = RecordingProvider {
            prompts: Mutex::new(Vec::new()),
            output: "too long".to_owned(),
        };
        assert!(explain_recommendation(
            &recommendation(),
            None,
            Some(&provider),
            AssistantLimits {
                max_schema_bytes: 1_024,
                max_output_bytes: 3,
            }
        )
        .is_err());
    }
}
