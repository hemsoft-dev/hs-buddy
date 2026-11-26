use serde_json::json;

use super::*;

fn project(id: u64) -> JiraProjectId {
    JiraProjectId::parse(id).unwrap()
}
fn catalog(rows: &[(u64, &str, &str)]) -> Vec<BillingAccount> {
    let results: Vec<_> = rows
        .iter()
        .map(|(id, key, status)| json!({"id":id,"key":key,"name":format!("Synthetic billing account {key}"),"status":status,"global":false}))
        .collect();
    let page = json!({"metadata":{"count":results.len(),"limit":50,"offset":0},"results":results});
    let (pages, complete) = super::super::accounts::AccountPages::new()
        .push(&serde_json::to_vec(&page).unwrap())
        .unwrap();
    assert!(complete);
    pages.finish()
}
/// The project links only `key`. Its catalog also holds an unlinked account.
fn result(
    connection: WriteScope,
    project_id: u64,
    key: &str,
    status: &str,
) -> ProjectBillingAccounts {
    let catalog = catalog(&[(1, key, status), (2, "UNLINKED", "OPEN")]);
    ProjectBillingAccounts {
        connection,
        project: project(project_id),
        choices: vec![ProjectBillingAccount {
            account: catalog[0].clone(),
            is_default: true,
        }],
        catalog,
    }
}
fn keys(state: &BillingSelection) -> Vec<&str> {
    state
        .choices()
        .iter()
        .map(|choice| choice.account.key())
        .collect()
}

#[test]
fn an_edits_existing_account_survives_a_late_default_even_when_unlisted() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::for_edit(connection, Some("INITIAL")).unwrap();
    let lookup = state.begin(project(7));
    assert!(
        state
            .loaded(lookup, result(connection, 7, "DEFAULT", "OPEN"))
            .unwrap()
    );
    assert_eq!(state.selected_key(), Some("INITIAL"));
    assert!(!state.selection_is_listed());
    assert_eq!(state.choices()[0].account.key(), "DEFAULT");
    assert_eq!(state.sendable_key().unwrap(), Some("INITIAL"));
}

/// The project links DEFAULT (its default) and `key` with `status`.
fn two_links(connection: WriteScope, key: &str, status: &str) -> ProjectBillingAccounts {
    let mut listed = result(connection, 7, "DEFAULT", "OPEN");
    let other = catalog(&[(3, key, status)]).remove(0);
    listed.catalog.push(other.clone());
    listed.choices.push(ProjectBillingAccount {
        account: other,
        is_default: false,
    });
    listed
}

#[test]
fn an_earlier_pick_survives_a_lookup_only_where_the_fresh_list_has_it_open() {
    // (status in the fresh list, or None when it isn't listed, kept?)
    for (status, kept) in [
        (Some("OPEN"), true),
        (Some("CLOSED"), false),
        (Some("ARCHIVED"), false),
        (None, false),
    ] {
        let connection = WriteScope::new();
        for initial in [true, false] {
            // A create's initial value and a pick made while loading follow
            // the same rule.
            let mut state = if initial {
                BillingSelection::new(connection, Some("PICKED")).unwrap()
            } else {
                BillingSelection::new(connection, None).unwrap()
            };
            let lookup = state.begin(project(7));
            if !initial {
                state.select(Some("PICKED")).unwrap();
            }
            let listed = match status {
                Some(status) => two_links(connection, "PICKED", status),
                None => two_links(connection, "OTHER", "OPEN"),
            };
            assert!(state.loaded(lookup, listed).unwrap());
            if kept {
                assert_eq!(state.selected_key(), Some("PICKED"), "{status:?}");
                assert!(state.is_chosen());
                assert_eq!(state.create_account("INT-4").unwrap(), Some("PICKED"));
            } else {
                // Exactly as a fresh draft: the project default, unchosen,
                // so a later lookup takes the fresh default again.
                assert_eq!(state.selected_key(), Some("DEFAULT"), "{status:?}");
                assert!(!state.is_chosen(), "{status:?}");
                assert_eq!(state.create_account("INT-4").unwrap(), Some("DEFAULT"));
            }
        }
    }
    // With no open default, a dropped pick leaves the account unchosen.
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, Some("GONE")).unwrap();
    let lookup = state.begin(project(7));
    state
        .loaded(lookup, result(connection, 7, "DEFAULT", "CLOSED"))
        .unwrap();
    assert!(state.is_unchosen());
    // An edit falls back to its existing account, never to a default.
    let mut edit = BillingSelection::for_edit(connection, None).unwrap();
    edit.select(Some("GONE")).unwrap();
    let lookup = edit.begin(project(7));
    edit.loaded(lookup, two_links(connection, "GONE", "CLOSED"))
        .unwrap();
    assert_eq!(edit.selected_key(), None);
    assert!(edit.is_chosen());
}

