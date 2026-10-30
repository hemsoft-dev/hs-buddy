//! Electron sums the four list badge counts, not unique open PRs.
use std::collections::HashSet;
use std::time::{Duration, Instant};

use buddy_core::config::{GitHubConfig, PrConfig};
use buddy_core::github::{AccountScope, GitHubClients, PrMode};
use gpui_kit::Context;

use super::DashboardView;
use crate::settings::Settings;

#[derive(Default)]
pub(super) struct ActivePrs {
    pub total: Option<u64>,
    pub error: Option<String>,
    pub loading: bool,
    inputs: Option<(GitHubConfig, PrConfig)>,
    attempted: Option<Instant>,
    generation: u64,
}

impl ActivePrs {
    pub(super) fn clear(&mut self) {
        *self = Self {
            generation: self.generation + 1,
            ..Default::default()
        };
    }

    fn begin(&mut self, github: &GitHubConfig, pr: &PrConfig, force: bool) -> Option<u64> {
        let changed = self
            .inputs
            .as_ref()
            .is_none_or(|old| old != &(github.clone(), pr.clone()));
        if changed {
            self.total = None;
            self.error = None;
        }
        let seconds = if pr.refresh_interval.is_finite() {
            (pr.refresh_interval * 60.0).clamp(60.0, 86400.0)
        } else {
            900.0
        };
        let due = pr.auto_refresh
            && self
                .attempted
                .is_some_and(|at| at.elapsed() >= Duration::from_secs_f64(seconds));
        if !changed && (self.loading || (!force && !due)) {
            return None;
        }
        self.inputs = Some((github.clone(), pr.clone()));
        self.attempted = Some(Instant::now());
        self.loading = true;
        self.generation += 1;
        Some(self.generation)
    }

    fn complete(&mut self, generation: u64, result: Result<u64, String>) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(total) => {
                self.total = Some(total);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

impl DashboardView {
    pub(super) fn refresh_active_prs(&mut self, force: bool, cx: &mut Context<Self>) {
        if !self.active || !self.card_visible(buddy_core::dashboard::CardId::WorkspacePulse, cx) {
            return;
        }
        let config = &Settings::global(cx).config;
        let github = config.github.clone();
        let pr = config.pr.clone();
        let Some(generation) = self.active_prs.begin(&github, &pr, force) else {
            return;
        };
        self.run(
            cx,
            count(GitHubClients::configured(), github, pr),
            move |this, result, _| {
                this.active_prs.complete(generation, result);
            },
        );
    }
}

async fn count(clients: GitHubClients, github: GitHubConfig, pr: PrConfig) -> Result<u64, String> {
    let mut accounts = HashSet::new();
    for account in github.accounts {
        accounts
            .insert(AccountScope::new(&account.username, &account.org).map_err(|e| e.to_string())?);
    }
    let mut total = 0;
    for mode in PrMode::ALL {
        let mut pulls = HashSet::new();
        for account in &accounts {
            let result = clients
                .for_account(account.clone())
                .pull_requests(
                    mode,
                    pr.recently_merged_days.clamp(1.0, 3650.0) as u32,
                    false,
                )
                .await
                .map_err(|e| e.to_string())?;
            // A partial list cannot establish a total. Keep any previous total
            // with a visible error rather than presenting an undercount as exact.
            if result.truncated || !result.warnings.is_empty() {
                return Err("Active PR total unavailable: a GitHub list is incomplete. Retry or open the PR lists.".into());
            }
            pulls.extend(result.items.into_iter().map(|pull| pull.target));
        }
        total += pulls.len() as u64;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy_core::config::GitHubAccount;

    struct Fixture {
        incomplete: bool,
        fail: bool,
    }
    impl buddy_core::github::Transport for Fixture {
        fn request<'a>(
            &'a self,
            _: &'a AccountScope,
            request: buddy_core::github::ApiRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            buddy_core::github::ApiResponse,
                            buddy_core::github::GitHubError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                assert!(!request.is_write);
                if self.fail {
                    return Err(buddy_core::github::GitHubError::Network);
                }
                let body = if request.path.starts_with("/users/") {
                    serde_json::json!({"type":"Organization"})
                } else {
                    serde_json::json!({"data":{"search":{
                        "issueCount":if self.incomplete {1001} else {1},
                        "pageInfo":{"hasNextPage":false,"endCursor":null},
                        "nodes":[{"number":1,"title":"Shared PR","state":"OPEN","createdAt":"2026-10-29","updatedAt":"2026-10-29",
                            "headRefName":"work","baseRefName":"main","headRefOid":"abc","changedFiles":1,"isDraft":false,
                            "author":{"login":"author"},"repository":{"nameWithOwner":"team/repo"},
                            "latestOpinion":{"nodes":[{"state":"APPROVED"}]},"reviewThreads":{"totalCount":0}}]
                    }}})
                };
                Ok(buddy_core::github::ApiResponse {
                    status: 200,
                    headers: Default::default(),
                    body,
                })
            })
        }
    }
    fn accounts() -> GitHubConfig {
        serde_json::from_value(serde_json::json!({"accounts":[
            {"username":"first","org":"team"},{"username":"second","org":"team"},
            {"username":"FIRST","org":"TEAM"}
        ]}))
        .unwrap()
    }
    #[tokio::test]
    async fn totals_dedupe_across_accounts_but_sum_overlapping_modes() {
        let clients = GitHubClients::with_transport(std::sync::Arc::new(Fixture {
            incomplete: false,
            fail: false,
        }));
        // Same PR in authored, merged and nudge; approved PR is excluded from Needs Review.
        assert_eq!(
            count(clients, accounts(), PrConfig::default())
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            count(
                GitHubClients::default(),
                GitHubConfig::default(),
                PrConfig::default()
            )
            .await
            .unwrap(),
            0
        );
    }
    #[tokio::test]
    async fn failed_or_incomplete_lists_never_become_zero_totals() {
        for fixture in [
            Fixture {
                incomplete: true,
                fail: false,
            },
            Fixture {
                incomplete: false,
                fail: true,
            },
        ] {
            assert!(
                count(
                    GitHubClients::with_transport(std::sync::Arc::new(fixture)),
                    accounts(),
                    PrConfig::default()
                )
                .await
                .is_err()
            );
        }
    }

