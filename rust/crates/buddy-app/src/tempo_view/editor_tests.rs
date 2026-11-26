use super::*;
use buddy_core::tempo::{
    Attribute, Client, IssueId, JiraProjectId, ProjectBillingAccounts, WorklogId,
};
use serde_json::Value;

fn details() -> EditorIssue {
    EditorIssue {
        issue: JiraIssue {
            id: IssueId::parse(19).unwrap(),
            key: "SYNTH-19".into(),
            summary: Some("Synthetic issue".into()),
        },
        billing: ProjectBillingAccounts {
            connection: Client::new("synthetic-unused-token").unwrap().write_scope(),
            project: JiraProjectId::parse(11).unwrap(),
            choices: Vec::new(),
            catalog: Vec::new(),
        },
        capitalization: Ok(None),
    }
}
fn worklog() -> Worklog {
    Worklog {
        id: WorklogId::parse(7).unwrap(),
        issue: Some(IssueId::parse(19).unwrap()),
        author: AccountId::parse("synthetic-user").unwrap(),
        seconds: 90061,
        billable_seconds: Some(600),
        date: "2026-10-16".parse().unwrap(),
        start_time: None,
        start_instant: None,
        description: Some("Original private work 界\nSecond line".into()),
        attributes: Some(vec![
            Attribute {
                key: "_Account_".into(),
                value: Some("UNAVAILABLE-ORIGINAL".into()),
            },
            Attribute {
                key: "synthetic-custom".into(),
                value: Some("keep-exactly".into()),
            },
        ]),
        created_at: "2026-10-16T01:00:00Z".parse().unwrap(),
        updated_at: "2026-10-16T02:00:00Z".parse().unwrap(),
    }
}

