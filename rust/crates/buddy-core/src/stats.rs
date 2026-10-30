//! Buddy usage statistics (the Convex `buddyStats` singleton) and the derived
//! Workspace Pulse figures from `WelcomePanel.tsx`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BuddyStats {
    pub app_launches: u64,
    pub tabs_opened: u64,
    pub prs_viewed: u64,
    pub prs_reviewed: u64,
    pub prs_merged_watched: u64,
    pub repos_browsed: u64,
    pub repo_detail_views: u64,
    pub jobs_created: u64,
    pub runs_triggered: u64,
    pub runs_completed: u64,
    pub runs_failed: u64,
    pub schedules_created: u64,
    pub bookmarks_created: u64,
    pub settings_changed: u64,
    pub searches_performed: u64,
    pub copilot_pr_reviews: u64,
    /// Epoch milliseconds; 0 means unknown ("Today").
    pub first_launch_date: u64,
    pub total_uptime_ms: u64,
    pub last_session_start: Option<u64>,
}

fn number(value: &Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| n.round() as u64)
        .unwrap_or(0)
}

impl BuddyStats {
    /// Build from the exported Convex document. Convex numbers arrive as
    /// floats, and fields added later may be missing.
    pub fn from_json(value: &Value) -> Self {
        Self {
            app_launches: number(value, "appLaunches"),
            tabs_opened: number(value, "tabsOpened"),
            prs_viewed: number(value, "prsViewed"),
            prs_reviewed: number(value, "prsReviewed"),
            prs_merged_watched: number(value, "prsMergedWatched"),
            repos_browsed: number(value, "reposBrowsed"),
            repo_detail_views: number(value, "repoDetailViews"),
            jobs_created: number(value, "jobsCreated"),
            runs_triggered: number(value, "runsTriggered"),
            runs_completed: number(value, "runsCompleted"),
            runs_failed: number(value, "runsFailed"),
            schedules_created: number(value, "schedulesCreated"),
            bookmarks_created: number(value, "bookmarksCreated"),
            settings_changed: number(value, "settingsChanged"),
            searches_performed: number(value, "searchesPerformed"),
            copilot_pr_reviews: number(value, "copilotPrReviews"),
            first_launch_date: number(value, "firstLaunchDate"),
            total_uptime_ms: number(value, "totalUptimeMs"),
            last_session_start: value
                .get("lastSessionStart")
                .and_then(Value::as_f64)
                .map(|n| n.round() as u64),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspacePulse {
    pub total_prs_viewed: u64,
    /// Sum of the four PR list counts, or `None` before a complete load.
    pub active_prs: Option<u64>,
    pub copilot_pr_reviews: u64,
    pub repos_browsed: u64,
    pub runs_triggered: u64,
    pub total_finished: u64,
    /// Whole percent of finished runs that completed.
    pub success_rate: u64,
    pub bookmarks: u64,
    pub first_launch_ms: u64,
    pub app_launches: u64,
}

impl WorkspacePulse {
    pub fn from_stats(stats: &BuddyStats, active_prs: Option<u64>, bookmarks: u64) -> Self {
        let total_finished = stats.runs_completed.saturating_add(stats.runs_failed);
        let success_rate = if total_finished > 0 {
            ((stats.runs_completed as f64 / total_finished as f64) * 100.0).round() as u64
        } else {
            0
        };
        Self {
            total_prs_viewed: stats
                .prs_viewed
                .saturating_add(stats.prs_reviewed)
                .saturating_add(stats.prs_merged_watched),
            active_prs,
            copilot_pr_reviews: stats.copilot_pr_reviews,
            repos_browsed: stats.repos_browsed,
            runs_triggered: stats.runs_triggered,
            total_finished,
            success_rate,
            bookmarks,
            first_launch_ms: stats.first_launch_date,
            app_launches: stats.app_launches,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_pulse_like_welcome_panel() {
        let stats = BuddyStats {
            prs_viewed: 10,
            prs_reviewed: 5,
            prs_merged_watched: 1,
            runs_completed: 3,
            runs_failed: 1,
            ..Default::default()
        };
        let pulse = WorkspacePulse::from_stats(&stats, Some(7), 4);
        assert_eq!(pulse.total_prs_viewed, 16);
        assert_eq!(pulse.active_prs, Some(7));
        assert_eq!(pulse.total_finished, 4);
        assert_eq!(pulse.success_rate, 75);
        assert_eq!(pulse.bookmarks, 4);
    }

    #[test]
    fn reads_convex_export_with_float_numbers() {
        let json = serde_json::json!({
            "_id": "abc",
            "_creationTime": 1.7e12,
            "key": "default",
            "appLaunches": 212.0,
            "prsViewed": 1480.0,
            "totalUptimeMs": 275400000.0,
            "lastSessionStart": 1759800000000.0
        });
        let stats = BuddyStats::from_json(&json);
        assert_eq!(stats.app_launches, 212);
        assert_eq!(stats.prs_viewed, 1480);
        assert_eq!(stats.total_uptime_ms, 275_400_000);
        assert_eq!(stats.last_session_start, Some(1_759_800_000_000));
        assert_eq!(stats.copilot_pr_reviews, 0);
    }

    #[test]
    fn success_rate_is_zero_without_runs() {
        let pulse = WorkspacePulse::from_stats(&BuddyStats::default(), None, 0);
        assert_eq!(pulse.success_rate, 0);
        assert_eq!(pulse.active_prs, None);

        let huge = BuddyStats {
            runs_completed: u64::MAX,
            runs_failed: 1,
            ..Default::default()
        };
        assert_eq!(
            WorkspacePulse::from_stats(&huge, None, 0).total_finished,
            u64::MAX
        );
    }
}
