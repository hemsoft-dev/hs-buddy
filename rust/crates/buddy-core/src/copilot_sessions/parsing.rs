use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Read as _};

use serde_json::Value;

use super::{ModelInfo, RequestResult, SessionDetail};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_line_bytes: usize,
    pub max_lines: usize,
    pub max_requests: usize,
    pub max_text_bytes: usize,
    pub max_tools_per_request: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_line_bytes: 1024 * 1024,
            max_lines: 50_000,
            max_requests: 4096,
            max_text_bytes: 128 * 1024,
            max_tools_per_request: 256,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    InvalidLimits,
    ReadFailed,
    TooLarge,
    LineTooLarge,
    TooManyLines,
    TooManyRequests,
    TooManyTools,
    TextTooLarge,
    InvalidUtf8,
    MissingSession,
    MixedSessionIds,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidLimits => "Session parser limits must be positive.",
            Self::ReadFailed => "The session file could not be read.",
            Self::TooLarge => "The session file exceeds the detail byte limit.",
            Self::LineTooLarge => "A session record exceeds the line byte limit.",
            Self::TooManyLines => "The session file exceeds the record limit.",
            Self::TooManyRequests => "The session exceeds the request limit.",
            Self::TooManyTools => "A session request exceeds the tool-name limit.",
            Self::TextTooLarge => "A session text field exceeds the detail limit.",
            Self::InvalidUtf8 => "The session file is not valid UTF-8.",
            Self::MissingSession => "No supported session identity was found.",
            Self::MixedSessionIds => "The file contains more than one session identity.",
        })
    }
}

impl std::error::Error for ParseError {}

fn text(value: &Value, limits: Limits) -> Result<String, ParseError> {
    let value = value.as_str().unwrap_or_default();
    if value.len() > limits.max_text_bytes {
        return Err(ParseError::TextTooLarge);
    }
    Ok(value.into())
}

fn count(value: &Value) -> u64 {
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 1_000_000_000_000.0 && n.fract() == 0.0)
        .map_or(0, |n| n as u64)
}

fn duration(value: &Value) -> f64 {
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 1_000_000_000_000.0)
        .unwrap_or(0.0)
}

fn valid_metrics(value: &Value) -> bool {
    let metadata = &value["metadata"];
    let timings = &value["timings"];
    if (!metadata.is_null() && !metadata.is_object())
        || (!timings.is_null() && !timings.is_object())
    {
        return false;
    }
    for field in ["promptTokens", "outputTokens"] {
        let number = if metadata[field].is_null() {
            &timings[field]
        } else {
            &metadata[field]
        };
        if !number.is_null()
            && !number.as_f64().is_some_and(|n| {
                n.is_finite() && (0.0..=1_000_000_000_000.0).contains(&n) && n.fract() == 0.0
            })
        {
            return false;
        }
    }
    for field in ["firstProgress", "totalElapsed"] {
        if !timings[field].is_null()
            && !timings[field]
                .as_f64()
                .is_some_and(|n| n.is_finite() && (0.0..=1_000_000_000_000.0).contains(&n))
        {
            return false;
        }
    }
    let rounds = &metadata["toolCallRounds"];
    if rounds.is_null() {
        return true;
    }
    let Some(rounds) = rounds.as_array() else {
        return false;
    };
    rounds.iter().all(|round| {
        round.is_object()
            && (round["toolCalls"].is_null()
                || round["toolCalls"].as_array().is_some_and(|calls| {
                    calls.iter().all(|call| {
                        call.is_object()
                            && call["name"].as_str().is_some_and(|name| !name.is_empty())
                    })
                }))
    })
}

fn model(value: &Value, limits: Limits) -> Result<Option<ModelInfo>, ParseError> {
    let metadata = &value["inputState"]["selectedModel"]["metadata"];
    if !metadata.is_object() {
        return Ok(None);
    }
    let multiplier = metadata["multiplierNumeric"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 1_000_000.0)
        .unwrap_or(1.0);
    Ok(Some(ModelInfo {
        id: text(&metadata["id"], limits)?,
        name: text(&metadata["name"], limits)?,
        family: text(&metadata["family"], limits)?,
        vendor: text(&metadata["vendor"], limits)?,
        multiplier: metadata["multiplier"].as_str().map_or_else(
            || Ok("1x".into()),
            |_| text(&metadata["multiplier"], limits),
        )?,
        multiplier_numeric: multiplier,
        max_input_tokens: count(&metadata["maxInputTokens"]),
        max_output_tokens: count(&metadata["maxOutputTokens"]),
    }))
}

fn result(value: &Value, limits: Limits) -> Result<RequestResult, ParseError> {
    let mut result = RequestResult::default();
    let metadata = &value["metadata"];
    let timings = &value["timings"];
    result.prompt_tokens = count(if metadata["promptTokens"].is_null() {
        &timings["promptTokens"]
    } else {
        &metadata["promptTokens"]
    });
    result.output_tokens = count(if metadata["outputTokens"].is_null() {
        &timings["outputTokens"]
    } else {
        &metadata["outputTokens"]
    });
    result.first_progress_ms = duration(&timings["firstProgress"]);
    result.total_elapsed_ms = duration(&timings["totalElapsed"]);
    let mut names = BTreeSet::new();
    for round in metadata["toolCallRounds"].as_array().into_iter().flatten() {
        let Some(calls) = round["toolCalls"].as_array() else {
            continue;
        };
        result.tool_call_count = result.tool_call_count.saturating_add(calls.len() as u64);
        for call in calls {
            let Some(name) = call["name"].as_str() else {
                continue;
            };
            if !names.insert(name) {
                continue;
            }
            if names.len() > limits.max_tools_per_request {
                return Err(ParseError::TooManyTools);
            }
            result.tool_names.push(text(&call["name"], limits)?);
        }
    }
    Ok(result)
}