#[gpui_kit::test]
fn reauthentication_requires_a_fresh_lookup_and_retains_exact_draft_inputs_and_billing_choice(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        assert!(!cx.has_global::<Runtime>());
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = TempoView::new(window, cx);
                for selected in [None, Some("UNAVAILABLE-ORIGINAL")] {
                    let row = worklog();
                    let mut editor = Editor::new(Some(row.clone()), row.date, next_start_time(0), window, cx);
                    editor.accept_lookup("19".into(), details(), cx).unwrap();
                    editor.billing.as_mut().unwrap().select(selected).unwrap();
                    editor.hours.update(cx, |field, cx| field.set_value("2:15:01", window, cx));
                    editor.date.update(cx, |field, cx| field.set_value("2026-10-22", window, cx));
                    editor.start.update(cx, |field, cx| field.set_value("09:10:11", window, cx));
                    editor.description.update(cx, |field, cx| field.set_value("Retained draft 界", window, cx));
                    editor.billing_filter.update(cx, |field, cx| field.set_value("Original filter", window, cx));
                    let old_id = editor.id;
                    editor.invalidate_connection();
                    assert_ne!(editor.id, old_id, "Late old-connection lookups cannot match this editor");
                    assert!(editor.resolved.is_none() && editor.billing.is_none());
                    assert!(!editor.has_current_issue(cx));
                    assert!(editor.draft(&row.author, cx).is_err());
                    assert_eq!(editor.hours.read(cx).value(), "2:15:01");
                    assert_eq!(editor.date.read(cx).value(), "2026-10-22");
                    assert_eq!(editor.start.read(cx).value(), "09:10:11");
                    assert_eq!(editor.description.read(cx).value(), "Retained draft 界");
                    assert_eq!(editor.billing_filter.read(cx).value(), "Original filter");
                    assert_eq!(editor.original.as_ref().unwrap().updated_at, row.updated_at);
                    editor.accept_lookup("19".into(), details(), cx).unwrap();
                    assert!(editor.has_current_issue(cx));
                    assert_eq!(editor.billing.as_ref().unwrap().selected_key(), selected);
                    let body: Value = serde_json::from_slice(&editor.draft(&row.author, cx).unwrap().encode().unwrap()).unwrap();
                    assert_eq!(body["timeSpentSeconds"], 8101);
                    assert_eq!(body["billableSeconds"], 600);
                    assert_eq!(body["startDate"], "2026-10-22");
                    assert_eq!(body["startTime"], "09:10:11");
                    assert_eq!(body["description"], "Retained draft 界");
                    assert!(body["attributes"].as_array().unwrap().iter().any(|a|a["key"]=="synthetic-custom" && a["value"]=="keep-exactly"));
                    assert_eq!(body["attributes"].as_array().unwrap().iter().find(|a|a["key"]=="_Account_").and_then(|a|a["value"].as_str()), selected);
                    let new_id = editor.id;
                    view.editor = Some(editor);
                    view.operate(Operation::Today, cx);
                    assert_eq!(view.editor.as_ref().unwrap().id, new_id);
                    assert!(view.error.as_deref().unwrap().contains("Save or cancel"));
                    assert!(!view.busy && view.connection.is_none());
                }
                view
            })
        }).unwrap();
    });
}
#[gpui_kit::test]
fn editing_retains_unavailable_billing_custom_attributes_and_input_after_failed_validation(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        assert!(!cx.has_global::<Runtime>());
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                let row = worklog();
                let mut editor =
                    Editor::new(Some(row.clone()), row.date, next_start_time(0), window, cx);
                editor.accept_lookup("19".into(), details(), cx).unwrap();
                assert!(!editor.billing.as_ref().unwrap().selection_is_listed());
                editor
                    .hours
                    .update(cx, |field, cx| field.set_value("2:15:01", window, cx));
                let body: Value = serde_json::from_slice(
                    &editor.draft(&row.author, cx).unwrap().encode().unwrap(),
                )
                .unwrap();
                assert_eq!(body["timeSpentSeconds"], 8101);
                assert_eq!(body["billableSeconds"], 600);
                assert_eq!(body["startDate"], "2026-10-16");
                assert!(body.get("startTime").is_none());
                assert_eq!(body["description"], "Original private work 界\nSecond line");
                assert!(body.get("issueId").is_none());
                assert_eq!(body["attributes"].as_array().unwrap().len(), 2);
                assert!(
                    body["attributes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|a| a["key"] == "synthetic-custom" && a["value"] == "keep-exactly")
                );
                assert!(
                    body["attributes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|a| a["key"] == "_Account_" && a["value"] == "UNAVAILABLE-ORIGINAL")
                );
                editor
                    .hours
                    .update(cx, |field, cx| field.set_value("NaN", window, cx));
                assert!(editor.draft(&row.author, cx).is_err());
                assert_eq!(editor.hours.read(cx).value(), "NaN");
                assert_eq!(
                    editor.description.read(cx).value(),
                    "Original private work 界\nSecond line"
                );
                assert_eq!(editor.date.read(cx).value(), "2026-10-16");
                view
            })
        })
        .unwrap();
    });
}
#[gpui_kit::test]
fn an_obsolete_issue_lookup_cannot_enable_a_create_and_activation_without_runtime_cannot_load_credentials(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = TempoView::new(window, cx);
                view.activate(cx);
                assert!(!view.initialized && !view.busy && view.connection.is_none());
                let date = "2026-10-16".parse().unwrap();
                let mut editor = Editor::new(None, date, next_start_time(0), window, cx);
                editor
                    .issue
                    .update(cx, |field, cx| field.set_value("SYNTH-20", window, cx));
                assert!(
                    editor
                        .accept_lookup("SYNTH-19".into(), details(), cx)
                        .is_err()
                );
                assert!(!editor.has_current_issue(cx));
                assert!(editor.resolved.is_none());
                editor
                    .issue
                    .update(cx, |field, cx| field.set_value("SYNTH-19", window, cx));
                editor
                    .accept_lookup("SYNTH-19".into(), details(), cx)
                    .unwrap();
                editor
                    .hours
                    .update(cx, |field, cx| field.set_value("1.5", window, cx));
                let author = AccountId::parse("synthetic-user").unwrap();
                let body: Value =
                    serde_json::from_slice(&editor.draft(&author, cx).unwrap().encode().unwrap())
                        .unwrap();
                assert_eq!(body["issueId"], 19);
                assert_eq!(body["timeSpentSeconds"], 5400);
                assert_eq!(body["startDate"], "2026-10-16");
                assert_eq!(body["startTime"], "08:00:00");
                editor
                    .issue
                    .update(cx, |field, cx| field.set_value("SYNTH-20", window, cx));
                assert!(editor.draft(&author, cx).is_err());
                assert_eq!(editor.hours.read(cx).value(), "1.5");
                view
            })
        })
        .unwrap();
    });
}

