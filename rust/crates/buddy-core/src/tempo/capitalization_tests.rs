use super::*;
use crate::tempo::jira::tests::{Fixture, reply};
use serde_json::{Value, json};

fn issue(id: &str, key: &str, choice: Value, parent: Value) -> String {
    json!({"id":id,"key":key,"fields":{"customfield_11702":choice,"parent":parent}}).to_string()
}
fn client(fixture: &Fixture) -> JiraClient {
    JiraClient::build(
        fixture.base.clone(),
        "synthetic@example.invalid",
        "synthetic-jira-token",
        false,
    )
    .unwrap()
}
fn requested(fixture: &Fixture) -> Vec<String> {
    fixture
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.lines().next().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn explicit_choice_uses_exact_issue_identity_and_never_fetches_its_parent() {
    for (choice, expected) in [
        (json!({"value":"Yes"}), true),
        (json!([{"value":"No"}]), false),
    ] {
        let fixture = Fixture::new(vec![reply(
            200,
            &issue(
                "9007199254740993",
                "PROJ-1",
                choice,
                json!({"key":"EPIC-1"}),
            ),
        )]);
        assert_eq!(
            client(&fixture)
                .capitalization("9007199254740993")
                .await
                .unwrap(),
            Some(expected)
        );
        assert_eq!(
            requested(&fixture),
            [
                "GET /rest/api/3/issue/9007199254740993?fields=id,key,customfield_11702,parent HTTP/1.1"
            ]
        );
    }
}

#[tokio::test]
async fn absent_and_empty_choices_can_inherit_a_parent_but_no_parent_stays_unresolved() {
    for choice in [Value::Null, json!([]), json!({"value":""})] {
        let fixture = Fixture::new(vec![
            reply(200, &issue("1", "PROJ-1", choice, json!({"key":"EPIC-1"}))),
            reply(
                200,
                &issue("2", "EPIC-1", json!({"value":"Yes"}), Value::Null),
            ),
        ]);
        assert_eq!(
            client(&fixture).capitalization("PROJ-1").await.unwrap(),
            Some(true)
        );
        assert_eq!(requested(&fixture).len(), 2);
        assert!(requested(&fixture)[1].starts_with("GET /rest/api/3/issue/EPIC-1?"));
    }
    let fixture = Fixture::new(vec![reply(
        200,
        &issue("1", "PROJ-1", Value::Null, Value::Null),
    )]);
    assert_eq!(
        client(&fixture).capitalization("PROJ-1").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn lookup_failure_is_not_cached_as_non_capex_and_an_explicit_refresh_can_recover() {
    let fixture = Fixture::new(vec![
        reply(401, "{}"),
        reply(
            200,
            &issue("1", "PROJ-1", json!({"value":"Yes"}), Value::Null),
        ),
    ]);
    let client = client(&fixture);
    assert_eq!(
        client.capitalization("PROJ-1").await.unwrap_err(),
        JiraError::Authentication
    );
    assert_eq!(client.capitalization("PROJ-1").await.unwrap(), Some(true));
    assert_eq!(requested(&fixture).len(), 2);
}

#[tokio::test]
async fn unknown_or_ambiguous_choices_and_malformed_parents_do_not_become_false() {
    for (choice, parent) in [
        (json!({"value":"Maybe"}), Value::Null),
        (json!([{"value":"Yes"},{"value":"No"}]), Value::Null),
        (json!(false), Value::Null),
        (Value::Null, json!({"key":"//foreign.invalid"})),
    ] {
        let fixture = Fixture::new(vec![reply(200, &issue("1", "PROJ-1", choice, parent))]);
        assert_eq!(
            client(&fixture).capitalization("PROJ-1").await.unwrap_err(),
            JiraError::InvalidResponse
        );
        assert_eq!(requested(&fixture).len(), 1);
    }
}

#[tokio::test]
async fn wrong_identity_is_rejected_before_using_its_classification() {
    let fixture = Fixture::new(vec![reply(
        200,
        &issue("1", "OTHER-1", json!({"value":"Yes"}), Value::Null),
    )]);
    assert_eq!(
        client(&fixture).capitalization("PROJ-1").await.unwrap_err(),
        JiraError::WrongIssue
    );
}

#[tokio::test]
async fn parent_cycles_and_depth_are_bounded_without_following_an_arbitrary_url() {
    let fixture = Fixture::new(vec![
        reply(
            200,
            &issue("1", "PROJ-1", Value::Null, json!({"key":"EPIC-1"})),
        ),
        reply(
            200,
            &issue("2", "EPIC-1", Value::Null, json!({"key":"PROJ-1"})),
        ),
        reply(200, &issue("1", "PROJ-1", Value::Null, Value::Null)),
    ]);
    assert_eq!(
        client(&fixture).capitalization("PROJ-1").await.unwrap_err(),
        JiraError::InvalidResponse
    );
    assert_eq!(requested(&fixture).len(), 3);
    let responses = (1..=MAX_ANCESTORS)
        .map(|i| {
            reply(
                200,
                &issue(
                    &i.to_string(),
                    &format!("PROJ-{i}"),
                    Value::Null,
                    json!({"key":format!("PROJ-{}",i+1)}),
                ),
            )
        })
        .collect();
    let fixture = Fixture::new(responses);
    assert_eq!(
        client(&fixture).capitalization("PROJ-1").await.unwrap_err(),
        JiraError::InvalidResponse
    );
    assert_eq!(requested(&fixture).len(), MAX_ANCESTORS);
}

#[tokio::test]
async fn invalid_identifiers_fail_before_a_request_and_connections_do_not_share_results() {
    let fixture = Fixture::new(vec![]);
    assert_eq!(
        client(&fixture)
            .capitalization("PROJ-1?fields=all")
            .await
            .unwrap_err(),
        JiraError::InvalidIssue
    );
    assert!(requested(&fixture).is_empty());
    for expected in [true, false] {
        let fixture = Fixture::new(vec![reply(
            200,
            &issue(
                "1",
                "PROJ-1",
                json!({"value":if expected {"Yes"} else {"No"}}),
                Value::Null,
            ),
        )]);
        assert_eq!(
            client(&fixture).capitalization("PROJ-1").await.unwrap(),
            Some(expected)
        );
        assert_eq!(requested(&fixture).len(), 1);
    }
}
