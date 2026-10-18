use super::*;
use crate::pending_file::{PendingFile, RecoverySlot};
use crate::tempo::jira::tests::{Fixture, reply};
use crate::tempo::transport::tests::single_worklog;
use crate::tempo::{AccountId, IssueId};
use serde_json::json;

fn now() -> DateTime<Utc> {
    "2026-10-16T12:00:00Z".parse().unwrap()
}
fn identity() -> String {
    reply(
        200,
        r#"{"accountId":"123456:synthetic-user","timeZone":"America/Winnipeg"}"#,
    )
}
fn complete() -> Vec<String> {
    vec![
        identity(),
        reply(
            200,
            &json!({"self":"https://api.tempo.io/4/worklogs",
        "metadata":{"count":1,"limit":50,"offset":0},"results":[single_worklog()]})
            .to_string(),
        ),
        reply(
            200,
            r#"{"self":"https://api.tempo.io/4/user-schedule","metadata":{"count":0},"results":[]}"#,
        ),
        identity(),
    ]
}
fn clients(fixture: &Fixture) -> (Arc<Client>, Arc<JiraClient>) {
    (
        Arc::new(
            Client::build(
                "synthetic-tempo-token",
                fixture.base.join("4/").unwrap(),
                false,
            )
            .unwrap(),
        ),
        Arc::new(
            JiraClient::build(
                fixture.base.clone(),
                "synthetic@example.invalid",
                "synthetic-jira-token",
                false,
            )
            .unwrap(),
        ),
    )
}
fn draft() -> Draft {
    Draft::create(
        AccountId::parse("123456:synthetic-user").unwrap(),
        IssueId::parse(19).unwrap(),
        "2026-10-16".parse().unwrap(),
        3600,
    )
    .unwrap()
}
fn file(config: &Path) -> PendingFile {
    PendingFile::open(
        config,
        RecoverySlot::Tempo,
        super::super::recovery::MAX_STORED_BYTES,
    )
    .unwrap()
}

#[test]
fn copied_work_starts_after_exact_today_totals_without_losing_seconds() {
    assert_eq!(copy_start_time(900).to_string(), "08:15:00");
    assert_eq!(copy_start_time(4501).to_string(), "09:15:01");
    assert_eq!(copy_start_time(u64::MAX).to_string(), "15:00:15");
}

fn page(rows: &[serde_json::Value]) -> String {
    reply(
        200,
        &json!({"self":"https://api.tempo.io/4/worklogs","metadata":{"count":rows.len(),"offset":0,"limit":50},"results":rows}).to_string(),
    )
}

fn copy_source(id: u64, seconds: u64, date: &str) -> serde_json::Value {
    let mut row = single_worklog();
    row["tempoWorklogId"] = json!(id);
    row["timeSpentSeconds"] = json!(seconds);
    row["billableSeconds"] = json!(600);
    row["startDate"] = json!(date);
    row["self"] = json!(format!("https://api.tempo.io/4/worklogs/{id}"));
    row["attributes"] = json!({"self":format!("https://api.tempo.io/4/worklogs/{id}/work-attribute-values"),"values":[{"key":"synthetic-custom","value":"keep 界"}]});
    row
}

#[tokio::test]
async fn review_regression_fix3_copying_forty_unexpanded_sources_can_finish_its_paced_reads() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let expanded: Vec<_> = (9..49)
        .map(|id| copy_source(id, 600, "2026-10-14"))
        .collect();
    let displayed: Vec<_> = expanded
        .iter()
        .map(|row| {
            let mut row = row.clone();
            row["attributes"].as_object_mut().unwrap().remove("values");
            row
        })
        .collect();
    let mut load = complete();
    load[1] = page(&displayed);
    let mut replies = load.clone();
    replies.extend(load);
    for (row, expanded) in displayed.iter().zip(&expanded) {
        replies.extend([
            reply(200, &row.to_string()),
            reply(200, &json!({"self":expanded["attributes"]["self"],"metadata":{"count":1},"results":expanded["attributes"]["values"]}).to_string()),
            reply(200, &row.to_string()),
        ]);
    }
    let fixture = Fixture::new(replies);
    let (client, jira) = clients(&fixture);
    let mut session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    let selected = (9..49)
        .map(|id| crate::tempo::WorklogId::parse(id).unwrap())
        .collect();
    let plan = session
        .prepare_copy(selected, now())
        .await
        .expect("a valid paced selection must produce a plan");
    assert_eq!(plan.sources().len(), 40);
    assert_eq!(plan.seconds(), 24000);
    assert!(session.can_submit() && session.pending_phase().is_none());
    assert!(file(&config).current().is_none());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 128);
    assert!(requests.iter().all(|request| request.starts_with("GET ")));
}

