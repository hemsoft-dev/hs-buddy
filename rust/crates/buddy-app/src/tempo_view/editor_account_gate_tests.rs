//! Every way an account key can reach a worklog request goes through one
//! gate, `BillingSelection::request_account`. These tests drive each source
//! through the real editor and read the encoded request.
use super::behavior_tests::{account, account_with, catalog, details, worklog};
use super::*;
use buddy_core::tempo::BillingAccountStatus::{self, Archived, Closed, Open};
use buddy_core::tempo::{BillingCatalog, ProjectBillingAccount};
use serde_json::Value;

/// The account each source tries to send. A non-INT issue key infers it.
const TARGET: &str = "GEN-DEV";

fn author() -> AccountId {
    AccountId::parse("synthetic-user").unwrap()
}
/// A test case label: the status enum has no Debug, by design.
fn label(status: Option<BillingAccountStatus>) -> &'static str {
    match status {
        Some(Open) => "open",
        Some(Closed) => "closed",
        Some(Archived) => "archived",
        None => "unlisted",
    }
}
/// The `_Account_` value the request carries, if any.
fn sent(editor: &Editor, cx: &Context<TempoView>) -> Option<String> {
    let body: Value =
        serde_json::from_slice(&editor.draft(&author(), cx).unwrap().encode().unwrap()).unwrap();
    body["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|attribute| attribute["key"] == "_Account_")
        .and_then(|attribute| attribute["value"].as_str().map(str::to_owned))
}
/// Every Tempo account, with TARGET open: what a draft lists before a lookup.
fn catalog_with_target() -> BillingCatalog {
    let mut catalog = catalog();
    catalog.accounts.push(account(40, TARGET));
    catalog
}
/// A fresh lookup of the issue's project. `linked`: the project links
/// SYNTH-OTHER (open, not a default) and TARGET with `status`; otherwise it
/// links none, so the list is every account with TARGET at `status`. `None`
/// leaves TARGET out of the list. `default` makes TARGET the project default.
fn fresh(linked: bool, status: Option<BillingAccountStatus>, default: bool) -> EditorIssue {
    let mut info = details();
    info.billing.catalog = vec![account(17, "SYNTH-LINKED"), account(20, "SYNTH-UNLINKED")];
    let target = status.map(|status| account_with(40, TARGET, status));
    if linked {
        info.billing.choices = vec![ProjectBillingAccount {
            account: account(30, "SYNTH-OTHER"),
            is_default: false,
        }];
        if let Some(target) = target {
            info.billing.choices.push(ProjectBillingAccount {
                account: target,
                is_default: default,
            });
        }
    } else if let Some(target) = target {
        info.billing.catalog.push(target);
    }
    info
}

#[derive(Clone, Copy, Debug)]
enum Source {
    /// A person picks TARGET after the lookup.
    Explicit,
    /// Picked after an earlier lookup of the same project, then looked up again.
    Retained,
    /// Picked from every account before any lookup.
    Provisional,
    /// A template whose worklog billed TARGET.
    Template,
    /// Picked, then kept across a connection recheck.
    Recheck,
    /// Nothing picked: a create infers TARGET from the issue key.
    Inference,
    /// The project makes TARGET its default.
    ProjectDefault,
    /// An edit of a worklog billing SYNTH-EXISTING; a person picks TARGET.
    Edit,
}
const SOURCES: [Source; 8] = [
    Source::Explicit,
    Source::Retained,
    Source::Provisional,
    Source::Template,
    Source::Recheck,
    Source::Inference,
    Source::ProjectDefault,
    Source::Edit,
];

/// Drive `source` into a draft whose fresh list holds TARGET at `status`
/// (`None`: not listed). Returns the editor after the fresh lookup and any
/// later pick.
fn drive(
    source: Source,
    linked: bool,
    status: Option<BillingAccountStatus>,
    window: &mut Window,
    cx: &mut Context<TempoView>,
) -> Editor {
    let date = "2026-10-16".parse().unwrap();
    let blank = |window: &mut Window, cx: &mut Context<TempoView>| {
        let editor = Editor::new(None, date, next_start_time(0), window, cx);
        editor
            .issue
            .update(cx, |field, cx| field.set_value("19", window, cx));
        editor
    };
    let pick = |editor: &mut Editor| {
        // Refused or not, the request decides; a refusal leaves it as it was.
        let _ = editor.choose(Some(TARGET));
    };
    let earlier = fresh(linked, Some(Open), false);
    let default = matches!(source, Source::ProjectDefault);
    let mut editor = match source {
        Source::Template => {
            let mut row = worklog();
            row.attributes.as_mut().unwrap()[0].value = Some(TARGET.into());
            Editor::from_template(&row, date, next_start_time(0), window, cx).unwrap()
        }
        Source::Edit => {
            let mut row = worklog();
            row.attributes.as_mut().unwrap()[0].value = Some("SYNTH-EXISTING".into());
            let editor = Editor::new(Some(row), date, next_start_time(0), window, cx);
            editor
                .issue
                .update(cx, |field, cx| field.set_value("19", window, cx));
            // The fixture's 25 hours is not a duration a save accepts.
            editor
                .hours
                .update(cx, |field, cx| field.set_value("1", window, cx));
            editor
        }
        _ => blank(window, cx),
    };
    match source {
        Source::Retained | Source::Recheck => {
            editor.accept_lookup("19".into(), earlier, cx).unwrap();
            pick(&mut editor);
            if matches!(source, Source::Recheck) {
                editor.invalidate_connection();
            }
        }
        Source::Provisional => {
            assert!(editor.accept_catalog(editor.id, Ok(catalog_with_target())));
            pick(&mut editor);
        }
        _ => {}
    }
    editor
        .accept_lookup("19".into(), fresh(linked, status, default), cx)
        .unwrap();
    if matches!(source, Source::Explicit | Source::Edit) {
        pick(&mut editor);
    }
    editor
}

/// Across every source and every state of TARGET in the fresh list, the
/// request carries TARGET only where that list has it open, and then it
/// always does. Otherwise it carries what a fresh draft would: the edit's
/// existing account, or nothing (no other account here is a default).
#[gpui_kit::test]
fn no_source_sends_an_account_the_fresh_list_does_not_hold_open(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                let statuses = [Some(Open), Some(Closed), Some(Archived), None];
                for source in SOURCES {
                    for linked in [true, false] {
                        if matches!(source, Source::ProjectDefault) && !linked {
                            // Every-account lists have no default.
                            continue;
                        }
                        for status in statuses {
                            let editor = drive(source, linked, status, window, cx);
                            let case = format!("{source:?} linked={linked} {}", label(status));
                            let open = status == Some(Open);
                            let fallback = match source {
                                Source::Edit => Some("SYNTH-EXISTING".to_owned()),
                                _ => None,
                            };
                            let expected = if open {
                                Some(TARGET.to_owned())
                            } else {
                                fallback
                            };
                            assert_eq!(sent(&editor, cx), expected, "{case}");
                        }
                    }
                }
                view
            })
        })
        .unwrap();
    });
}

