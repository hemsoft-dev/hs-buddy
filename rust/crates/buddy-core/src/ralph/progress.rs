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

/// `$1.24  (31 premium` after a cost label.
fn cost(rest: &str) -> Option<(String, u32)> {
    let rest = rest.trim_start().strip_prefix('$')?;
    let amount = rest.len()
        - rest
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
            .len();
    if amount == 0 {
        return None;
    }
    let premium = rest[amount..].trim_start().strip_prefix('(')?;
    let count = leading_number(premium)?;
    let digits = count.to_string().len();
    premium[digits..]
        .trim_start()
        .starts_with("premium")
        .then(|| (format!("${}", &rest[..amount]), count))
}

impl Progress {
    /// Applies one stdout line; returns whether anything changed.
    pub fn observe(&mut self, line: &str) -> bool {
        let before = self.clone();
        let trimmed = line.trim();
        if let Some(scan) = banner(trimmed, "Scan Iteration") {
            self.phase = Phase::Scanning;
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
            self.apply_cost(rest);
        } else if let Some(rest) = trimmed.strip_prefix("Cost")
            // The running total; per-round lines read `(n premium x $rate)`.
            && trimmed.contains("premium requests)")
        {
            self.apply_cost(rest);
        }
        *self != before
    }

    fn apply_cost(&mut self, rest: &str) {
        if let Some((amount, premium)) = cost(rest) {
            self.stats.total_cost = Some(amount);
            self.stats.total_premium = Some(premium);
        }
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
    fn unrelated_lines_change_nothing() {
        let mut progress = Progress::default();
        assert!(!progress.observe("Compiling crate v1.0"));
        assert!(!progress.observe(""));
        assert!(progress.observe("== Iteration 1/1"));
        assert!(!progress.observe("== Iteration 1/1"));
    }
}
