use serde_json::json;

use super::*;
use crate::tempo::{AccountId, DateRange, Worklog, parse_worklog, summarize};

fn issue(id: u64) -> IssueId {
    IssueId::parse(id).unwrap()
}
fn row(id: u64, issue_id: u64, seconds: u64) -> Worklog {
    let raw = json!({"tempoWorklogId":id,"issue":{"id":issue_id,"self":format!("https://api.tempo.io/4/issues/{issue_id}")},"author":{"accountId":"synthetic-user"},"timeSpentSeconds":seconds,"startDate":"2026-10-17","createdAt":"2026-10-17T01:00:00Z","updatedAt":"2026-10-17T02:00:00Z","attributes":{"self":format!("https://api.tempo.io/4/worklogs/{id}/work-attribute-values"),"values":[]},"self":format!("https://api.tempo.io/4/worklogs/{id}")});
    parse_worklog(&serde_json::to_vec(&raw).unwrap()).unwrap()
}
fn summary(rows: &[Worklog]) -> TimesheetSummary {
    let date = "2026-10-17".parse().unwrap();
    summarize(
        rows,
        &AccountId::parse("synthetic-user").unwrap(),
        DateRange::new(date, date).unwrap(),
        &[],
    )
    .unwrap()
}

#[test]
fn exact_seconds_are_split_without_per_worklog_rounding() {
    let summary = summary(&[row(1, 7, 3601), row(2, 8, 3599), row(3, 7, 1)]);
    let values = HashMap::from([(issue(7), Some(true)), (issue(8), Some(false))]);
    let totals = summary.capitalization_totals(&values).unwrap();
    assert_eq!(summary.logged_seconds, 7201);
    assert_eq!(totals.capitalized_seconds, 3602);
    assert_eq!(totals.non_capitalized_seconds, 3599);
    assert_eq!(totals.unclassified_seconds, 0);
    assert!(totals.all_classified());
}

#[test]
fn absent_failed_or_unresolved_lookup_never_becomes_non_capex() {
    let summary = summary(&[row(1, 7, 3601), row(2, 8, 3599), row(3, 9, 2)]);
    let values = HashMap::from([(issue(7), Some(true)), (issue(8), None)]);
    let totals = summary.capitalization_totals(&values).unwrap();
    assert_eq!(totals.capitalized_seconds, 3601);
    assert_eq!(totals.non_capitalized_seconds, 0);
    assert_eq!(totals.unclassified_seconds, 3601);
    assert_eq!(totals.unclassified_worklogs, 2);
    assert!(!totals.all_classified());
}

#[test]
fn a_worklog_without_issue_identity_stays_unclassified() {
    let mut missing = row(1, 7, 61);
    missing.issue = None;
    let totals = summary(&[missing])
        .capitalization_totals(&HashMap::from([(issue(7), Some(false))]))
        .unwrap();
    assert_eq!(totals.non_capitalized_seconds, 0);
    assert_eq!(totals.unclassified_seconds, 61);
    assert_eq!(totals.unclassified_worklogs, 1);
}

#[test]
fn unrelated_classifications_do_not_change_empty_or_known_totals() {
    let values = HashMap::from([(issue(999), Some(true))]);
    let totals = summary(&[]).capitalization_totals(&values).unwrap();
    assert_eq!(totals.capitalized_seconds, 0);
    assert_eq!(totals.non_capitalized_seconds, 0);
    assert_eq!(totals.unclassified_seconds, 0);
    assert!(totals.all_classified());
    let totals = summary(&[row(1, 7, 90061)])
        .capitalization_totals(&values)
        .unwrap();
    assert_eq!(totals.unclassified_seconds, 90061);
}

#[test]
fn inconsistent_summary_is_rejected_instead_of_publishing_mismatched_cards() {
    let mut snapshot = summary(&[row(1, 7, 3600)]);
    snapshot.logged_seconds = 7200;
    assert!(matches!(
        snapshot.capitalization_totals(&HashMap::new()),
        Err(Error::InvalidResponse)
    ));
}
