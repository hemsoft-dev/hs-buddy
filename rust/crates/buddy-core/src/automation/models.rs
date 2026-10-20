use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// Convex optional any-fields distinguish an omitted field from explicit null.
pub(super) fn present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkerType {
    Exec,
    Ai,
    Skill,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Shell {
    Powershell,
    Bash,
    Cmd,
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<Shell>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub params: Option<Value>,
    // Includes maxTokens, temperature, repository context and future fields.
    // Keep lossless Convex JSON instead of coercing unsupported values.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    #[serde(rename = "_id")]
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub worker_type: WorkerType,
    pub config: JobConfig,
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub input_params: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    #[serde(rename = "_id")]
    pub id: String,
    pub job_id: String,
    pub status: RunStatus,
    pub started_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub input: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MissedPolicy {
    Catchup,
    Skip,
    Last,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    #[serde(rename = "_id")]
    pub id: String,
    pub job_id: String,
    pub name: String,
    pub cron: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    pub enabled: bool,
    pub missed_policy: MissedPolicy,
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub params: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Claim {
    Retired {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "runId")]
        run_id: String,
    },
    Claimed {
        #[serde(rename = "requestId")]
        request_id: String,
        run: Box<Run>,
        job: Box<Job>,
    },
}

impl Claim {
    pub fn parse(value: Value, expected: &str, candidate: &str) -> Result<Self, InvalidData> {
        let original = serde_json::to_vec(&value).unwrap_or_default();
        let parsed: Self =
            serde_json::from_value(value).map_err(|_| InvalidData(original.clone()))?;
        let valid_identity = uuid::Uuid::parse_str(expected)
            .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == expected);
        let (actual, valid_capture) = match &parsed {
            Self::Retired { request_id, run_id } => {
                (request_id, !run_id.is_empty() && run_id == candidate)
            }
            Self::Claimed {
                request_id,
                run,
                job,
            } => (
                request_id,
                !run.id.is_empty()
                    && run.id == candidate
                    && !job.id.is_empty()
                    && run.job_id == job.id
                    && run.started_at.is_finite()
                    && run.started_at >= 0.0
                    && run.status != RunStatus::Pending,
            ),
        };
        if !valid_identity || actual != expected || !valid_capture {
            return Err(InvalidData(original));
        }
        Ok(parsed)
    }
}

// Configurations, run output and claim identities may contain private data.
macro_rules! private_debug {
    ($($name:ty),+ $(,)?) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), " (private automation data)"))
            }
        }
    )+};
}
private_debug!(JobConfig, Job, Run, Schedule, Claim);

/// Original server data is retained privately, never printed in diagnostics.
pub struct InvalidData(pub Vec<u8>);

impl std::fmt::Debug for InvalidData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Unsupported automation data; original bytes retained")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const REQUEST: &str = "2cd2e788-58ae-493e-90eb-449b6c726a77";

    fn captured() -> Value {
        json!({"kind":"claimed","requestId":REQUEST,
            "run":{"_id":"run-one","jobId":"job-one","status":"running","startedAt":42.0,
                "input":{"keep":"雪"},"future":{"$integer":"KgAAAAAAAAA="}},
            "job":{"_id":"job-one","name":"fixture","workerType":"exec",
                "config":{"command":"fixture","repoOwner":"owner","maxTokens":22.0,
                    "future":{"$bytes":"AQID"}},"inputParams":[{"name":"x","required":true}],
                "_creationTime":12.0,"createdAt":12.0,"updatedAt":12.0}})
    }

    #[test]
    fn claim_roundtrip_keeps_parameters_and_lossless_unknown_fields() {
        let original = captured();
        let claim = Claim::parse(original.clone(), REQUEST, "run-one").unwrap();
        assert_eq!(serde_json::to_value(claim).unwrap(), original);
    }

    #[test]
    fn explicit_null_payloads_survive_the_captured_claim_roundtrip() {
        let mut original = captured();
        original["job"]["config"]["params"] = Value::Null;
        original["run"]["input"] = Value::Null;
        original["run"]["output"] = Value::Null;
        let claim = Claim::parse(original.clone(), REQUEST, "run-one").unwrap();
        assert_eq!(serde_json::to_value(claim).unwrap(), original);
    }

    #[test]
    fn mismatched_or_pending_claims_keep_the_original_for_recovery() {
        for (path, replacement) in [
            (
                vec!["requestId"],
                json!("c1b97af3-8983-4b74-bbdd-a545cb87ecc7"),
            ),
            (vec!["run", "jobId"], json!("another-job")),
            (vec!["run", "status"], json!("pending")),
            (vec!["job", "workerType"], json!("future-worker")),
            (vec!["job", "config", "shell"], json!("unknown-shell")),
        ] {
            let mut original = captured();
            let mut field = &mut original;
            for key in path {
                field = &mut field[key];
            }
            *field = replacement;
            let bytes = serde_json::to_vec(&original).unwrap();
            assert_eq!(
                Claim::parse(original, REQUEST, "run-one").unwrap_err().0,
                bytes
            );
        }
    }

    #[test]
    fn terminal_claims_remain_visible_but_are_not_active() {
        for status in ["completed", "failed", "cancelled"] {
            let mut value = captured();
            value["run"]["status"] = json!(status);
            let Claim::Claimed { run, .. } = Claim::parse(value, REQUEST, "run-one").unwrap()
            else {
                panic!()
            };
            assert!(!run.status.is_active());
        }
    }
}