#[gpui_kit::test]
fn a_template_opens_a_new_one_hour_draft_with_the_selected_date_description_and_billing(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        assert!(!cx.has_global::<Runtime>());
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                for selected in [None, Some("UNAVAILABLE-ORIGINAL")] {
                    let mut source = worklog();
                    if selected.is_none() {
                        source
                            .attributes
                            .as_mut()
                            .unwrap()
                            .retain(|attribute| attribute.key != "_Account_");
                    }
                    let date = "2026-11-05".parse().unwrap();
                    let start = "09:15:01".parse().unwrap();
                    let mut editor =
                        Editor::from_template(&source, date, start, window, cx).unwrap();
                    assert!(editor.original.is_none());
                    assert_eq!(editor.hours.read(cx).value(), "1");
                    assert_eq!(editor.date.read(cx).value(), "2026-11-05");
                    assert_eq!(editor.start.read(cx).value(), "09:15:01");
                    assert!(!editor.has_current_issue(cx));
                    editor.accept_lookup("19".into(), details(), cx).unwrap();
                    assert_eq!(editor.billing.as_ref().unwrap().selected_key(), selected);
                    let draft = editor.draft(&source.author, cx).unwrap();
                    assert_eq!(draft.kind(), buddy_core::tempo::DraftKind::Create);
                    assert!(draft.worklog_id().is_none() && draft.observed_update().is_none());
                    let body: Value = serde_json::from_slice(&draft.encode().unwrap()).unwrap();
                    assert_eq!(body["issueId"], 19);
                    assert_eq!(body["timeSpentSeconds"], 3600);
                    assert_eq!(body["startDate"], "2026-11-05");
                    assert_eq!(body["startTime"], "09:15:01");
                    assert_eq!(body["description"], "Original private work 界\nSecond line");
                    assert_eq!(
                        body["attributes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|attribute| attribute["key"] == "_Account_")
                            .and_then(|attribute| attribute["value"].as_str()),
                        selected
                    );
                    assert_eq!(source.seconds, 90061);
                    assert_eq!(source.date.to_string(), "2026-10-16");
                }
                view
            })
        })
        .unwrap();
    });
}