#[tokio::test]
async fn review_regression_fix3_an_issue_less_worklog_does_not_hide_valid_templates() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut unavailable = copy_source(8, 600, "2026-09-18");
    unavailable["issue"]["id"] = json!(null);
    let valid = copy_source(9, 600, "2026-09-19");
    let mut replies = complete();
    replies.extend([
        identity(),
        page(&[unavailable, valid]),
        complete()[2].clone(),
        identity(),
    ]);
    let fixture = Fixture::new(replies);
    let (client, jira) = clients(&fixture);
    let session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    let range = session.snapshot().range;
    let templates = session
        .previous_month_templates(now())
        .await
        .expect("valid issue templates must remain available");
    assert_eq!(templates.rows().len(), 1);
    assert_eq!(templates.rows()[0].id.get(), 9);
    assert_eq!(session.snapshot().range.start, range.start);
    assert_eq!(session.snapshot().range.end, range.end);
    assert!(file(&config).current().is_none());
    assert_eq!(fixture.requests.lock().unwrap().len(), 8);
    assert!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.starts_with("GET "))
    );
}

#[tokio::test]
async fn a_preflight_failure_keeps_the_exact_ready_capture_until_one_explicit_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let service = crate::tempo::transport::tests::Fixture::new(vec![
        complete()[1].clone(),
        complete()[2].clone(),
        reply(200, &single_worklog().to_string()),
    ]);
    let identities = Fixture::new(vec![
        identity(),
        identity(),
        identity(),
        identity(),
        reply(503, "{}"),
        identity(),
    ]);
    let mut session = Session::connect_current_month_at(
        &config,
        Arc::new(service.client()),
        Arc::new(identities.client()),
        now(),
    )
    .await
    .unwrap();
    let captured = draft();
    let body = captured.encode().unwrap();
    assert!(session.submit(captured).await.is_err());
    assert_eq!(session.pending_phase(), Some(WritePhase::Ready));
    assert_eq!(session.pending_draft().unwrap().encode().unwrap(), body);
    let checkpoint = file(&config).current().unwrap().to_vec();
    assert!(!checkpoint.is_empty());
    assert!(
        service
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.starts_with("GET "))
    );
    assert!(!session.can_submit());
    let report = session.dispatch_prepared().await.unwrap();
    assert_eq!(report.phase, WritePhase::Confirmed);
    assert!(session.pending_phase().is_none() && file(&config).current().is_none());
    assert!(session.dispatch_prepared().await.is_err());
    let requests = service.requests.lock().unwrap();
    let posts: Vec<_> = requests
        .iter()
        .filter(|request| request.starts_with("POST "))
        .collect();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].split_once("\r\n\r\n").unwrap().1.as_bytes(), body);
}