    #[test]
    fn clearing_cache_rejects_inflight_result_even_after_reload() {
        let mut state = ActivePrs::default();
        let github = GitHubConfig::default();
        let pr = PrConfig::default();
        let old = state.begin(&github, &pr, false).unwrap();
        state.clear();
        let new = state.begin(&github, &pr, false).unwrap();
        state.complete(old, Ok(99));
        assert!(state.loading);
        assert_eq!(state.total, None);
        state.complete(new, Ok(0));
        assert_eq!(state.total, Some(0));
    }

    #[test]
    fn refresh_retains_last_total_on_failure_and_recovers() {
        let mut state = ActivePrs::default();
        let github = GitHubConfig::default();
        let pr = PrConfig::default();
        let first = state.begin(&github, &pr, false).unwrap();
        assert!(state.begin(&github, &pr, true).is_none());
        state.complete(first, Ok(12));
        assert!(state.begin(&github, &pr, false).is_none());
        let retry = state.begin(&github, &pr, true).unwrap();
        state.complete(retry, Err("offline".into()));
        assert_eq!(state.total, Some(12));
        assert_eq!(state.error.as_deref(), Some("offline"));
        let retry = state.begin(&github, &pr, true).unwrap();
        state.complete(retry, Ok(0));
        assert_eq!(state.total, Some(0));
        assert!(state.error.is_none());
    }

    #[test]
    fn account_change_clears_total_and_rejects_old_response() {
        let mut state = ActivePrs::default();
        let mut github = GitHubConfig::default();
        let pr = PrConfig::default();
        let old = state.begin(&github, &pr, false).unwrap();
        state.complete(old, Ok(8));
        github.accounts.push(GitHubAccount {
            username: "second".into(),
            org: "team".into(),
            repo_root: None,
            usage_provider: None,
            extra: Default::default(),
        });
        let new = state.begin(&github, &pr, false).unwrap();
        assert_eq!(state.total, None);
        state.complete(old, Ok(99));
        assert_eq!(state.total, None);
        assert!(state.loading);
        state.complete(new, Ok(3));
        assert_eq!(state.total, Some(3));
    }

    #[test]
    fn automatic_refresh_respects_off_and_retries_after_interval() {
        let mut state = ActivePrs::default();
        let github = GitHubConfig::default();
        let mut pr = PrConfig {
            auto_refresh: false,
            ..Default::default()
        };
        let generation = state.begin(&github, &pr, false).unwrap();
        state.complete(generation, Ok(2));
        state.attempted = Some(Instant::now() - Duration::from_secs(3600));
        assert!(state.begin(&github, &pr, false).is_none());
        pr.auto_refresh = true;
        let generation = state.begin(&github, &pr, false).unwrap();
        state.complete(generation, Ok(4));
        state.attempted = Some(Instant::now() - Duration::from_secs(3600));
        assert!(state.begin(&github, &pr, false).is_some());
    }
}
