use serde_json::json;

use super::*;

fn project(id: u64) -> JiraProjectId {
    JiraProjectId::parse(id).unwrap()
}
fn result(
    connection: WriteScope,
    project_id: u64,
    key: &str,
    status: &str,
) -> ProjectBillingAccounts {
    let page = json!({"metadata":{"count":1,"limit":50,"offset":0},"results":[{"id":1,"key":key,"name":"Synthetic billing account","status":status,"global":false}]});
    let (pages, complete) = super::super::accounts::AccountPages::new()
        .push(&serde_json::to_vec(&page).unwrap())
        .unwrap();
    assert!(complete);
    ProjectBillingAccounts {
        connection,
        project: project(project_id),
        choices: pages
            .finish()
            .into_iter()
            .map(|account| ProjectBillingAccount {
                account,
                is_default: true,
            })
            .collect(),
    }
}

#[test]
fn initial_edit_or_prefill_account_survives_a_late_default() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, Some("INITIAL")).unwrap();
    let lookup = state.begin(project(7));
    assert!(
        state
            .loaded(lookup, result(connection, 7, "DEFAULT", "OPEN"))
            .unwrap()
    );
    assert_eq!(state.selected_key(), Some("INITIAL"));
    assert!(!state.selection_is_listed());
    assert_eq!(state.choices()[0].account.key(), "DEFAULT");
}

#[test]
fn editing_an_unassigned_worklog_does_not_assign_a_project_default() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::for_edit(connection, None).unwrap();
    let lookup = state.begin(project(7));
    state
        .loaded(lookup, result(connection, 7, "DEFAULT", "OPEN"))
        .unwrap();
    assert_eq!(state.selected_key(), None);
    assert_eq!(state.choices().len(), 1);
}

#[test]
fn manual_selection_and_explicit_clear_survive_a_late_default() {
    for selected in [Some("MANUAL"), None] {
        let connection = WriteScope::new();
        let mut state = BillingSelection::new(connection, None).unwrap();
        let lookup = state.begin(project(7));
        state.select(selected).unwrap();
        assert!(
            state
                .loaded(lookup, result(connection, 7, "DEFAULT", "OPEN"))
                .unwrap()
        );
        assert_eq!(state.selected_key(), selected);
    }
}

#[test]
fn obsolete_success_and_failure_cannot_clear_or_replace_the_current_project_load() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, None).unwrap();
    let old = state.begin(project(7));
    let current = state.begin(project(8));
    assert!(!state.failed(old));
    assert!(
        !state
            .loaded(old, result(connection, 7, "OLD", "OPEN"))
            .unwrap()
    );
    assert!(state.is_loading() && state.choices().is_empty());
    assert!(
        state
            .loaded(current, result(connection, 8, "CURRENT", "OPEN"))
            .unwrap()
    );
    assert_eq!(state.selected_key(), Some("CURRENT"));
    assert!(state.selection_is_listed() && !state.is_loading());
    assert!(
        !state
            .loaded(current, result(connection, 8, "REPLAY", "OPEN"))
            .unwrap()
    );
    assert_eq!(state.selected_key(), Some("CURRENT"));
}

#[test]
fn newer_same_project_request_also_rejects_an_older_completion() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, None).unwrap();
    let old = state.begin(project(7));
    let current = state.begin(project(7));
    assert!(
        !state
            .loaded(old, result(connection, 7, "OLD", "OPEN"))
            .unwrap()
    );
    assert!(state.is_loading());
    assert!(
        state
            .loaded(current, result(connection, 7, "NEW", "OPEN"))
            .unwrap()
    );
    assert_eq!(state.selected_key(), Some("NEW"));
}

#[test]
fn foreign_result_keeps_the_draft_and_cannot_clear_another_connections_load() {
    let connection = WriteScope::new();
    let other = WriteScope::new();
    let mut state = BillingSelection::new(connection, Some("RETAINED")).unwrap();
    let lookup = state.begin(project(7));
    assert!(matches!(
        state.loaded(lookup, result(other, 7, "OTHER", "OPEN")),
        Err(Error::WrongConnection)
    ));
    assert_eq!(state.selected_key(), Some("RETAINED"));
    assert!(state.choices().is_empty() && !state.is_loading());
    let mut other_state = BillingSelection::new(other, None).unwrap();
    let other_lookup = other_state.begin(project(7));
    assert!(!other_state.failed(lookup));
    assert!(other_state.is_loading());
    assert!(
        other_state
            .loaded(other_lookup, result(other, 7, "OTHER", "OPEN"))
            .unwrap()
    );
}

#[test]
fn wrong_project_result_retains_the_selected_account_and_prior_choices() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, None).unwrap();
    let first = state.begin(project(7));
    state
        .loaded(first, result(connection, 7, "RETAINED", "OPEN"))
        .unwrap();
    let next = state.begin(project(7));
    assert!(matches!(
        state.loaded(next, result(connection, 8, "OTHER", "OPEN")),
        Err(Error::WrongScope)
    ));
    assert_eq!(state.selected_key(), Some("RETAINED"));
    assert_eq!(state.choices()[0].account.key(), "RETAINED");
}

#[test]
fn failed_refresh_retains_choices_and_an_explicit_recovery_can_replace_them() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, None).unwrap();
    let first = state.begin(project(7));
    state
        .loaded(first, result(connection, 7, "RETAINED", "OPEN"))
        .unwrap();
    let failed = state.begin(project(7));
    assert!(state.failed(failed));
    assert_eq!(state.selected_key(), Some("RETAINED"));
    assert_eq!(state.choices()[0].account.key(), "RETAINED");
    let recovery = state.begin(project(7));
    state
        .loaded(recovery, result(connection, 7, "RECOVERED", "OPEN"))
        .unwrap();
    assert_eq!(state.selected_key(), Some("RECOVERED"));
}

#[test]
fn closed_or_archived_defaults_are_displayed_without_automatic_selection() {
    for status in ["CLOSED", "ARCHIVED"] {
        let connection = WriteScope::new();
        let mut state = BillingSelection::new(connection, None).unwrap();
        let lookup = state.begin(project(7));
        state
            .loaded(lookup, result(connection, 7, "UNAVAILABLE", status))
            .unwrap();
        assert_eq!(state.selected_key(), None);
        assert_eq!(state.choices().len(), 1);
    }
}

#[test]
fn invalid_issue_clears_pending_defaults_but_preserves_a_manual_account() {
    for initial in [Some("MANUAL"), None] {
        let connection = WriteScope::new();
        let mut state = BillingSelection::new(connection, initial).unwrap();
        let lookup = state.begin(project(7));
        state.clear_project();
        assert!(
            !state
                .loaded(lookup, result(connection, 7, "LATE", "OPEN"))
                .unwrap()
        );
        assert_eq!(state.selected_key(), initial);
        assert!(!state.is_loading() && state.choices().is_empty());
    }
}

#[test]
fn invalid_manual_selection_preserves_the_existing_choice() {
    let mut state = BillingSelection::new(WriteScope::new(), Some("RETAINED")).unwrap();
    for invalid in ["", " ", "bad\nkey", &"x".repeat(257)] {
        assert!(matches!(
            state.select(Some(invalid)),
            Err(Error::InvalidDraft)
        ));
        assert_eq!(state.selected_key(), Some("RETAINED"));
    }
}