#[tokio::test]
async fn last_month_templates_keep_the_view_select_latest_work_and_expand_without_writing() {
    for changed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let first = copy_source(9, 3600, "2026-09-19");
        let mut latest = copy_source(11, 1800, "2026-09-20");
        latest["startTime"] = json!("15:00:00");
        let mut summary = latest.clone();
        summary["attributes"] =
            json!({"self":"https://api.tempo.io/4/worklogs/11/work-attribute-values"});
        let mut expanded = latest;
        if changed {
            expanded["description"] = json!("Changed without a new timestamp");
        }
        let mut replies = complete();
        replies.extend([
            identity(),
            page(&[first, summary]),
            complete()[2].clone(),
            identity(),
            identity(),
            reply(200, &expanded.to_string()),
        ]);
        if !changed {
            replies.push(identity());
        }
        let fixture = Fixture::new(replies);
        let (client, jira) = clients(&fixture);
        let session = Session::connect_current_month_at(&config, client, jira, now())
            .await
            .unwrap();
        let original = session.snapshot().clone();
        let templates = session.previous_month_templates(now()).await.unwrap();
        assert_eq!(templates.range().start.to_string(), "2026-09-01");
        assert_eq!(templates.range().end.to_string(), "2026-09-30");
        assert_eq!(templates.rows().len(), 1);
        assert_eq!(templates.rows()[0].id.get(), 11);
        assert!(templates.rows()[0].attributes.is_none());
        let result = session
            .expand_template(&templates, super::super::WorklogId::parse(11).unwrap())
            .await;
        if changed {
            assert!(matches!(
                result,
                Err(SessionError::Domain(Error::ChangedWhileLoading))
            ));
        } else {
            let row = result.unwrap();
            assert_eq!(row.seconds, 1800);
            assert_eq!(row.attributes.unwrap()[0].value.as_deref(), Some("keep 界"));
        }
        assert!(Arc::ptr_eq(&original, session.snapshot()) && session.can_submit());
        assert!(file(&config).current().is_none());
        let requests = fixture.requests.lock().unwrap();
        assert_eq!(requests.len(), if changed { 10 } else { 11 });
        assert!(requests.iter().all(|request| request.starts_with("GET ")));
        assert!(requests[5].contains("from=2026-09-01") && requests[5].contains("to=2026-09-30"));
    }
}

#[tokio::test]
async fn templates_refuse_another_account_and_keep_the_original_range_without_writes() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let other = reply(
        200,
        r#"{"accountId":"synthetic-other","timeZone":"America/Winnipeg"}"#,
    );
    let mut replies = complete();
    replies.extend([other.clone(), page(&[]), complete()[2].clone(), other]);
    let fixture = Fixture::new(replies);
    let (client, jira) = clients(&fixture);
    let session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    let original = session.snapshot().clone();
    assert!(matches!(
        session.previous_month_templates(now()).await,
        Err(SessionError::Read(PlannerError::ChangedIdentity))
    ));
    assert!(Arc::ptr_eq(&original, session.snapshot()) && session.can_submit());
    assert!(file(&config).current().is_none());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 8);
    assert!(requests.iter().all(|request| request.starts_with("GET ")));
}

#[tokio::test]
async fn a_copy_confirmation_cannot_cross_credentials_profiles_or_the_account_day() {
    for change in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let source = copy_source(7, 3600, "2026-10-16");
        let schedule = complete()[2].clone();
        let mut replies = vec![
            page(std::slice::from_ref(&source)),
            schedule.clone(),
            page(std::slice::from_ref(&source)),
            schedule.clone(),
            reply(200, &source.to_string()),
        ];
        if change < 2 {
            replies.extend([page(&[source]), schedule]);
        }
        let service = crate::tempo::transport::tests::Fixture::new(replies);
        let identities = Fixture::new(vec![identity(); if change < 2 { 6 } else { 4 }]);
        let client = Arc::new(service.client());
        let jira = Arc::new(identities.client());
        let mut original =
            Session::connect_current_month_at(&config, client.clone(), jira.clone(), now())
                .await
                .unwrap();
        let plan = original
            .prepare_copy(vec![super::super::WorklogId::parse(7).unwrap()], now())
            .await
            .unwrap();
        let (report, expected) = if change < 2 {
            let target_config = if change == 1 {
                directory.path().join("another-profile.json")
            } else {
                config.clone()
            };
            let target_client = if change == 0 {
                Arc::new(service.client())
            } else {
                client
            };
            let mut replacement =
                Session::connect_current_month_at(&target_config, target_client, jira, now())
                    .await
                    .unwrap();
            (
                replacement.execute_copy(plan, now()).await,
                Error::WrongConnection,
            )
        } else {
            (
                original
                    .execute_copy(plan, now() + chrono::Duration::days(1))
                    .await,
                Error::ChangedWhileLoading,
            )
        };
        assert_eq!(report.problem, Some(SessionError::Domain(expected)));
        assert!(
            report.confirmed.is_empty()
                && report.unconfirmed.is_none()
                && report.rejected.is_none()
        );
        assert!(report.unattempted.len() == 1 && report.unattempted[0].get() == 7);
        assert!(file(&config).current().is_none());
        let requests = service.requests.lock().unwrap();
        assert_eq!(requests.len(), if change < 2 { 7 } else { 5 });
        assert!(requests.iter().all(|request| request.starts_with("GET ")));
    }
}