#[gpui_kit::test]
fn the_work_date_must_be_canonical_and_the_start_time_may_be_unpadded(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                let row = worklog();
                let mut editor =
                    Editor::new(Some(row.clone()), row.date, next_start_time(0), window, cx);
                editor.accept_lookup("19".into(), details(), cx).unwrap();
                editor
                    .hours
                    .update(cx, |field, cx| field.set_value("1:00:00", window, cx));
                let invalid_draft = buddy_core::tempo::Error::InvalidDraft.to_string();
                for (text, sent) in [
                    ("", Ok(None)),
                    ("09:10:11", Ok(Some("09:10:11"))),
                    ("9:5:7", Ok(Some("09:05:07"))),
                    ("09:10", Ok(Some("09:10:00"))),
                    ("9:5", Ok(Some("09:05:00"))),
                    (" 9: 5", Ok(Some("09:05:00"))),
                    ("23:59:59", Ok(Some("23:59:59"))),
                    // A 60th second is read, then refused as an unsendable time.
                    ("09:10:60", Err(invalid_draft.as_str())),
                    ("24:00", Err("Enter a start time as HH:MM or HH:MM:SS.")),
                    ("09:60", Err("Enter a start time as HH:MM or HH:MM:SS.")),
                    (
                        "09:10:11.5",
                        Err("Enter a start time as HH:MM or HH:MM:SS."),
                    ),
                    ("09:10:11 ", Err("Enter a start time as HH:MM or HH:MM:SS.")),
                    ("0910", Err("Enter a start time as HH:MM or HH:MM:SS.")),
                    ("9 AM", Err("Enter a start time as HH:MM or HH:MM:SS.")),
                ] {
                    editor
                        .start
                        .update(cx, |field, cx| field.set_value(text, window, cx));
                    let result = editor.draft(&row.author, cx).map(|draft| {
                        serde_json::from_slice::<Value>(&draft.encode().unwrap()).unwrap()
                    });
                    match (result, sent) {
                        (Ok(body), Ok(sent)) => {
                            assert_eq!(
                                body.get("startTime").and_then(Value::as_str),
                                sent,
                                "{text:?}"
                            )
                        }
                        (Err(message), Err(expected)) => assert_eq!(message, expected, "{text:?}"),
                        (result, _) => panic!("{text:?}: {:?}", result.map(|_| ())),
                    }
                }
                editor
                    .start
                    .update(cx, |field, cx| field.set_value("", window, cx));
                for (text, sent) in [
                    ("2026-10-22", Ok("2026-10-22")),
                    ("2028-02-29", Ok("2028-02-29")),
                    ("0001-01-01", Ok("0001-01-01")),
                    ("9999-12-31", Ok("9999-12-31")),
                    ("2026-10-2", Err("Enter the work date as YYYY-MM-DD.")),
                    (" 2026-10-22", Err("Enter the work date as YYYY-MM-DD.")),
                    ("+2026-10-22", Err("Enter the work date as YYYY-MM-DD.")),
                    ("2026-02-30", Err("Enter a valid work date as YYYY-MM-DD.")),
                    ("2026-10-22 ", Err("Enter a valid work date as YYYY-MM-DD.")),
                    ("22/10/2026", Err("Enter a valid work date as YYYY-MM-DD.")),
                    ("", Err("Enter a valid work date as YYYY-MM-DD.")),
                    ("0000-01-01", Err(invalid_draft.as_str())),
                ] {
                    editor
                        .date
                        .update(cx, |field, cx| field.set_value(text, window, cx));
                    let result = editor.draft(&row.author, cx).map(|draft| {
                        serde_json::from_slice::<Value>(&draft.encode().unwrap()).unwrap()
                    });
                    match (result, sent) {
                        (Ok(body), Ok(sent)) => assert_eq!(body["startDate"], sent, "{text:?}"),
                        (Err(message), Err(expected)) => assert_eq!(message, expected, "{text:?}"),
                        (result, _) => panic!("{text:?}: {:?}", result.map(|_| ())),
                    }
                }
                view
            })
        })
        .unwrap();
    });
}

#[gpui_kit::test]
fn a_create_defaults_an_empty_description_and_infers_an_unchosen_account_from_the_issue_key(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        assert!(!cx.has_global::<Runtime>());
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                let author = AccountId::parse("synthetic-user").unwrap();
                let date = "2026-10-16".parse().unwrap();
                let body = |editor: &Editor, cx: &Context<TempoView>| -> Value {
                    serde_json::from_slice(&editor.draft(&author, cx).unwrap().encode().unwrap())
                        .unwrap()
                };
                for (key, account) in [("INT-4", "INT"), ("SYNTH-19", "GEN-DEV")] {
                    let issue = || {
                        let mut issue = details();
                        issue.issue.key = key.into();
                        issue
                    };
                    let mut editor = Editor::new(None, date, next_start_time(0), window, cx);
                    editor
                        .issue
                        .update(cx, |field, cx| field.set_value("19", window, cx));
                    editor.accept_lookup("19".into(), issue(), cx).unwrap();
                    editor
                        .hours
                        .update(cx, |field, cx| field.set_value("1.5", window, cx));
                    assert!(editor.billing.as_ref().unwrap().is_unchosen());
                    // A connection re-check and a new lookup keep it unchosen.
                    editor.invalidate_connection();
                    editor.accept_lookup("19".into(), issue(), cx).unwrap();
                    assert!(editor.billing.as_ref().unwrap().is_unchosen());
                    let value = body(&editor, cx);
                    assert_eq!(value["description"], format!("Working on issue {key}"));
                    assert_eq!(
                        value["attributes"],
                        serde_json::json!([{"key": "_Account_", "value": account}])
                    );
                    // Typed text and an explicit "No billing account" are kept.
                    editor
                        .description
                        .update(cx, |field, cx| field.set_value("Typed work", window, cx));
                    editor.billing.as_mut().unwrap().select(None).unwrap();
                    let value = body(&editor, cx);
                    assert_eq!(value["description"], "Typed work");
                    assert_eq!(value["attributes"], serde_json::json!([]));
                    // Looking the same project up again keeps that choice.
                    editor.accept_lookup("19".into(), issue(), cx).unwrap();
                    assert!(!editor.billing.as_ref().unwrap().is_unchosen());
                    assert_eq!(body(&editor, cx)["attributes"], serde_json::json!([]));
                }
                // An edit never gains either default.
                let mut original = worklog();
                original.description = None;
                original.attributes = Some(Vec::new());
                let mut editor = Editor::new(Some(original), date, next_start_time(0), window, cx);
                editor
                    .issue
                    .update(cx, |field, cx| field.set_value("19", window, cx));
                editor.accept_lookup("19".into(), details(), cx).unwrap();
                editor
                    .hours
                    .update(cx, |field, cx| field.set_value("1.5", window, cx));
                let value = body(&editor, cx);
                assert!(value.get("description").is_none());
                assert_eq!(value["attributes"], serde_json::json!([]));
                view
            })
        })
        .unwrap();
    });
}

