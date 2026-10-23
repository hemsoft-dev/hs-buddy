//! Progress read from loop output: the phase, iteration and counters the dashboard shows.
//!
//! The markers are the lines the bundled scripts print (`ralph.ps1`, `ralph-pr.ps1`,
//! `ralph-issues.ps1`). Unknown lines change nothing, so a template that prints its own
//! format still runs; it only shows less progress.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    #[default]
    Initializing,
    Iterating,
    Scanning,
    PrHandoff,
    PrResolving,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub checks: u32,
    pub agent_turns: u32,
    pub reviews: u32,
    pub copilot_prs: u32,
    pub issues_created: u32,
    pub scan_iterations: u32,
    /// The latest cumulative cost as printed, for example `$1.24`.
    pub total_cost: Option<String>,
    pub total_premium: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: Phase,
    /// The iteration the loop reported last, 1-based.
    pub iteration: Option<u32>,
    pub stats: Stats,
    /// The branch the loop works on, as it prints it. ralph.ps1 adds a random suffix to
    /// a requested branch, so this is the name to look for on GitHub.
    #[serde(default)]
    pub branch: Option<String>,
    /// Cost of the finished cycles, in millionths of a dollar, and their premium requests.
    /// Every cycle is a fresh script that reports its own total from zero.
    #[serde(default)]
    pub earlier_cost: u64,
    #[serde(default)]
    pub earlier_premium: u32,
    /// The current cycle's latest total.
    #[serde(default)]
    pub cycle_cost: u64,
    #[serde(default)]
    pub cycle_premium: u32,
}

/// Removes terminal escape sequences (CSI and OSC) and other control characters except tabs.
pub fn strip_ansi(line: &str) -> String {
    let mut text = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    // Parameters and intermediates end at a final byte from @ to ~.
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // An operating system command ends with BEL or ESC \.
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else if c == '\t' || !c.is_control() {
            text.push(c);
        }
    }
    text
}