#[tokio::test]
async fn a_confirmed_copy_with_failed_totals_refresh_stays_confirmed_without_becoming_a_retry() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let source = copy_source(7, 3600, "2026-10-16");
    let mut created = copy_source(8, 3600, "2026-10-16");
    created["startTime"] = json!("09:00:00");
    let schedule = complete()[2].clone();
    let service = crate::tempo::transport::tests::Fixture::new(vec![
        page(std::slice::from_ref(&source)),
        schedule.clone(),
        page(std::slice::from_ref(&source)),
        schedule.clone(),
        reply(200, &source.to_string()),
        page(std::slice::from_ref(&source)),
        schedule,
        reply(200, &source.to_string()),
        reply(200, &created.to_string()),
        reply(503, "{}"),
    ]);
    let identities = Fixture::new(vec![identity(); 10]);
    let mut session = Session::connect_current_month_at(
        &config,
        Arc::new(service.client()),
        Arc::new(identities.client()),
        now(),
    )
    .await
    .unwrap();
    let id = super::super::WorklogId::parse(7).unwrap();
    let plan = session.prepare_copy(vec![id], now()).await.unwrap();
    let report = session.execute_copy(plan, now()).await;
    assert!(
        report.confirmed.len() == 1
            && report.confirmed[0].0.get() == 7
            && report.confirmed[0].1.get() == 8
    );
    assert!(
        report.unattempted.is_empty() && report.unconfirmed.is_none() && report.rejected.is_none()
    );
    assert!(report.refresh_problem.is_some() && !report.complete() && !session.can_submit());
    assert!(session.pending_phase().is_none() && file(&config).current().is_none());
    let before = service.requests.lock().unwrap().len();
    assert!(session.prepare_copy(vec![id], now()).await.is_err());
    let requests = service.requests.lock().unwrap();
    assert_eq!(requests.len(), before);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn copy_preparation_expands_unknown_attributes_but_rejects_same_timestamp_field_changes_without_writes()
 {
    for changed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let mut summary = copy_source(7, 3600, "2026-10-16");
        summary["attributes"] =
            json!({"self":"https://api.tempo.io/4/worklogs/7/work-attribute-values"});
        let mut expanded = copy_source(7, 3600, "2026-10-16");
        if changed {
            expanded["description"] = json!("Changed with the same timestamp");
        }
        let schedule = complete()[2].clone();
        let service = crate::tempo::transport::tests::Fixture::new(vec![
            page(&[summary.clone()]),
            schedule.clone(),
            page(&[summary]),
            schedule,
            reply(200, &expanded.to_string()),
        ]);
        let identity_fixture = Fixture::new(vec![identity(); 4]);
        let mut session = Session::connect_current_month_at(
            &config,
            Arc::new(service.client()),
            Arc::new(identity_fixture.client()),
            now(),
        )
        .await
        .unwrap();
        let original = session.snapshot().clone();
        let plan = session
            .prepare_copy(vec![super::super::WorklogId::parse(7).unwrap()], now())
            .await;
        if changed {
            assert!(matches!(
                plan,
                Err(SessionError::Domain(Error::ChangedWhileLoading))
            ));
            assert!(!session.can_submit());
        } else {
            let plan = plan.unwrap();
            assert_eq!(plan.date().to_string(), "2026-10-16");
            assert_eq!(plan.seconds(), 3600);
            assert_eq!(
                plan.sources()[0].attributes.as_ref().unwrap()[0]
                    .value
                    .as_deref(),
                Some("keep 界")
            );
            assert!(session.can_submit());
        }
        assert!(Arc::ptr_eq(&original, session.snapshot()));
        assert!(file(&config).current().is_none());
        let requests = service.requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        assert!(requests.iter().all(|request| request.starts_with("GET ")));
    }
}