#[test]
fn a_save_refuses_a_chosen_account_the_current_list_does_not_offer_as_open() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, None).unwrap();
    let lookup = state.begin(project(7));
    state
        .loaded(lookup, two_links(connection, "CLOSED-ONE", "CLOSED"))
        .unwrap();
    for refused in ["CLOSED-ONE", "UNLISTED"] {
        state.select(Some(refused)).unwrap();
        assert!(!state.offers(Some(refused)));
        assert!(matches!(
            state.create_account("INT-4"),
            Err(Error::UnavailableBillingAccount)
        ));
        assert!(matches!(
            state.sendable_key(),
            Err(Error::UnavailableBillingAccount)
        ));
    }
    state.select(Some("DEFAULT")).unwrap();
    assert_eq!(state.create_account("INT-4").unwrap(), Some("DEFAULT"));
    state.select(None).unwrap();
    assert_eq!(state.create_account("INT-4").unwrap(), None);
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
                .loaded(lookup, two_links(connection, "MANUAL", "OPEN"))
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

#[test]
fn an_unchosen_create_account_is_inferred_from_the_issue_key_prefix() {
    assert_eq!(inferred_account("INT-12"), "INT");
    assert_eq!(inferred_account("INT"), "INT");
    for key in ["FIX-17", "INTX-1", "PE-992", "int-12", ""] {
        assert_eq!(inferred_account(key), "GEN-DEV", "{key:?}");
    }
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, None).unwrap();
    let lookup = state.begin(project(7));
    let mut empty = result(connection, 7, "UNUSED", "OPEN");
    empty.choices.clear();
    assert!(state.loaded(lookup, empty).unwrap());
    assert!(state.is_unchosen());
    assert_eq!(state.selected_key(), None);
    assert_eq!(state.create_account("INT-4").unwrap(), Some("INT"));
    assert_eq!(state.create_account("FIX-17").unwrap(), Some("GEN-DEV"));

    let mut defaulted = BillingSelection::new(connection, None).unwrap();
    let lookup = defaulted.begin(project(7));
    defaulted
        .loaded(lookup, result(connection, 7, "DEFAULT", "OPEN"))
        .unwrap();
    assert!(!defaulted.is_unchosen());
    assert_eq!(defaulted.create_account("INT-4").unwrap(), Some("DEFAULT"));

    // An explicit "No billing account" choice is kept, never inferred.
    state.select(None).unwrap();
    assert!(!state.is_unchosen());
    assert_eq!(state.create_account("INT-4").unwrap(), None);
    state.select(Some("UNLINKED")).unwrap();
    assert_eq!(state.create_account("INT-4").unwrap(), Some("UNLINKED"));
}

#[test]
fn inference_never_picks_an_account_the_project_lists_as_closed_or_archived() {
    for (status, sent) in [("CLOSED", None), ("ARCHIVED", None), ("OPEN", Some("INT"))] {
        let connection = WriteScope::new();
        let mut state = BillingSelection::new(connection, None).unwrap();
        let lookup = state.begin(project(7));
        // A closed or archived default is never selected, so the account stays unchosen.
        let mut listed = result(connection, 7, "INT", status);
        listed.choices[0].is_default = false;
        state.loaded(lookup, listed).unwrap();
        assert!(state.is_unchosen(), "{status}");
        assert_eq!(state.create_account("INT-4").unwrap(), sent, "{status}");
        // An account the project doesn't list is still inferred, as in Electron.
        assert_eq!(
            state.create_account("FIX-17").unwrap(),
            Some("GEN-DEV"),
            "{status}"
        );
    }
}

#[test]
fn only_a_person_initial_value_or_edit_counts_as_a_chosen_account() {
    let connection = WriteScope::new();
    // A project default picked by the lookup is not a choice, so a repeated
    // lookup or connection recheck takes the fresh default instead.
    let mut defaulted = BillingSelection::new(connection, None).unwrap();
    let lookup = defaulted.begin(project(7));
    defaulted
        .loaded(lookup, result(connection, 7, "DEFAULT", "OPEN"))
        .unwrap();
    assert_eq!(defaulted.selected_key(), Some("DEFAULT"));
    assert!(!defaulted.is_chosen());
    defaulted.select(None).unwrap();
    assert!(defaulted.is_chosen());
    assert!(
        BillingSelection::new(connection, Some("TEMPLATE"))
            .unwrap()
            .is_chosen()
    );
    assert!(
        BillingSelection::for_edit(connection, None)
            .unwrap()
            .is_chosen()
    );
    assert!(!BillingSelection::new(connection, None).unwrap().is_chosen());
}