/// The number after a `==` banner word, as in `== Iteration 2/5` or `=== ITERATION 2`.
fn banner(line: &str, word: &str) -> Option<u32> {
    let rest = line.trim_start();
    let equals = rest.len() - rest.trim_start_matches('=').len();
    if equals < 2 {
        return None;
    }
    let rest = rest[equals..].trim_start();
    let head = rest.get(..word.len())?;
    if !head.eq_ignore_ascii_case(word) {
        return None;
    }
    let rest = &rest[word.len()..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    leading_number(rest.trim_start())
}

fn leading_number(text: &str) -> Option<u32> {
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    text[..digits].parse().ok()
}

/// The amount at the start of `$`-less text such as `1.24  (31 premium`.
fn amount(text: &str) -> Option<u64> {
    let end = text.len()
        - text
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
            .len();
    dollars(&text[..end])
}

/// `$1.20  (30 x $0.04)` after ralph.ps1's `Est. cost:` label.
fn estimate(rest: &str) -> Option<(u64, u32)> {
    let rest = rest.trim_start().strip_prefix('$')?;
    let micros = amount(rest)?;
    let count = rest.split_once('(')?.1.trim_start();
    let premium = leading_number(count)?;
    count[premium.to_string().len()..]
        .trim_start()
        .starts_with('x')
        .then_some((micros, premium))
}

/// `$1.24  (31 premium` after a cost label, as millionths of a dollar and a count.
fn cost(rest: &str) -> Option<(u64, u32)> {
    let rest = rest.trim_start().strip_prefix('$')?;
    let amount = rest.len()
        - rest
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
            .len();
    let micros = dollars(&rest[..amount])?;
    let premium = rest[amount..].trim_start().strip_prefix('(')?;
    let count = leading_number(premium)?;
    let digits = count.to_string().len();
    premium[digits..]
        .trim_start()
        .starts_with("premium")
        .then_some((micros, count))
}

/// `1.24` as millionths of a dollar; digits past the sixth decimal are dropped.
fn dollars(text: &str) -> Option<u64> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() || fraction.contains('.') {
        return None;
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let mut fraction = fraction.chars().take(6).collect::<String>();
    while fraction.len() < 6 {
        fraction.push('0');
    }
    whole
        .checked_mul(1_000_000)?
        .checked_add(fraction.parse().ok()?)
}

/// Millionths of a dollar as `$1.24`, rounded to the cent.
fn money(micros: u64) -> String {
    let cents = micros.saturating_add(5_000) / 10_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

impl Progress {
    /// Applies one stdout line; returns whether anything changed.
    pub fn observe(&mut self, line: &str) -> bool {
        let before = self.clone();
        let trimmed = line.trim();
        if let Some(branch) = trimmed
            .trim_start_matches('=')
            .trim_start()
            .strip_prefix("Branch:")
            .map(str::trim)
            .filter(|branch| !branch.is_empty() && !branch.contains(char::is_whitespace))
        {
            self.branch = Some(branch.to_owned());
        } else if let Some(scan) = banner(trimmed, "Scan Iteration") {
            self.phase = Phase::Scanning;
            self.iteration = Some(scan);
            self.stats.scan_iterations = self.stats.scan_iterations.max(scan);
        } else if let Some(iteration) = banner(trimmed, "Iteration") {
            self.phase = Phase::Iterating;
            self.iteration = Some(iteration);
        } else if banner(trimmed, "Check").is_some() {
            self.stats.checks += 1;
        } else if let Some(rest) = trimmed.strip_prefix("== Issues created this iteration:") {
            self.stats.issues_created += leading_number(rest.trim_start()).unwrap_or(0);
        } else if trimmed.contains("Handing off to ralph-pr") {
            self.phase = Phase::PrHandoff;
        } else if trimmed.contains("PR review cycle") || trimmed.contains("Checking CI status") {
            self.phase = Phase::PrResolving;
        } else if trimmed.starts_with("AGENT REVIEW [")
            || (trimmed.starts_with("Agent ") && trimmed.contains(" review done"))
        {
            self.stats.agent_turns += 1;
        } else if trimmed.contains("Copilot review requested")
            || trimmed.contains("request-copilot-review")
        {
            self.stats.copilot_prs += 1;
        } else if trimmed.contains("PR quality review") || review_round(trimmed) {
            self.stats.reviews += 1;
        } else if let Some(rest) = trimmed.strip_prefix("GRAND TOTAL:") {
            // ralph-pr.ps1's total; inside a Ralph Loop, the PR phase only.
            self.apply_cost(rest);
        } else if let Some(rest) = trimmed.strip_prefix("Cost")
            // The running total; per-round lines read `(n premium x $rate)`.
            && trimmed.contains("premium requests)")
        {
            self.apply_cost(rest);
        } else if let Some(rest) = trimmed.strip_prefix("Est. cost:") {
            // ralph.ps1's work iterations, `$1.20  (30 x $0.04)`, or ralph-issues.ps1's
            // scan summary, `$1.20` after its `Premium reqs:` line.
            match estimate(rest) {
                Some((micros, premium)) => self.apply(Some(micros), Some(premium)),
                None => self.apply(rest.trim_start().strip_prefix('$').and_then(amount), None),
            }
        } else if let Some(rest) = trimmed.strip_prefix("Premium reqs:") {
            self.apply(None, leading_number(rest.trim_start()));
        } else if let Some(rest) = trimmed.strip_prefix("Grand total premium:") {
            // ralph.ps1's work and PR phases together.
            self.apply(None, leading_number(rest.trim_start()));
        } else if let Some(rest) = trimmed.strip_prefix("Grand total cost:") {
            self.apply(rest.trim_start().strip_prefix('$').and_then(amount), None);
        }
        *self != before
    }

    fn apply_cost(&mut self, rest: &str) {
        if let Some((micros, premium)) = cost(rest) {
            self.apply(Some(micros), Some(premium));
        }
    }

    /// A total the current cycle's scripts printed. A Ralph Loop prints its PR phase's
    /// total, then its work iterations', then both together; every one covers part of
    /// the cycle's usage and the last covers all of it, so the cycle keeps the largest.
    fn apply(&mut self, micros: Option<u64>, premium: Option<u32>) {
        if let Some(micros) = micros {
            self.cycle_cost = self.cycle_cost.max(micros);
            self.stats.total_cost = Some(money(self.earlier_cost.saturating_add(self.cycle_cost)));
        }
        if let Some(premium) = premium {
            self.cycle_premium = self.cycle_premium.max(premium);
            self.stats.total_premium =
                Some(self.earlier_premium.saturating_add(self.cycle_premium));
        }
    }

    /// Starts another cycle: its script reports from zero, so the totals so far carry over.
    pub fn next_cycle(&mut self) {
        self.earlier_cost = self.earlier_cost.saturating_add(self.cycle_cost);
        self.earlier_premium = self.earlier_premium.saturating_add(self.cycle_premium);
        self.cycle_cost = 0;
        self.cycle_premium = 0;
    }
}

fn review_round(line: &str) -> bool {
    line.match_indices("review round ")
        .any(|(index, word)| leading_number(&line[index + word.len()..]).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(lines: &[&str]) -> Progress {
        let mut progress = Progress::default();
        for line in lines {
            progress.observe(&strip_ansi(line));
        }
        progress
    }

    #[test]
    fn the_bundled_iteration_banner_advances_progress() {
        // ralph.ps1 prints `== Iteration i/max`; Electron's `=== ITERATION` never matched it.
        let progress = read(&["== Iteration 1/3", "work", "== Iteration 2/3"]);
        assert_eq!(progress.phase, Phase::Iterating);
        assert_eq!(progress.iteration, Some(2));
        assert_eq!(read(&["=== ITERATION 4"]).iteration, Some(4));
        assert_eq!(
            read(&["== Iteration 2/unlimited completed in 3m"]).iteration,
            Some(2)
        );
        assert_eq!(
            read(&["Iteration 2 notes", "= Iteration 3"]).iteration,
            None
        );
    }

    #[test]
    fn scans_checks_handoff_and_reviews_are_counted() {
        let progress = read(&[
            "== Scan Iteration 1/5",
            "== Scan Iteration 2/5",
            "== Issues created this iteration: 3  |  Total: 3",
            "== Issues created this iteration: 2  |  Total: 5",
        ]);
        assert_eq!(progress.phase, Phase::Scanning);
        assert_eq!(progress.stats.scan_iterations, 2);
        assert_eq!(progress.iteration, Some(2));
        assert_eq!(progress.stats.issues_created, 5);
        let progress = read(&[
            "Handing off to ralph-pr.ps1 for PR #7...",
            "[12:00:01] Checking CI status and PR comments...",
            "== Check 1  |  Elapsed: 0m",
            "== Check 2  |  Elapsed: 3m",
            "  AGENT REVIEW [1/2]",
            "Copilot review requested",
            "Starting review round 2",
        ]);
        assert_eq!(progress.phase, Phase::PrResolving);
        assert_eq!(progress.stats.checks, 2);
        assert_eq!(progress.stats.agent_turns, 1);
        assert_eq!(progress.stats.copilot_prs, 1);
        assert_eq!(progress.stats.reviews, 1);
        assert_eq!(read(&["AGENT REVIEW FLOW"]).stats.agent_turns, 0);
    }

    #[test]
    fn only_running_and_grand_totals_set_the_cost() {
        let progress = read(&["  Cost           $0.16  (4 premium x $0.04)"]);
        assert_eq!(progress.stats.total_cost, None);
        let progress = read(&[
            "  Cost           $1.2  (30 premium requests)",
            "  GRAND TOTAL:         $1.36  (34 premium)",
        ]);
        assert_eq!(progress.stats.total_cost.as_deref(), Some("$1.36"));
        assert_eq!(progress.stats.total_premium, Some(34));
        assert_eq!(
            read(&["  Cost           $1.2  (30 premium requests)"])
                .stats
                .total_cost
                .as_deref(),
            Some("$1.20")
        );
    }

    #[test]
    fn a_ralph_loop_cycle_counts_its_work_and_pr_phases() {
        // The order ralph.ps1 prints them: the PR phase (ralph-pr.ps1), then its own
        // work iterations, then both together.
        let cycle = [
            "  Cost           $0.24  (6 premium requests)",
            "  GRAND TOTAL:         $0.24  (6 premium)",
            "  Premium requests:    30",
            "  Est. cost:           $1.20  (30 x $0.04)",
            "  PR phase premium:    6",
            "  PR phase cost:       $0.24",
            "  Grand total premium: 36",
            "  Grand total cost:    $1.44",
        ];
        let mut progress = Progress::default();
        let mut shown = vec![];
        for line in cycle {
            progress.observe(line);
            shown.push(progress.stats.total_premium.unwrap_or(0));
        }
        assert_eq!(progress.stats.total_cost.as_deref(), Some("$1.44"));
        assert_eq!(progress.stats.total_premium, Some(36));
        assert!(shown.windows(2).all(|pair| pair[0] <= pair[1]), "{shown:?}");
        progress.next_cycle();
        for line in cycle {
            progress.observe(line);
        }
        assert_eq!(progress.stats.total_cost.as_deref(), Some("$2.88"));
        assert_eq!(progress.stats.total_premium, Some(72));
        // A work-only cycle (no pull request) ends with its own estimate.
        let progress = read(&["  Est. cost:           $0.12  (3 x $0.04)"]);
        assert_eq!(progress.stats.total_cost.as_deref(), Some("$0.12"));
        assert_eq!(progress.stats.total_premium, Some(3));
    }

    #[test]
    fn a_ralph_issues_scan_summary_sets_cost_and_premium_requests() {
        // ralph-issues.ps1's stdout summary, as printed.
        let progress = read(&[
            "== RALPH ISSUES SCAN SUMMARY",
            "  Duration:        4m 2s",
            "  Iterations:      3 / 3",
            "  Issues created:  2",
            "  Premium reqs:    30",
            "  Est. cost:       $1.2",
        ]);
        assert_eq!(progress.stats.total_cost.as_deref(), Some("$1.20"));
        assert_eq!(progress.stats.total_premium, Some(30));
    }

    #[test]
    fn repeat_cycles_add_up_cost_and_premium_requests() {
        let mut progress = read(&["  GRAND TOTAL:         $1.36  (34 premium)"]);
        progress.next_cycle();
        // The next cycle's script starts from zero; the run's totals never go down.
        progress.observe("  Cost           $0.04  (1 premium requests)");
        assert_eq!(progress.stats.total_cost.as_deref(), Some("$1.40"));
        assert_eq!(progress.stats.total_premium, Some(35));
        progress.observe("  GRAND TOTAL:         $0.5  (12 premium)");
        assert_eq!(progress.stats.total_cost.as_deref(), Some("$1.86"));
        assert_eq!(progress.stats.total_premium, Some(46));
        progress.next_cycle();
        progress.next_cycle();
        assert_eq!(progress.stats.total_cost.as_deref(), Some("$1.86"));
        progress.observe("  GRAND TOTAL:         $.255  (2 premium)");
        assert_eq!(progress.stats.total_cost.as_deref(), Some("$2.12"));
        assert_eq!(progress.stats.total_premium, Some(48));
    }

    #[test]
    fn escape_sequences_and_controls_are_removed_but_text_is_kept() {
        assert_eq!(
            strip_ansi("\u{1b}[1;32m== Iteration 1/3\u{1b}[0m"),
            "== Iteration 1/3"
        );
        assert_eq!(
            strip_ansi("\u{1b}]0;title\u{7}after\u{1b}]8;;x\u{1b}\\link"),
            "afterlink"
        );
        assert_eq!(strip_ansi("tab\there\u{0}→ é"), "tab\there→ é");
        assert_eq!(strip_ansi("cut\u{1b}"), "cut");
    }

    #[test]
    fn the_branch_the_loop_prints_is_recorded() {
        assert_eq!(
            read(&["  Branch:          fix/issue-12-1a2b3c4d"])
                .branch
                .as_deref(),
            Some("fix/issue-12-1a2b3c4d")
        );
        assert_eq!(
            read(&["== Branch: feature/ralph-0f0f0f0f"])
                .branch
                .as_deref(),
            Some("feature/ralph-0f0f0f0f")
        );
        assert_eq!(read(&["Branch: not a name"]).branch, None);
    }

    #[test]
    fn unrelated_lines_change_nothing() {
        let mut progress = Progress::default();
        assert!(!progress.observe("Compiling crate v1.0"));
        assert!(!progress.observe(""));
        assert!(progress.observe("== Iteration 1/1"));
        assert!(!progress.observe("== Iteration 1/1"));
    }
}
