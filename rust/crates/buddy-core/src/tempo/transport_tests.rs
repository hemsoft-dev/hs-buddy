use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::json;

use super::*;

#[tokio::test]
async fn regional_clients_bind_paging_accounts_and_expanded_attributes_to_their_origin() {
    for origin in [
        super::super::ApiOrigin::Global,
        super::super::ApiOrigin::Europe,
        super::super::ApiOrigin::Americas,
    ] {
        let regional = |value: String| value.replace("https://api.tempo.io", origin.as_str());
        let next = format!(
            "{}/4/worklogs/user/123456:synthetic-user?offset=50&limit=50",
            origin.as_str()
        );
        let mut detail = single_worklog();
        detail["attributes"]
            .as_object_mut()
            .unwrap()
            .remove("values");
        let service = Fixture::new(vec![
            reply(200, &regional(page(0, Some(&next)))),
            reply(200, &regional(page(50, None))),
            reply(200, &regional(project_links(11))),
            reply(200, &regional(account_page(0, 1, None))),
            reply(200, &regional(detail.to_string())),
            reply(200, &regional(attribute_response().to_string())),
            reply(200, &regional(detail.to_string())),
        ]);
        let client =
            Client::build_in_region(origin, "synthetic-tempo-token", service.base.clone(), false)
                .unwrap();
        assert_eq!(client.api_origin(), origin);
        let (author, range) = scope();
        assert!(
            client
                .worklogs(author.clone(), range)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            client
                .project_billing_accounts(super::super::JiraProjectId::parse(11).unwrap())
                .await
                .unwrap()
                .choices
                .len(),
            1
        );
        assert_eq!(
            client
                .worklog_for_edit(&author, WorklogId::parse(7).unwrap())
                .await
                .unwrap()
                .attributes
                .unwrap()
                .len(),
            2
        );
        let requests = service.requests.lock().unwrap();
        assert_eq!(requests.len(), 7);
        assert!(
            requests
                .iter()
                .all(|request| request.starts_with("GET /4/")
                    && !request.starts_with("GET https://"))
        );
        assert!(requests[1].starts_with("GET /4/worklogs/user/123456:synthetic-user?from=2026-10-16&to=2026-10-17&limit=50&offset=50 "));
    }
}