#[tokio::test]
async fn copying_stops_at_a_lost_reply_and_separates_confirmed_uncertain_and_unattempted_entries() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let first = copy_source(7, 3600, "2026-10-15");
    let second = copy_source(8, 1800, "2026-10-15");
    let last = copy_source(11, 600, "2026-10-15");
    let existing = copy_source(9, 900, "2026-10-16");
    let initial = [
        first.clone(),
        second.clone(),
        last.clone(),
        existing.clone(),
    ];
    let mut created = copy_source(10, 3600, "2026-10-16");
    created["startTime"] = json!("08:15:00");
    let after_first = [
        first.clone(),
        second.clone(),
        last.clone(),
        existing,
        created.clone(),
    ];
    let schedule = complete()[2].clone();
    let service = crate::tempo::transport::tests::Fixture::new(vec![
        page(&initial),
        schedule.clone(),
        page(&initial),
        schedule.clone(),
        reply(200, &first.to_string()),
        reply(200, &second.to_string()),
        reply(200, &last.to_string()),
        page(&initial),
        schedule.clone(),
        reply(200, &first.to_string()),
        reply(200, &second.to_string()),
        reply(200, &last.to_string()),
        reply(200, &created.to_string()),
        page(&after_first),
        schedule,
        String::new(),
    ]);
    let identities = Fixture::new(vec![identity(); 14]);
    let mut session = Session::connect_current_month_at(
        &config,
        Arc::new(service.client()),
        Arc::new(identities.client()),
        now(),
    )
    .await
    .unwrap();
    let ids: Vec<_> = [7, 8, 11]
        .into_iter()
        .map(|id| super::super::WorklogId::parse(id).unwrap())
        .collect();
    let plan = session.prepare_copy(ids.clone(), now()).await.unwrap();
    let report = session.execute_copy(plan, now()).await;
    assert_eq!(report.confirmed.len(), 1);
    assert!(report.confirmed[0].0.get() == 7 && report.confirmed[0].1.get() == 10);
    assert!(report.unconfirmed.is_some_and(|id| id.get() == 8));
    assert!(
        report.rejected.is_none()
            && report.unattempted.len() == 1
            && report.unattempted[0].get() == 11
    );
    assert!(!report.complete() && report.transport_problem.is_some());
    assert!(session.pending_phase() == Some(WritePhase::Unconfirmed) && !session.can_submit());
    let path = file(&config).path().to_owned();
    let saved = std::fs::read(&path).unwrap();
    let before = service.requests.lock().unwrap().len();
    assert!(session.prepare_copy(ids, now()).await.is_err());
    assert!(session.dispatch_prepared().await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), saved);
    let requests = service.requests.lock().unwrap();
    assert_eq!(requests.len(), before);
    let posts: Vec<_> = requests
        .iter()
        .filter(|request| request.starts_with("POST "))
        .collect();
    assert_eq!(posts.len(), 2);
    let bodies: Vec<serde_json::Value> = posts
        .iter()
        .map(|request| serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap())
        .collect();
    assert_eq!(bodies[0]["startTime"], "08:15:00");
    assert_eq!(bodies[1]["startTime"], "09:15:00");
    assert_eq!(bodies[0]["timeSpentSeconds"], 3600);
    assert_eq!(bodies[1]["timeSpentSeconds"], 1800);
    assert_eq!(
        bodies[0]["attributes"],
        json!([{"key":"synthetic-custom","value":"keep 界"}])
    );
    assert_eq!(bodies[0]["billableSeconds"], 600);
}