#[test]
fn every_account_is_listed_in_tempo_order_before_a_project_is_known() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, None).unwrap();
    let accounts = catalog(&[
        (9, "ZULU", "OPEN"),
        (3, "ALPHA", "CLOSED"),
        (5, "MIKE", "OPEN"),
    ]);
    assert!(
        state
            .offer_catalog(BillingCatalog {
                connection,
                accounts,
            })
            .unwrap()
    );
    assert!(state.lists_every_account() && state.project().is_none());
    // Tempo's order, as Electron's select lists it; no account is a default.
    assert_eq!(keys(&state), ["ZULU", "ALPHA", "MIKE"]);
    assert!(state.choices().iter().all(|choice| !choice.is_default));
    assert!(state.is_unchosen() && state.selected_key().is_none());
    // A closed account stays listed with its status, so the editor can
    // refuse it as it refuses a closed linked account.
    assert!(state.choices()[1].account.status() == BillingAccountStatus::Closed);
    assert!(state.lists(Some("MIKE")) && state.lists(None) && !state.lists(Some("GONE")));
}

#[test]
fn a_project_with_no_linked_accounts_lists_every_account_without_a_default() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, None).unwrap();
    let lookup = state.begin(project(7));
    let mut unlinked = result(connection, 7, "LINKED", "OPEN");
    unlinked.choices.clear();
    assert!(state.loaded(lookup, unlinked).unwrap());
    assert!(state.lists_every_account());
    assert_eq!(keys(&state), ["LINKED", "UNLINKED"]);
    assert!(state.is_unchosen() && state.selected_key().is_none());
}

#[test]
fn a_project_with_links_lists_only_those_and_its_default() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, None).unwrap();
    assert!(
        state
            .offer_catalog(BillingCatalog {
                connection,
                accounts: catalog(&[(1, "LINKED", "OPEN"), (2, "UNLINKED", "OPEN")]),
            })
            .unwrap()
    );
    let lookup = state.begin(project(7));
    // The project narrows the list as soon as its lookup begins.
    assert!(!state.lists_every_account() && state.choices().is_empty());
    assert!(
        state
            .loaded(lookup, result(connection, 7, "LINKED", "OPEN"))
            .unwrap()
    );
    assert!(!state.lists_every_account());
    assert_eq!(keys(&state), ["LINKED"]);
    assert_eq!(state.selected_key(), Some("LINKED"));
    assert!(!state.lists(Some("UNLINKED")));
}

#[test]
fn a_late_catalog_never_replaces_a_project_lookup_or_crosses_connections() {
    let connection = WriteScope::new();
    let accounts = || catalog(&[(2, "UNLINKED", "OPEN")]);
    // In flight or loaded, the project's own result wins.
    let mut state = BillingSelection::new(connection, None).unwrap();
    let lookup = state.begin(project(7));
    let late = BillingCatalog {
        connection,
        accounts: accounts(),
    };
    assert!(!state.offer_catalog(late).unwrap());
    assert!(state.choices().is_empty() && state.is_loading());
    state
        .loaded(lookup, result(connection, 7, "LINKED", "OPEN"))
        .unwrap();
    let late = BillingCatalog {
        connection,
        accounts: accounts(),
    };
    assert!(!state.offer_catalog(late).unwrap());
    assert_eq!(keys(&state), ["LINKED"]);
    // A failed project lookup leaves the project unknown, not a fallback.
    let failed = state.begin(project(7));
    assert!(state.failed(failed));
    assert!(!state.lists_every_account());
    assert_eq!(keys(&state), ["LINKED"]);
    // Another connection's catalog is refused and lists nothing.
    let mut fresh = BillingSelection::new(connection, None).unwrap();
    let foreign = BillingCatalog {
        connection: WriteScope::new(),
        accounts: accounts(),
    };
    assert!(matches!(
        fresh.offer_catalog(foreign),
        Err(Error::WrongConnection)
    ));
    assert!(fresh.choices().is_empty() && !fresh.lists_every_account());
}

#[test]
fn a_cleared_or_changed_project_stops_listing_the_fallback() {
    let connection = WriteScope::new();
    let mut state = BillingSelection::new(connection, None).unwrap();
    let lookup = state.begin(project(7));
    let mut unlinked = result(connection, 7, "LINKED", "OPEN");
    unlinked.choices.clear();
    state.loaded(lookup, unlinked).unwrap();
    assert!(state.lists_every_account());
    let _next = state.begin(project(8));
    assert!(!state.lists_every_account() && state.choices().is_empty());
    state.clear_project();
    assert!(!state.lists_every_account() && state.choices().is_empty());
}