fn account(id: u64, key: &str) -> buddy_core::tempo::BillingAccount {
    buddy_core::tempo::BillingAccount::new(
        id,
        key,
        &format!("Synthetic billing account {key}"),
        false,
        buddy_core::tempo::BillingAccountStatus::Open,
    )
    .unwrap()
}
/// Tempo's catalog, in its order: one account the project links, one it doesn't.
fn every_account() -> Vec<buddy_core::tempo::BillingAccount> {
    vec![account(17, "SYNTH-LINKED"), account(20, "SYNTH-UNLINKED")]
}
fn catalog() -> buddy_core::tempo::BillingCatalog {
    buddy_core::tempo::BillingCatalog {
        connection: Client::new("synthetic-unused-token").unwrap().write_scope(),
        accounts: every_account(),
    }
}
/// The lookup result. `linked` is the project's one linked account, if any.
fn project_lookup(linked: bool) -> EditorIssue {
    let mut info = details();
    info.billing.catalog = every_account();
    if linked {
        info.billing.choices = vec![buddy_core::tempo::ProjectBillingAccount {
            account: account(17, "SYNTH-LINKED"),
            is_default: true,
        }];
    }
    info
}
fn listed(editor: &Editor) -> Vec<String> {
    editor
        .billing
        .as_ref()
        .unwrap()
        .choices()
        .iter()
        .map(|choice| choice.account.key().to_owned())
        .collect()
}

#[gpui_kit::test]
fn every_account_is_listed_before_a_lookup_and_for_a_project_without_linked_accounts(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        assert!(!cx.has_global::<Runtime>());
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                let date = "2026-10-16".parse().unwrap();
                let author = AccountId::parse("synthetic-user").unwrap();
                let open = |window: &mut Window, cx: &mut Context<TempoView>| {
                    let editor = Editor::new(None, date, next_start_time(0), window, cx);
                    editor
                        .issue
                        .update(cx, |field, cx| field.set_value("19", window, cx));
                    editor
                };

                // Before a project is known: every account, in Tempo's order,
                // with nothing chosen. Saving still needs the lookup.
                let mut editor = open(window, cx);
                assert!(!editor.accept_catalog(uuid::Uuid::new_v4(), Ok(catalog())));
                assert!(editor.billing.is_none());
                assert!(editor.accept_catalog(editor.id, Ok(catalog())));
                let billing = editor.billing.as_ref().unwrap();
                assert!(billing.lists_every_account() && billing.project().is_none());
                assert!(billing.is_unchosen());
                assert_eq!(listed(&editor), ["SYNTH-LINKED", "SYNTH-UNLINKED"]);
                assert!(!editor.has_current_issue(cx));
                assert!(editor.draft(&author, cx).is_err());
                // A repeated catalog cannot replace the list a person sees.
                assert!(!editor.accept_catalog(editor.id, Ok(catalog())));

                // A project with links lists only those, with its default.
                editor
                    .accept_lookup("19".into(), project_lookup(true), cx)
                    .unwrap();
                let billing = editor.billing.as_ref().unwrap();
                assert!(!billing.lists_every_account());
                assert_eq!(listed(&editor), ["SYNTH-LINKED"]);
                assert_eq!(billing.selected_key(), Some("SYNTH-LINKED"));
                // A late catalog never widens a looked-up project's list.
                assert!(!editor.accept_catalog(editor.id, Ok(catalog())));
                assert_eq!(listed(&editor), ["SYNTH-LINKED"]);

                // A project that links no account lists every account, with
                // no default, and a create still infers its account.
                let mut editor = open(window, cx);
                editor
                    .accept_lookup("19".into(), project_lookup(false), cx)
                    .unwrap();
                let billing = editor.billing.as_ref().unwrap();
                assert!(billing.lists_every_account() && billing.project().is_some());
                assert!(billing.is_unchosen());
                assert_eq!(listed(&editor), ["SYNTH-LINKED", "SYNTH-UNLINKED"]);
                editor
                    .hours
                    .update(cx, |field, cx| field.set_value("1", window, cx));
                let body: Value =
                    serde_json::from_slice(&editor.draft(&author, cx).unwrap().encode().unwrap())
                        .unwrap();
                assert_eq!(
                    body["attributes"],
                    serde_json::json!([{"key": "_Account_", "value": "GEN-DEV"}])
                );
                editor
                    .billing
                    .as_mut()
                    .unwrap()
                    .select(Some("SYNTH-UNLINKED"))
                    .unwrap();
                let body: Value =
                    serde_json::from_slice(&editor.draft(&author, cx).unwrap().encode().unwrap())
                        .unwrap();
                assert_eq!(
                    body["attributes"],
                    serde_json::json!([{"key": "_Account_", "value": "SYNTH-UNLINKED"}])
                );
                view
            })
        })
        .unwrap();
    });
}

