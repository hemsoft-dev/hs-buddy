//! The selected account's REST quota, as Electron's `getRateLimit` reads it
//! for the organization overview's gauge.
use serde_json::Value;

use super::{ApiRequest, GitHubClient, GitHubError, Loaded};

/// `resources.core` of `GET /rate_limit`. Only a consistent reading is kept,
/// so a malformed response is never shown as an empty quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    pub limit: u64,
    pub remaining: u64,
    pub used: u64,
    /// Unix seconds at which the quota resets.
    pub reset: i64,
}

impl RateLimit {
    pub(crate) fn parse(body: &Value) -> Result<Self, GitHubError> {
        let core = &body["resources"]["core"];
        let field = |key: &str| core[key].as_u64().ok_or(GitHubError::InvalidData);
        let limit = field("limit")?;
        let remaining = field("remaining")?;
        let used = field("used")?;
        let reset = core["reset"].as_i64().ok_or(GitHubError::InvalidData)?;
        if limit == 0 || remaining > limit {
            return Err(GitHubError::InvalidData);
        }
        Ok(Self {
            limit,
            remaining,
            used,
            reset,
        })
    }

    /// The share of the quota still available, from 0 to 1.
    pub fn ratio(&self) -> f32 {
        self.remaining as f32 / self.limit as f32
    }

    /// Electron's `formatResetTime`: whole minutes rounded up, or `now`.
    pub fn resets_in(&self, now_seconds: i64) -> String {
        let diff = self.reset.saturating_sub(now_seconds);
        if diff <= 0 {
            "now".into()
        } else {
            format!("{}m", (diff as u64).div_ceil(60))
        }
    }
}

impl GitHubClient {
    /// Reading `/rate_limit` does not count against the quota it reports.
    /// It is always read from GitHub: the response cache is kept per
    /// namespace, while the quota belongs to the login, so a cached reading
    /// from one namespace could be older than one read in another. The app's
    /// per-login tracker decides when a reading is fresh enough to reuse.
    pub async fn rate_limit(&self) -> Result<Loaded<RateLimit>, GitHubError> {
        let loaded = self.request(ApiRequest::get("/rate_limit"), true).await?;
        Ok(Loaded {
            value: RateLimit::parse(&loaded.value.body)?,
            from_cache: loaded.from_cache,
            age: loaded.age,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(limit: Value, remaining: Value) -> Value {
        json!({"resources":{"core":{"limit":limit,"remaining":remaining,"used":12,"reset":1_000}}})
    }

    #[test]
    fn a_rate_limit_reads_the_core_quota_and_rejects_inconsistent_readings() {
        let reading = RateLimit::parse(&body(json!(5000), json!(4988))).unwrap();
        assert_eq!(
            reading,
            RateLimit {
                limit: 5000,
                remaining: 4988,
                used: 12,
                reset: 1_000
            }
        );
        assert!((reading.ratio() - 0.9976).abs() < 1e-4);
        // A missing, negative or impossible reading is unknown, never zero.
        for (limit, remaining) in [
            (json!(0), json!(0)),
            (json!(10), json!(11)),
            (json!(5000), json!(-1)),
            (json!(5000), Value::Null),
            (json!("5000"), json!(1)),
        ] {
            assert_eq!(
                RateLimit::parse(&body(limit, remaining)),
                Err(GitHubError::InvalidData)
            );
        }
        assert_eq!(
            RateLimit::parse(&json!({"rate":{"limit":5000}})),
            Err(GitHubError::InvalidData)
        );
    }

    #[derive(Default)]
    struct Quota {
        reads: std::sync::atomic::AtomicU64,
    }

    impl crate::github::Transport for Quota {
        fn request<'a>(
            &'a self,
            _: &'a crate::github::AccountScope,
            _: ApiRequest,
        ) -> crate::github::transport::ApiFuture<'a> {
            let reads = self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                Ok(crate::github::ApiResponse {
                    status: 200,
                    headers: Default::default(),
                    body: body(json!(5000), json!(4000 - reads)),
                })
            })
        }
    }

    #[tokio::test]
    async fn the_quota_is_always_read_from_github_never_from_the_namespace_cache() {
        let quota = std::sync::Arc::new(Quota::default());
        let clients = crate::github::GitHubClients::with_transport(quota.clone());
        let team = clients.for_account(crate::github::AccountScope::new("first", "team").unwrap());
        let solo = clients.for_account(crate::github::AccountScope::new("first", "solo").unwrap());
        let first = team.rate_limit().await.unwrap();
        assert!(!first.from_cache && first.age.is_zero());
        assert_eq!(first.value.remaining, 4000);
        // Another namespace spends quota; re-reading the first namespace
        // inside the cache's lifetime still asks GitHub for the newest value.
        assert_eq!(solo.rate_limit().await.unwrap().value.remaining, 3999);
        let again = team.rate_limit().await.unwrap();
        assert!(!again.from_cache && again.age.is_zero());
        assert_eq!(again.value.remaining, 3998);
        assert_eq!(quota.reads.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn the_reset_label_rounds_up_to_whole_minutes_like_electron() {
        let reading = RateLimit::parse(&body(json!(60), json!(30))).unwrap();
        assert_eq!(reading.resets_in(1_000), "now");
        assert_eq!(reading.resets_in(2_000), "now");
        assert_eq!(reading.resets_in(999), "1m");
        assert_eq!(reading.resets_in(940), "1m");
        assert_eq!(reading.resets_in(939), "2m");
        assert_eq!(reading.resets_in(-2_600), "60m");
    }
}