#[derive(Default)]
struct State {
    identity: Option<(String, i64, Option<ModelInfo>)>,
    title: String,
    prompts: BTreeMap<u64, String>,
    results: BTreeMap<u64, RequestResult>,
    malformed_lines: usize,
}

impl State {
    fn prompts(
        &mut self,
        value: &Value,
        overwrite: bool,
        limits: Limits,
    ) -> Result<bool, ParseError> {
        let Some(requests) = value.as_array() else {
            return Ok(false);
        };
        if requests.len() > limits.max_requests {
            return Err(ParseError::TooManyRequests);
        }
        for (index, request) in requests.iter().enumerate() {
            let prompt = text(&request["message"]["text"], limits)?;
            if !prompt.is_empty() && (overwrite || !self.prompts.contains_key(&(index as u64))) {
                self.prompts.insert(index as u64, prompt);
            }
        }
        Ok(true)
    }

    fn line(&mut self, line: Value, limits: Limits) -> Result<bool, ParseError> {
        let value = &line["v"];
        let keys = line["k"].as_array();
        match line["kind"].as_u64() {
            Some(0) => {
                let id = text(&value["sessionId"], limits)?;
                if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
                    return Ok(false);
                }
                if self.identity.as_ref().is_some_and(|old| old.0 != id) {
                    return Err(ParseError::MixedSessionIds);
                }
                let created = value["creationDate"]
                    .as_i64()
                    .filter(|n| (0..=253_402_300_799_999).contains(n))
                    .unwrap_or(0);
                self.identity = Some((id, created, model(value, limits)?));
                let title = text(&value["customTitle"], limits)?;
                if !title.is_empty() {
                    self.title = title;
                }
                if !value["requests"].is_null()
                    && !self.prompts(&value["requests"], true, limits)?
                {
                    return Ok(false);
                }
                Ok(true)
            }
            Some(1) => {
                let Some(keys) = keys else {
                    return Ok(false);
                };
                if keys.len() == 1 && keys[0] == "customTitle" {
                    let title = text(value, limits)?;
                    if !title.is_empty() {
                        self.title = title;
                    }
                } else if keys.len() == 3 && keys[0] == "requests" && keys[2] == "result" {
                    let index = keys[1]
                        .as_u64()
                        .or_else(|| keys[1].as_str().and_then(|v| v.parse().ok()));
                    let Some(index) = index.filter(|n| *n <= 1_000_000) else {
                        return Ok(false);
                    };
                    if !value.is_object() {
                        return Ok(false);
                    }
                    if self.results.len() >= limits.max_requests
                        && !self.results.contains_key(&index)
                    {
                        return Err(ParseError::TooManyRequests);
                    }
                    self.results.insert(index, result(value, limits)?);
                    return Ok(valid_metrics(value));
                }
                Ok(true)
            }
            Some(2) => {
                if keys.is_some_and(|keys| keys.len() == 1 && keys[0] == "requests") {
                    self.prompts(value, false, limits)
                } else {
                    Ok(true)
                }
            }
            _ => Ok(false),
        }
    }
}

/// Consumes at most one byte beyond a configured byte limit before rejection.
/// Malformed JSON lines remain visible as a partial-data count. Size, encoding
/// and identity violations are explicit failures and do not yield a full digest.
pub fn parse(reader: &mut impl BufRead, limits: Limits) -> Result<SessionDetail, ParseError> {
    if [
        limits.max_bytes,
        limits.max_line_bytes,
        limits.max_lines,
        limits.max_requests,
        limits.max_text_bytes,
        limits.max_tools_per_request,
    ]
    .contains(&0)
    {
        return Err(ParseError::InvalidLimits);
    }
    let mut state = State::default();
    let mut total = 0_usize;
    let mut lines = 0_usize;
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        let remaining = limits.max_bytes.saturating_sub(total);
        let max = remaining
            .saturating_add(1)
            .min(limits.max_line_bytes.saturating_add(1));
        let read = reader
            .by_ref()
            .take(max as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(|_| ParseError::ReadFailed)?;
        if read == 0 {
            break;
        }
        if read > remaining {
            return Err(ParseError::TooLarge);
        }
        if read > limits.max_line_bytes {
            return Err(ParseError::LineTooLarge);
        }
        total += read;
        lines += 1;
        if lines > limits.max_lines {
            return Err(ParseError::TooManyLines);
        }
        let line = std::str::from_utf8(&bytes).map_err(|_| ParseError::InvalidUtf8)?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(value) => {
                if !state.line(value, limits)? {
                    state.malformed_lines += 1;
                }
            }
            Err(_) => state.malformed_lines += 1,
        }
    }
    let (session_id, start_time, model) = state.identity.ok_or(ParseError::MissingSession)?;
    let results = state
        .results
        .into_iter()
        .map(|(index, mut result)| {
            result.prompt = state.prompts.remove(&index).unwrap_or_default();
            result
        })
        .collect();
    let title = if state.title.is_empty() {
        format!("Session {}", session_id.chars().take(8).collect::<String>())
    } else {
        state.title
    };
    Ok(SessionDetail {
        session_id,
        title,
        start_time,
        model,
        results,
        malformed_lines: state.malformed_lines,
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