fn editor_replies(catalog_id: u64, capitalization: bool) -> Vec<String> {
    let mut rows=vec![
        reply(200,r#"{"id":"19","key":"SYNTH-19","fields":{"summary":"Synthetic issue"}}"#),
        reply(200,r#"{"id":"11","key":"SYNTH"}"#),
        reply(200,r#"{"metadata":{"count":1},"results":[{"id":1,"account":{"self":"https://api.tempo.io/4/accounts/1"},"default":true,"scope":{"id":11,"type":"PROJECT"}}]}"#),
        reply(200,&json!({"metadata":{"count":1,"offset":0,"limit":50},"results":[{"id":catalog_id,"key":"SYNTHETIC","name":"Synthetic account","global":false,"status":"OPEN"}]}).to_string())];
    if capitalization {
        rows.push(reply(503, "{}"));
    }
    rows
}

#[tokio::test]
async fn summary_reads_keep_the_resolved_key_when_classification_fails_without_loading_billing() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut rows = complete();
    rows.extend([
        reply(
            200,
            r#"{"id":"19","key":"SYNTH-19","fields":{"summary":"Synthetic issue"}}"#,
        ),
        reply(503, "{}"),
    ]);
    let fixture = Fixture::new(rows);
    let (client, jira) = clients(&fixture);
    let session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    let previous = session.snapshot().clone();
    let info = session
        .issue_summary(IssueId::parse(19).unwrap())
        .await
        .unwrap();
    assert_eq!(info.issue.key, "SYNTH-19");
    assert_eq!(info.issue.summary.as_deref(), Some("Synthetic issue"));
    assert!(info.capitalization.is_err());
    assert!(Arc::ptr_eq(&previous, session.snapshot()));
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 6);
    assert!(requests.iter().all(|row| row.starts_with("GET ")));
    assert!(
        !requests
            .iter()
            .any(|row| row.contains("accounts") || row.contains("account-links"))
    );
}

#[tokio::test]
async fn editor_billing_uses_the_resolved_issue_project_and_keeps_failed_classification_unknown() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut rows = complete();
    rows.extend(editor_replies(1, true));
    let fixture = Fixture::new(rows);
    let (client, jira) = clients(&fixture);
    let session = Session::connect_current_month_at(&config, client.clone(), jira, now())
        .await
        .unwrap();
    let details = session.editor_issue("19").await.unwrap();
    assert_eq!(details.issue.key, "SYNTH-19");
    assert_eq!(details.billing.project.get(), 11);
    assert!(details.billing.connection == client.write_scope());
    assert_eq!(details.billing.choices.len(), 1);
    assert_eq!(details.billing.choices[0].account.key(), "SYNTHETIC");
    assert!(details.billing.choices[0].is_default);
    assert!(details.capitalization.is_err());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 9);
    assert!(requests[5].starts_with("GET /rest/api/3/project/SYNTH "));
    assert!(requests[6].starts_with("GET /4/account-links/project/11?includeGlobalAccounts=true "));
    assert!(requests.iter().all(|r| r.starts_with("GET ")));
}

#[tokio::test]
async fn an_incomplete_billing_catalog_cannot_publish_editor_choices_or_change_the_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut rows = complete();
    rows.extend(editor_replies(2, false));
    let fixture = Fixture::new(rows);
    let (client, jira) = clients(&fixture);
    let session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    let previous = session.snapshot().clone();
    assert!(session.editor_issue("19").await.is_err());
    assert!(Arc::ptr_eq(&previous, session.snapshot()));
    assert!(file(&config).current().is_none());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 8);
    assert!(requests.iter().all(|r| r.starts_with("GET ")));
}