#[tokio::test]
async fn regional_clients_reject_another_cluster_before_following_a_continuation() {
    let next = "https://api.tempo.io/4/worklogs/user/123456:synthetic-user?offset=50";
    let service = Fixture::new(vec![reply(200, &page(0, Some(next)))]);
    let client = Client::build_in_region(
        super::super::ApiOrigin::Europe,
        "synthetic-tempo-token",
        service.base.clone(),
        false,
    )
    .unwrap();
    let (author, range) = scope();
    assert!(matches!(
        client.worklogs(author, range).await,
        Err(TransportError::Domain(Error::WrongScope))
    ));
    assert_eq!(service.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_long_retry_after_stops_without_an_early_automatic_read_retry() {
    let response =
        reply(429, "").replacen("Content-Length:", "Retry-After: 60\r\nContent-Length:", 1);
    let service = Fixture::new(vec![response]);
    let client = service.client();
    let (author, range) = scope();
    let started = Instant::now();
    assert!(matches!(
        client.worklogs(author.clone(), range).await,
        Err(TransportError::RateLimited)
    ));
    assert!(matches!(
        client.schedule(author, range).await,
        Err(TransportError::RateLimited)
    ));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(service.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn review_regression_a_rate_limited_read_backs_off_and_loads_the_complete_result() {
    let service = Fixture::new(vec![reply(429, ""), reply(200, &page(0, None))]);
    let client = service.client();
    let (author, range) = scope();
    let started = Instant::now();
    let result = client.worklogs(author, range).await;
    assert!(
        result.is_ok(),
        "a bounded read retry should recover from 429"
    );
    assert!(result.unwrap().is_empty());
    assert_eq!(service.requests.lock().unwrap().len(), 2);
    assert!(started.elapsed() >= Duration::from_millis(900));
}

#[tokio::test]
async fn review_regression_one_client_spaces_requests_across_different_read_methods() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let capture = observed.clone();
    let schedule =
        r#"{"self":"https://api.tempo.io/4/user-schedule","metadata":{"count":0},"results":[]}"#;
    let service = Fixture::with_before_reply(
        vec![
            reply(200, schedule),
            reply(200, &page(0, None)),
            reply(200, schedule),
        ],
        move |_| capture.lock().unwrap().push(Instant::now()),
    );
    let client = service.client();
    let (author, range) = scope();
    client.schedule(author.clone(), range).await.unwrap();
    client.worklogs(author.clone(), range).await.unwrap();
    client.schedule(author, range).await.unwrap();
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 3);
    assert!(
        observed
            .windows(2)
            .all(|pair| pair[1].duration_since(pair[0]) >= Duration::from_millis(900))
    );
}

pub(in crate::tempo) struct Fixture {
    pub(in crate::tempo) base: Url,
    pub(in crate::tempo) requests: Arc<Mutex<Vec<String>>>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[tokio::test]
async fn concurrent_reads_on_one_client_cannot_form_a_request_burst() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let capture = observed.clone();
    let schedule =
        r#"{"self":"https://api.tempo.io/4/user-schedule","metadata":{"count":0},"results":[]}"#;
    let service = Fixture::with_before_reply(vec![reply(200, schedule); 3], move |_| {
        capture.lock().unwrap().push(Instant::now())
    });
    let client = service.client();
    let (author, range) = scope();
    let results = tokio::join!(
        client.schedule(author.clone(), range),
        client.schedule(author.clone(), range),
        client.schedule(author, range)
    );
    assert!(results.0.is_ok() && results.1.is_ok() && results.2.is_ok());
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 3);
    assert!(
        observed
            .windows(2)
            .all(|pair| pair[1].duration_since(pair[0]) >= Duration::from_millis(900))
    );
}
impl Fixture {
    pub(in crate::tempo) fn new(responses: Vec<String>) -> Self {
        Self::with_before_reply(responses, |_| {})
    }
    pub(in crate::tempo) fn with_before_reply(
        responses: Vec<String>,
        mut before_reply: impl FnMut(usize) + Send + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/4/", listener.local_addr().unwrap())).unwrap();
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let capture = requests.clone();
        let join = std::thread::spawn(move || {
            for (index, response) in responses.into_iter().enumerate() {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() >= deadline {
                                return;
                            }
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => return,
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") && request.len() < 16384 {
                    if stream.read_exact(&mut byte).is_err() {
                        return;
                    }
                    request.push(byte[0]);
                }
                let headers = String::from_utf8(request.clone()).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                assert!(length <= 2 * 1024 * 1024);
                let mut body = vec![0; length];
                if stream.read_exact(&mut body).is_err() {
                    return;
                }
                request.extend_from_slice(&body);
                capture
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                before_reply(index);
                if stream.write_all(response.as_bytes()).is_err() {
                    return;
                }
            }
        });
        Self {
            base,
            requests,
            join: Some(join),
        }
    }
    pub(in crate::tempo) fn client(&self) -> Client {
        Client::build("synthetic-tempo-token", self.base.clone(), false).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.join.take().unwrap().join().unwrap();
    }
}
fn scope() -> (AccountId, DateRange) {
    (
        AccountId::parse("123456:synthetic-user").unwrap(),
        DateRange::new("2026-10-16".parse().unwrap(), "2026-10-17".parse().unwrap()).unwrap(),
    )
}
pub(in crate::tempo) fn reply(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n{body}",
        body.len()
    )
}

fn account_page(offset: usize, count: usize, next: Option<&str>) -> String {
    let rows: Vec<_> = (offset + 1..=offset + count)
        .map(|id| {
            json!({
                "id": id, "key": format!("SYNTHETIC-{id}"), "name":"Synthetic billing 雪",
                "global": false, "status":"OPEN"
            })
        })
        .collect();
    json!({"metadata":{"count":count,"offset":offset,"limit":50,"next":next},"results":rows})
        .to_string()
}

