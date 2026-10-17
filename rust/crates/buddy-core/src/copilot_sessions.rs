//! Bounded, pure parsing and the existing VS Code session efficiency metrics.
//! Filesystem discovery and SDK conversations are separate from this format.
#[path = "copilot_sessions/parsing.rs"]
mod parsing;
#[path = "copilot_sessions/sources.rs"]
pub mod sources;
#[path = "copilot_sessions/storage.rs"]
pub mod storage;

use serde::Serialize;

pub use parsing::{Limits, ParseError, parse};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub family: String,
    pub vendor: String,
    pub multiplier: String,
    pub multiplier_numeric: f64,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
}

impl Default for ModelInfo {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            family: String::new(),
            vendor: String::new(),
            multiplier: "1x".into(),
            multiplier_numeric: 1.0,
            max_input_tokens: 0,
            max_output_tokens: 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestResult {
    pub prompt: String,
    pub prompt_tokens: u64,
    pub output_tokens: u64,
    pub first_progress_ms: f64,
    pub total_elapsed_ms: f64,
    pub tool_call_count: u64,
    pub tool_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub session_id: String,
    pub title: String,
    pub start_time: i64,
    pub model: Option<ModelInfo>,
    pub results: Vec<RequestResult>,
    /// Malformed or unknown structural lines make this a partial observation.
    pub malformed_lines: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub total_prompt_tokens: u64,
    pub total_output_tokens: u64,
    pub total_tool_calls: u64,
    pub total_duration_ms: f64,
    pub tools_used: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Digest {
    pub session_id: String,
    pub workspace_name: String,
    pub model: String,
    pub agent_mode: String,
    pub request_count: usize,
    pub total_prompt_tokens: u64,
    pub total_output_tokens: u64,
    pub total_tool_calls: u64,
    pub total_duration_ms: f64,
    pub token_efficiency: f64,
    pub tool_density: f64,
    pub search_churn: usize,
    /// Legacy approximation, not a provider invoice or current token price.
    pub estimated_cost: f64,
    pub dominant_tools: Vec<String>,
    pub first_prompt: String,
    pub session_date: i64,
    pub digested_at: i64,
    pub partial: bool,
}

impl SessionDetail {
    pub fn totals(&self) -> Totals {
        let mut totals = Totals::default();
        let mut names = std::collections::BTreeSet::new();
        for result in &self.results {
            totals.total_prompt_tokens = totals
                .total_prompt_tokens
                .saturating_add(result.prompt_tokens);
            totals.total_output_tokens = totals
                .total_output_tokens
                .saturating_add(result.output_tokens);
            totals.total_tool_calls = totals
                .total_tool_calls
                .saturating_add(result.tool_call_count);
            totals.total_duration_ms += result.total_elapsed_ms;
            for name in &result.tool_names {
                if names.insert(name) {
                    totals.tools_used.push(name.clone());
                }
            }
        }
        totals
    }

    pub fn digest(&self, workspace_name: &str, agent_mode: &str, now: i64) -> Digest {
        let totals = self.totals();
        let search_patterns = [
            "grep_search",
            "semantic_search",
            "file_search",
            "search_subagent",
        ];
        let mut frequencies = std::collections::BTreeMap::new();
        for result in &self.results {
            for tool in &result.tool_names {
                let order = frequencies.len();
                let count = frequencies.entry(tool.as_str()).or_insert((0_usize, order));
                count.0 += 1;
            }
        }
        let mut frequencies: Vec<_> = frequencies.into_iter().collect();
        frequencies.sort_by(|left, right| right.1.0.cmp(&left.1.0).then(left.1.1.cmp(&right.1.1)));
        let multiplier = self
            .model
            .as_ref()
            .map_or(1.0, |model| model.multiplier_numeric);
        Digest {
            session_id: self.session_id.clone(),
            workspace_name: workspace_name.into(),
            model: self.model.as_ref().map_or_else(String::new, |model| {
                if model.name.is_empty() {
                    model.id.clone()
                } else {
                    model.name.clone()
                }
            }),
            agent_mode: agent_mode.into(),
            request_count: self.results.len(),
            token_efficiency: if totals.total_prompt_tokens == 0 {
                0.0
            } else {
                totals.total_output_tokens as f64 / totals.total_prompt_tokens as f64
            },
            tool_density: if self.results.is_empty() {
                0.0
            } else {
                totals.total_tool_calls as f64 / self.results.len() as f64
            },
            search_churn: self
                .results
                .iter()
                .filter(|result| {
                    result
                        .tool_names
                        .iter()
                        .any(|name| search_patterns.iter().any(|pattern| name.contains(pattern)))
                })
                .count(),
            estimated_cost: (totals.total_prompt_tokens as f64 + totals.total_output_tokens as f64)
                * multiplier
                * 0.000_001,
            dominant_tools: frequencies
                .into_iter()
                .take(3)
                .map(|(name, _)| name.into())
                .collect(),
            first_prompt: self.results.first().map_or_else(String::new, |result| {
                result.prompt.chars().take(200).collect()
            }),
            session_date: if self.start_time == 0 {
                now
            } else {
                self.start_time
            },
            digested_at: now,
            partial: self.malformed_lines > 0,
            total_prompt_tokens: totals.total_prompt_tokens,
            total_output_tokens: totals.total_output_tokens,
            total_tool_calls: totals.total_tool_calls,
            total_duration_ms: totals.total_duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail() -> SessionDetail {
        SessionDetail {
            session_id: "fixture-id".into(),
            title: "Fixture".into(),
            start_time: 1234,
            model: None,
            results: vec![],
            malformed_lines: 0,
        }
    }

    #[test]
    fn aggregates_unique_tools_and_preserves_first_observed_order() {
        let mut session = detail();
        session.results = vec![
            RequestResult {
                prompt_tokens: 100,
                output_tokens: 20,
                tool_call_count: 4,
                total_elapsed_ms: 10.5,
                tool_names: vec!["z_tool".into(), "grep_search".into()],
                ..Default::default()
            },
            RequestResult {
                prompt_tokens: 50,
                output_tokens: 10,
                tool_call_count: 2,
                total_elapsed_ms: 20.25,
                tool_names: vec!["z_tool".into(), "a_tool".into()],
                ..Default::default()
            },
        ];
        let digest = session.digest("workspace", "agent", 9999);
        assert_eq!(digest.total_prompt_tokens, 150);
        assert_eq!(digest.total_output_tokens, 30);
        assert_eq!(digest.total_tool_calls, 6);
        assert_eq!(digest.total_duration_ms, 30.75);
        assert_eq!(digest.token_efficiency, 0.2);
        assert_eq!(digest.tool_density, 3.0);
        assert_eq!(digest.search_churn, 1);
        assert_eq!(digest.dominant_tools, ["z_tool", "grep_search", "a_tool"]);
        assert_eq!(
            session.totals().tools_used,
            ["z_tool", "grep_search", "a_tool"]
        );
        assert_eq!(digest.session_date, 1234);
        assert_eq!(digest.digested_at, 9999);
        assert!(!digest.partial);
    }

    #[test]
    fn unicode_first_prompt_is_limited_by_characters_not_bytes() {
        let mut session = detail();
        session.results.push(RequestResult {
            prompt: "界🦀".repeat(110),
            ..Default::default()
        });
        assert_eq!(session.digest("", "", 0).first_prompt, "界🦀".repeat(100));
    }

    #[test]
    fn zero_inputs_and_missing_date_have_finite_metrics_and_injected_time() {
        let mut session = detail();
        session.start_time = 0;
        session.malformed_lines = 1;
        let digest = session.digest("", "", 42);
        assert_eq!(digest.token_efficiency, 0.0);
        assert_eq!(digest.tool_density, 0.0);
        assert_eq!(digest.estimated_cost, 0.0);
        assert_eq!(digest.session_date, 42);
        assert!(digest.partial);
    }

    #[test]
    fn legacy_cost_estimate_uses_model_multiplier_without_becoming_a_price_claim() {
        let mut session = detail();
        session.model = Some(ModelInfo {
            id: "model-id".into(),
            multiplier_numeric: 3.0,
            ..Default::default()
        });
        session.results.push(RequestResult {
            prompt_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        });
        let digest = session.digest("", "", 0);
        assert_eq!(digest.model, "model-id");
        assert!((digest.estimated_cost - 0.0045).abs() < 0.000_000_1);
    }

    #[test]
    fn default_model_metadata_does_not_turn_an_unknown_multiplier_into_zero_cost() {
        let mut session = detail();
        session.model = None;
        session.results = vec![RequestResult {
            prompt_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        }];
        let missing_metadata = session.digest("", "", 0).estimated_cost;
        session.model = Some(ModelInfo::default());
        assert_eq!(session.digest("", "", 0).estimated_cost, missing_metadata);
        assert_eq!(session.model.as_ref().unwrap().multiplier, "1x");
        session.model.as_mut().unwrap().multiplier_numeric = 0.0;
        assert_eq!(session.digest("", "", 0).estimated_cost, 0.0);
    }
}