#[tokio::test]
async fn failed_read_retains_the_exact_snapshot_and_blocks_a_new_write() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut replies = complete();
    replies.push(reply(503, "{}"));
    let fixture = Fixture::new(replies);
    let (client, jira) = clients(&fixture);
    let mut session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    let previous = session.snapshot().clone();
    let total = previous.summary.logged_seconds;
    assert!(total > 0 && session.can_submit());
    let other = previous.range.shift_month(1).unwrap();
    assert!(session.refresh(other, now()).await.is_err());
    assert!(Arc::ptr_eq(&previous, session.snapshot()));
    assert_eq!(session.snapshot().summary.logged_seconds, total);
    assert_eq!(session.snapshot().range.start, previous.range.start);
    assert_eq!(session.snapshot().range.end, previous.range.end);
    assert!(!session.can_submit());
    assert!(matches!(
        session.submit(draft()).await,
        Err(SessionError::Domain(Error::WriteBlocked))
    ));
    assert!(file(&config).current().is_none());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(|r| r.starts_with("GET ")));
}

#[tokio::test]
async fn another_author_is_rejected_before_a_journal_or_service_write() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let fixture = Fixture::new(complete());
    let (client, jira) = clients(&fixture);
    let mut session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    let other = Draft::create(
        AccountId::parse("another-user").unwrap(),
        IssueId::parse(19).unwrap(),
        "2026-10-16".parse().unwrap(),
        60,
    )
    .unwrap();
    assert!(matches!(
        session.submit(other).await,
        Err(SessionError::Domain(Error::WrongConnection))
    ));
    assert!(session.can_submit() && file(&config).current().is_none());
    assert_eq!(fixture.requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn lost_reply_cannot_resend_and_discard_requires_a_new_successful_read() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut replies = complete();
    replies.extend([identity(), identity(), identity(), String::new()]);
    replies.extend(complete());
    replies.extend(complete());
    let fixture = Fixture::new(replies);
    let (client, jira) = clients(&fixture);
    let mut session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    let report = session.submit(draft()).await.unwrap();
    assert!(report.phase == WritePhase::Unconfirmed);
    let bytes = std::fs::read(file(&config).path()).unwrap();
    assert!(session.pending_phase() == Some(WritePhase::Unconfirmed));
    assert!(session.pending_draft().is_some());
    assert!(matches!(
        session.dispatch_prepared().await,
        Err(SessionError::Change(ChangeError::Domain(
            Error::WriteBlocked
        )))
    ));
    assert!(session.credential_checkpoint().is_err());
    assert!(session.cleanup().is_err());
    assert_eq!(std::fs::read(file(&config).path()).unwrap(), bytes);
    let range = session.snapshot().range;
    session.refresh(range, now()).await.unwrap();
    assert!(!session.can_submit());
    let count = fixture.requests.lock().unwrap().len();
    session.discard().unwrap();
    assert_eq!(fixture.requests.lock().unwrap().len(), count);
    assert!(file(&config).current().is_none());
    assert!(!session.can_submit());
    session.refresh(range, now()).await.unwrap();
    assert!(session.can_submit());
    assert_eq!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn restored_checkpoint_is_exact_and_never_gains_a_dispatch_operation() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut replies = vec![identity(), identity()];
    replies.extend(complete());
    let fixture = Fixture::new(replies);
    let (client, jira) = clients(&fixture);
    let prepared = SavedChange::capture(client.clone(), jira.clone(), &config, draft())
        .await
        .unwrap();
    drop(prepared);
    let bytes = std::fs::read(file(&config).path()).unwrap();
    let mut session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    assert_eq!(
        session.credential_checkpoint().unwrap(),
        Some(bytes.as_slice())
    );
    assert!(!session.can_submit());
    assert!(session.pending_phase() == Some(WritePhase::Unconfirmed));
    assert!(session.dispatch_prepared().await.is_err());
    assert!(session.submit(draft()).await.is_err());
    assert!(session.cleanup().is_err());
    assert_eq!(std::fs::read(file(&config).path()).unwrap(), bytes);
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 6);
    assert!(requests.iter().all(|r| r.starts_with("GET ")));
}