#[gpui_kit::test]
fn a_failed_account_list_stays_unknown_and_a_pick_before_lookup_never_bills_an_unlisted_account(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                let date = "2026-10-16".parse().unwrap();
                let open = |window: &mut Window, cx: &mut Context<TempoView>| {
                    let editor = Editor::new(None, date, next_start_time(0), window, cx);
                    editor
                        .issue
                        .update(cx, |field, cx| field.set_value("19", window, cx));
                    editor
                };

                // A failed read is not an empty list: there is no billing
                // state, and the reason stays until a lookup succeeds.
                let mut editor = open(window, cx);
                let problem = "Tempo is limiting requests. Wait before refreshing.";
                assert!(!editor.accept_catalog(editor.id, Err(problem.into())));
                assert!(editor.billing.is_none());
                assert_eq!(editor.catalog_problem.as_deref(), Some(problem));
                editor
                    .accept_lookup("19".into(), project_lookup(true), cx)
                    .unwrap();
                assert!(editor.catalog_problem.is_none());
                assert_eq!(listed(&editor), ["SYNTH-LINKED"]);

                // A pick the project doesn't list gives way to its default;
                // one it lists, or "No billing account", is kept.
                for (pick, linked, kept) in [
                    (Some("SYNTH-UNLINKED"), true, Some("SYNTH-LINKED")),
                    (Some("SYNTH-LINKED"), true, Some("SYNTH-LINKED")),
                    (None, true, None),
                    (Some("SYNTH-UNLINKED"), false, Some("SYNTH-UNLINKED")),
                ] {
                    let mut editor = open(window, cx);
                    assert!(editor.accept_catalog(editor.id, Ok(catalog())));
                    editor.billing.as_mut().unwrap().select(pick).unwrap();
                    editor
                        .accept_lookup("19".into(), project_lookup(linked), cx)
                        .unwrap();
                    let billing = editor.billing.as_ref().unwrap();
                    assert_eq!(billing.selected_key(), kept, "{pick:?} {linked}");
                    assert!(billing.selection_is_listed(), "{pick:?} {linked}");
                }

                // A reconnect drops a pick made from the old connection's
                // catalog along with the catalog itself.
                let mut editor = open(window, cx);
                assert!(editor.accept_catalog(editor.id, Ok(catalog())));
                editor
                    .billing
                    .as_mut()
                    .unwrap()
                    .select(Some("SYNTH-UNLINKED"))
                    .unwrap();
                let old = editor.id;
                editor.invalidate_connection();
                assert!(editor.billing.is_none());
                assert!(!editor.accept_catalog(old, Ok(catalog())));
                editor
                    .accept_lookup("19".into(), project_lookup(false), cx)
                    .unwrap();
                assert!(editor.billing.as_ref().unwrap().is_unchosen());
                view
            })
        })
        .unwrap();
    });
}