fn project_links(project: u64) -> String {
    json!({"metadata":{"count":1},"results":[{"id":3,"account":{"self":"https://api.tempo.io/4/accounts/1"},"default":true,"scope":{"id":project,"type":"PROJECT"}}]}).to_string()
}

#[tokio::test]
async fn project_account_choices_capture_project_connection_and_default_without_writes() {
    let fixture = Fixture::new(vec![
        reply(200, &project_links(9007199254740993)),
        reply(200, &account_page(0, 1, None)),
    ]);
    let client = fixture.client();
    let result = client
        .project_billing_accounts(crate::tempo::JiraProjectId::parse(9007199254740993).unwrap())
        .await
        .unwrap();
    assert!(result.connection == client.write_scope());
    assert_eq!(result.project.get(), 9007199254740993);
    assert_eq!(result.choices.len(), 1);
    assert!(result.choices[0].is_default);
    assert_eq!(result.choices[0].account.key(), "SYNTHETIC-1");
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with(
        "GET /4/account-links/project/9007199254740993?includeGlobalAccounts=true HTTP/1.1\r\n"
    ));
    assert!(requests[1].starts_with("GET /4/accounts?limit=50&offset=0 HTTP/1.1\r\n"));
}

#[tokio::test]
async fn project_account_choices_reject_wrong_project_before_loading_catalog() {
    let fixture = Fixture::new(vec![reply(200, &project_links(8))]);
    assert!(matches!(
        fixture
            .client()
            .project_billing_accounts(crate::tempo::JiraProjectId::parse(7).unwrap())
            .await,
        Err(TransportError::Domain(Error::WrongScope))
    ));
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn project_account_choices_propagate_catalog_failure_without_partial_result() {
    let fixture = Fixture::new(vec![
        reply(200, &project_links(7)),
        reply(401, "private rejected catalog"),
    ]);
    assert!(matches!(
        fixture
            .client()
            .project_billing_accounts(crate::tempo::JiraProjectId::parse(7).unwrap())
            .await,
        Err(TransportError::Authentication)
    ));
    assert_eq!(fixture.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn account_catalog_rebuilds_every_request_using_the_captured_connection() {
    let fixture = Fixture::new(vec![
        reply(
            200,
            &account_page(
                0,
                50,
                Some("https://api.tempo.io/4/accounts?offset=50&limit=50"),
            ),
        ),
        reply(200, &account_page(50, 1, None)),
    ]);
    let rows = fixture.client().billing_accounts().await.unwrap();
    assert_eq!(rows.len(), 51);
    assert_eq!(rows[50].id().get(), 51);
    assert_eq!(rows[50].key(), "SYNTHETIC-51");
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /4/accounts?limit=50&offset=0 HTTP/1.1\r\n"));
    assert!(requests[1].starts_with("GET /4/accounts?limit=50&offset=50 HTTP/1.1\r\n"));
    assert!(requests.iter().all(|request| {
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-tempo-token\r\n")
    }));
}

#[tokio::test]
async fn account_catalog_later_failure_never_publishes_partial_choices() {
    for (status, expected) in [
        (401, TransportError::Authentication),
        (429, TransportError::RateLimited),
        (500, TransportError::Service),
    ] {
        let mut responses = vec![reply(
            200,
            &account_page(0, 50, Some("https://api.tempo.io/4/accounts?offset=50")),
        )];
        responses.extend(vec![
            reply(
                status,
                "synthetic private response must not appear in errors"
            );
            if status == 429 { 4 } else { 1 }
        ]);
        let fixture = Fixture::new(responses);
        assert!(
            matches!(fixture.client().billing_accounts().await, Err(error) if error == expected)
        );
        assert_eq!(
            fixture.requests.lock().unwrap().len(),
            if status == 429 { 5 } else { 2 }
        );
        assert!(!expected.to_string().contains("synthetic private"));
    }
}

#[tokio::test]
async fn account_catalog_rejects_cross_origin_continuation_before_another_request() {
    let fixture = Fixture::new(vec![reply(
        200,
        &account_page(0, 50, Some("https://evil.invalid/4/accounts?offset=50")),
    )]);
    assert!(matches!(
        fixture.client().billing_accounts().await,
        Err(TransportError::Domain(Error::WrongScope))
    ));
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn account_catalog_rejects_http_redirect_without_leaking_the_captured_token() {
    let fixture = Fixture::new(vec!["HTTP/1.1 302 Fixture\r\nContent-Length: 0\r\nLocation: https://evil.invalid/4/accounts\r\nConnection: close\r\n\r\n".into()]);
    assert!(matches!(
        fixture.client().billing_accounts().await,
        Err(TransportError::Redirect)
    ));
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}
fn page(offset: usize, next: Option<&str>) -> String {
    json!({"self":"https://api.tempo.io/4/worklogs","metadata":{"count":0,"limit":50,"offset":offset,"next":next},"results":[]}).to_string()
}

pub(in crate::tempo) fn single_worklog() -> serde_json::Value {
    json!({
        "tempoWorklogId":7,"issue":{"id":19,"self":"https://jira.example.invalid/rest/api/3/issue/19"},
        "author":{"accountId":"123456:synthetic-user"},"timeSpentSeconds":90061,
        "startDate":"2026-10-16","description":"synthetic private description",
        "createdAt":"2026-10-16T01:00:00Z","updatedAt":"2026-10-16T02:00:00Z",
        "attributes":{"self":"https://api.tempo.io/4/worklogs/7/work-attribute-values","values":[]},
        "self":"https://api.tempo.io/4/worklogs/7"
    })
}

fn unexpanded_worklog() -> serde_json::Value {
    let mut row = single_worklog();
    row["attributes"].as_object_mut().unwrap().remove("values");
    row
}
fn attribute_response() -> serde_json::Value {
    json!({"metadata":{"count":2}, "self":"https://api.tempo.io/4/worklogs/7/work-attribute-values", "results":[
        {"key":"_Account_", "value":"synthetic-billing"}, {"key":"_Custom_"}
    ]})
}

#[tokio::test]
async fn edit_loading_expands_attributes_and_retains_unrelated_optional_values() {
    let original = unexpanded_worklog().to_string();
    let fixture = Fixture::new(vec![
        reply(200, &original),
        reply(200, &attribute_response().to_string()),
        reply(200, &original),
    ]);
    let row = fixture
        .client()
        .worklog_for_edit(&scope().0, WorklogId::parse(7).unwrap())
        .await
        .unwrap();
    let mut draft = crate::tempo::Draft::edit(&row).unwrap();
    draft.set_billing_account(Some("changed-billing")).unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&draft.encode().unwrap()).unwrap();
    assert!(payload["attributes"][0]["value"] == "changed-billing");
    assert!(
        payload["attributes"][1]["key"] == "_Custom_"
            && payload["attributes"][1].get("value").is_none()
    );
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].starts_with("GET /4/worklogs/7/work-attribute-values HTTP/1.1\r\n"));
    assert!(requests.iter().all(|request| request.starts_with("GET ")));
}

#[tokio::test]
async fn edit_loading_rejects_a_changed_record_even_when_its_timestamp_is_unchanged() {
    let original = unexpanded_worklog();
    let mut changed = original.clone();
    changed["description"] = json!("another synthetic edit");
    let fixture = Fixture::new(vec![
        reply(200, &original.to_string()),
        reply(200, &attribute_response().to_string()),
        reply(200, &changed.to_string()),
    ]);
    assert!(matches!(
        fixture
            .client()
            .worklog_for_edit(&scope().0, WorklogId::parse(7).unwrap())
            .await,
        Err(TransportError::Domain(Error::ChangedWhileLoading))
    ));
    assert_eq!(fixture.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn incomplete_or_duplicate_attributes_cannot_form_a_replacement_edit() {
    for duplicate in [false, true] {
        let mut attrs = attribute_response();
        if duplicate {
            attrs["results"][1]["key"] = json!("_Account_");
        } else {
            attrs["metadata"]["count"] = json!(3);
        }
        let fixture = Fixture::new(vec![
            reply(200, &unexpanded_worklog().to_string()),
            reply(200, &attrs.to_string()),
        ]);
        assert!(matches!(
            fixture
                .client()
                .worklog_for_edit(&scope().0, WorklogId::parse(7).unwrap())
                .await,
            Err(TransportError::Domain(Error::InvalidResponse))
        ));
        assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn attributes_for_another_record_or_origin_cannot_be_attached_to_this_edit() {
    for reference in [
        "https://api.tempo.io/4/worklogs/8/work-attribute-values",
        "https://example.invalid/4/worklogs/7/work-attribute-values",
    ] {
        let mut attrs = attribute_response();
        attrs["self"] = json!(reference);
        let fixture = Fixture::new(vec![
            reply(200, &unexpanded_worklog().to_string()),
            reply(200, &attrs.to_string()),
        ]);
        assert!(matches!(
            fixture
                .client()
                .worklog_for_edit(&scope().0, WorklogId::parse(7).unwrap())
                .await,
            Err(TransportError::Domain(Error::WrongScope))
        ));
        assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn a_record_with_expanded_attributes_needs_no_second_attribute_request() {
    let fixture = Fixture::new(vec![reply(200, &single_worklog().to_string())]);
    assert!(
        fixture
            .client()
            .worklog_for_edit(&scope().0, WorklogId::parse(7).unwrap())
            .await
            .unwrap()
            .attributes
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn individual_read_preserves_record_fields_and_uses_only_the_captured_id() {
    let fixture = Fixture::new(vec![reply(200, &single_worklog().to_string())]);
    let (author, _) = scope();
    let id = WorklogId::parse(7).unwrap();
    let row = fixture.client().worklog(&author, id).await.unwrap();
    assert!(row.id == id && row.author == author);
    assert!(row.seconds == 90061 && row.issue.unwrap().get() == 19);
    assert!(row.description.as_deref() == Some("synthetic private description"));
    assert!(row.attributes.unwrap().is_empty());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /4/worklogs/7 HTTP/1.1\r\n"));
}

#[tokio::test]
async fn individual_read_rejects_another_record_or_author_instead_of_selecting_it() {
    for field in ["tempoWorklogId", "author"] {
        let mut body = single_worklog();
        body[field] = if field == "author" {
            json!({"accountId":"another-synthetic-user"})
        } else {
            json!(8)
        };
        let fixture = Fixture::new(vec![reply(200, &body.to_string())]);
        let result = fixture
            .client()
            .worklog(&scope().0, WorklogId::parse(7).unwrap())
            .await;
        assert!(matches!(
            result,
            Err(TransportError::Domain(Error::WrongScope))
        ));
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn missing_individual_record_returns_a_static_error_without_retrying() {
    let fixture = Fixture::new(vec![reply(404, "synthetic-private-server-error")]);
    let result = fixture
        .client()
        .worklog(&scope().0, WorklogId::parse(7).unwrap())
        .await;
    assert!(matches!(result, Err(TransportError::NotFound)));
    assert!(
        !TransportError::NotFound
            .to_string()
            .contains("synthetic-private")
    );
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn each_page_reapplies_the_original_user_date_range_and_limit() {
    let fixture = Fixture::new(vec![
        reply(
            200,
            &page(
                0,
                Some("https://api.tempo.io/4/worklogs/user/123456:synthetic-user?offset=50"),
            ),
        ),
        reply(200, &page(50, None)),
    ]);
    let client = fixture.client();
    assert!(client.authorization.is_sensitive());
    let (author, range) = scope();
    assert!(client.worklogs(author, range).await.unwrap().is_empty());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for (index, request) in requests.iter().enumerate() {
        let target = request
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        let url = fixture.base.join(target).unwrap();
        assert!(url.path() == "/4/worklogs/user/123456:synthetic-user");
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert!(
            query.len() == 4
                && query["from"] == "2026-10-16"
                && query["to"] == "2026-10-17"
                && query["limit"] == "50"
                && query["offset"] == (index * 50).to_string()
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-tempo-token\r\n")
        );
    }
}

#[tokio::test]
async fn next_link_scope_changes_fail_before_another_request() {
    let fixture = Fixture::new(vec![reply(
        200,
        &page(
            0,
            Some("https://unrelated.invalid/4/worklogs/user/123456:synthetic-user?offset=50"),
        ),
    )]);
    let (author, range) = scope();
    assert!(matches!(
        fixture.client().worklogs(author, range).await,
        Err(TransportError::Domain(Error::WrongScope))
    ));
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn status_failures_are_static_with_bounded_rate_retries_and_no_redirects() {
    for (status, expected) in [
        (302, TransportError::Redirect),
        (401, TransportError::Authentication),
        (403, TransportError::Authentication),
        (429, TransportError::RateLimited),
        (500, TransportError::Service),
    ] {
        let response = reply(status, "private server detail").replacen(
            "Content-Length:",
            "Location: http://unrelated.invalid/\r\nContent-Length:",
            1,
        );
        let attempts = if status == 429 { 4 } else { 1 };
        let fixture = Fixture::new(vec![response; attempts]);
        let (author, range) = scope();
        assert!(
            matches!(fixture.client().worklogs(author, range).await, Err(error) if error == expected)
        );
        assert!(!expected.to_string().contains("private server detail"));
        assert_eq!(fixture.requests.lock().unwrap().len(), attempts);
    }
}

#[tokio::test]
async fn declared_oversize_and_bad_json_cannot_be_empty_successes() {
    for (response, expected) in [
        (
            format!(
                "HTTP/1.1 200 Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_RESPONSE_BYTES + 1
            ),
            Error::TooLarge,
        ),
        (
            format!(
                "HTTP/1.1 200 Fixture\r\nConnection: close\r\n\r\n{}",
                "x".repeat(MAX_RESPONSE_BYTES + 1)
            ),
            Error::TooLarge,
        ),
        (
            reply(200, "private invalid response"),
            Error::InvalidResponse,
        ),
    ] {
        let fixture = Fixture::new(vec![response]);
        let (author, range) = scope();
        assert!(
            matches!(fixture.client().worklogs(author, range).await, Err(TransportError::Domain(error)) if error == expected)
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn credentials_are_bounded_before_http_and_the_public_origin_is_fixed() {
    for token in [
        "".to_string(),
        "contains space".into(),
        "line\nbreak".into(),
        "x".repeat(4097),
    ] {
        assert!(matches!(
            Client::new(&token),
            Err(TransportError::InvalidCredential)
        ));
    }
    let client = Client::new("synthetic-tempo-token").unwrap();
    assert!(
        client.base.as_str() == "https://api.tempo.io/4/" && client.authorization.is_sensitive()
    );
}

#[tokio::test]
async fn schedule_reads_capture_user_and_range_and_reject_out_of_scope_days() {
    for date in ["2026-10-16", "2026-10-15"] {
        let body = json!({"self":"https://api.tempo.io/4/user-schedule", "metadata":{"count":1},
            "results":[{"date":date,"requiredSeconds":28800,"type":"WORKING_DAY"}]})
        .to_string();
        let fixture = Fixture::new(vec![reply(200, &body)]);
        let (author, range) = scope();
        let result = fixture.client().schedule(author, range).await;
        if date == "2026-10-16" {
            let rows = result.unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].required_seconds, 28800);
        } else {
            assert!(matches!(
                result,
                Err(TransportError::Domain(Error::WrongScope))
            ));
        }
        let requests = fixture.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let target = requests[0]
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        let url = fixture.base.join(target).unwrap();
        assert!(url.path() == "/4/user-schedule/123456:synthetic-user");
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert!(query.len() == 2 && query["from"] == "2026-10-16" && query["to"] == "2026-10-17");
    }
}