#[tokio::test]
async fn corrupt_recovery_permits_read_only_inspection_and_preserves_the_owned_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut journal = file(&config);
    journal.persist(b"invalid saved recovery").unwrap();
    let path = journal.path().to_owned();
    let mut replies = vec![identity()];
    replies.extend(complete());
    let fixture = Fixture::new(replies);
    let (client, jira) = clients(&fixture);
    let mut session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    assert!(session.snapshot().summary.logged_seconds > 0);
    assert!(session.recovery_problem().is_some());
    assert!(!session.can_submit());
    assert!(session.credential_checkpoint().is_err());
    assert!(session.cleanup().is_err() && session.discard().is_err());
    assert!(session.submit(draft()).await.is_err());
    assert!(session.dispatch_prepared().await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"invalid saved recovery");
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(|r| r.starts_with("GET ")));
}

#[tokio::test]
async fn a_timezone_change_before_capture_preserves_the_snapshot_without_publishing_or_sending() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut replies = complete();
    replies.push(reply(
        200,
        r#"{"accountId":"123456:synthetic-user","timeZone":"Pacific/Kiritimati"}"#,
    ));
    let fixture = Fixture::new(replies);
    let (client, jira) = clients(&fixture);
    let mut session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    let previous = session.snapshot().clone();
    assert!(matches!(
        session.submit(draft()).await,
        Err(SessionError::Read(PlannerError::ChangedIdentity))
    ));
    assert!(Arc::ptr_eq(&previous, session.snapshot()));
    assert!(!session.can_submit());
    assert!(session.pending_phase().is_none() && file(&config).current().is_none());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(|r| r.starts_with("GET ")));
}

#[tokio::test]
async fn a_confirmed_write_retires_its_journal_but_requires_a_successful_refresh_before_the_next_write()
 {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut row = single_worklog();
    row["timeSpentSeconds"] = json!(3600);
    row["description"] = json!(null);
    let initial = complete();
    let service = crate::tempo::transport::tests::Fixture::new(vec![
        initial[1].clone(),
        initial[2].clone(),
        reply(200, &row.to_string()),
        initial[1].clone(),
        initial[2].clone(),
    ]);
    let identity_fixture = Fixture::new(vec![identity(); 7]);
    let client = Arc::new(service.client());
    let jira = Arc::new(identity_fixture.client());
    let mut session = Session::connect_current_month_at(&config, client, jira, now())
        .await
        .unwrap();
    let report = session.submit(draft()).await.unwrap();
    assert!(
        report.phase == WritePhase::Confirmed
            && report.acknowledgement_durable
            && report.cleanup_problem.is_none()
    );
    assert!(session.pending_phase().is_none() && file(&config).current().is_none());
    assert!(!session.can_submit());
    let count = service.requests.lock().unwrap().len();
    assert!(session.submit(draft()).await.is_err());
    assert_eq!(service.requests.lock().unwrap().len(), count);
    let range = session.snapshot().range;
    session.refresh(range, now()).await.unwrap();
    assert!(session.can_submit());
    let requests = service.requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests.iter().filter(|r| r.starts_with("POST ")).count(),
        1
    );
    let written = requests.iter().find(|r| r.starts_with("POST ")).unwrap();
    let body: serde_json::Value =
        serde_json::from_str(written.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["authorAccountId"], "123456:synthetic-user");
    assert_eq!(body["issueId"], 19);
    assert_eq!(body["timeSpentSeconds"], 3600);
    assert_eq!(body["attributes"], json!([]));
    let identities = identity_fixture.requests.lock().unwrap();
    assert_eq!(identities.len(), 7);
    assert!(identities.iter().all(|r| r.starts_with("GET ")));
}