/// The round-3 review case: pick GEN-DEV from every account, then look up a
/// non-INT issue whose project links other accounts and no open default.
#[gpui_kit::test]
fn a_dropped_catalog_pick_is_not_inferred_back_into_the_request(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                for default in [None, Some(Closed), Some(Archived)] {
                    let mut editor = Editor::new(
                        None,
                        "2026-10-16".parse().unwrap(),
                        next_start_time(0),
                        window,
                        cx,
                    );
                    editor
                        .issue
                        .update(cx, |field, cx| field.set_value("19", window, cx));
                    assert!(editor.accept_catalog(editor.id, Ok(catalog_with_target())));
                    editor.choose(Some(TARGET)).unwrap();
                    let mut info = fresh(true, None, false);
                    if let Some(status) = default {
                        info.billing.choices.push(ProjectBillingAccount {
                            account: account_with(31, "SYNTH-DEFAULT", status),
                            is_default: true,
                        });
                    }
                    editor.accept_lookup("19".into(), info, cx).unwrap();
                    assert!(
                        editor.billing().unwrap().is_unchosen(),
                        "{}",
                        label(default)
                    );
                    assert_eq!(sent(&editor, cx), None, "{}", label(default));
                }
                view
            })
        })
        .unwrap();
    });
}

/// Inference follows the fresh list's status: closed or archived, or not
/// listed, sends nothing; listed and open sends it.
#[gpui_kit::test]
fn inference_sends_an_account_only_where_the_fresh_list_has_it_open(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                for linked in [true, false] {
                    for (status, expected) in [
                        (Some(Closed), None),
                        (Some(Archived), None),
                        (None, None),
                        (Some(Open), Some(TARGET.to_owned())),
                    ] {
                        let editor = drive(Source::Inference, linked, status, window, cx);
                        assert!(editor.billing().unwrap().is_unchosen());
                        assert_eq!(sent(&editor, cx), expected, "{linked} {}", label(status));
                    }
                }
                view
            })
        })
        .unwrap();
    });
}

/// A connection recheck drops a blank draft's catalog, and the draft asks
/// for it again, as when it opened. A draft that looked its issue up, or an
/// edit, does not.
#[gpui_kit::test]
fn a_recheck_asks_a_blank_draft_to_read_every_account_again(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                let date = "2026-10-16".parse().unwrap();
                let mut editor = Editor::new(None, date, next_start_time(0), window, cx);
                assert!(editor.wants_catalog());
                assert!(editor.accept_catalog(editor.id, Ok(catalog())));
                assert!(!editor.wants_catalog());
                let old = editor.id;
                editor.invalidate_connection();
                assert!(editor.billing().is_none());
                assert!(
                    editor.wants_catalog(),
                    "the recheck must restart the catalog"
                );
                // The restarted read lands; the old connection's never does.
                assert!(!editor.accept_catalog(old, Ok(catalog())));
                assert!(editor.accept_catalog(editor.id, Ok(catalog())));
                assert_eq!(editor.billing().unwrap().choices().len(), 2);

                // Once an issue lookup began, the project's list decides.
                let mut looked_up = Editor::new(None, date, next_start_time(0), window, cx);
                looked_up.begin_lookup();
                looked_up.invalidate_connection();
                assert!(!looked_up.wants_catalog());
                // An edit never lists every account before its lookup.
                let mut edit = Editor::new(Some(worklog()), date, next_start_time(0), window, cx);
                edit.invalidate_connection();
                assert!(!edit.wants_catalog());
                view
            })
        })
        .unwrap();
    });
}
