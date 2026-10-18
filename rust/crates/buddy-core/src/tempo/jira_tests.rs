use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::*;

pub(in crate::tempo) struct Fixture {
    pub(in crate::tempo) base: Url,
    pub(in crate::tempo) requests: Arc<Mutex<Vec<String>>>,
    join: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    pub(in crate::tempo) fn new(responses: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let capture = requests.clone();
        let join = std::thread::spawn(move || {
            for response in responses {
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
                capture
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
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
    pub(in crate::tempo) fn client(&self) -> JiraClient {
        JiraClient::build(
            self.base.clone(),
            "synthetic@example.invalid",
            "synthetic-jira-token",
            false,
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.join.take().unwrap().join().unwrap();
    }
}

pub(in crate::tempo) fn reply(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n{body}",
        body.len()
    )
}

fn recovery_profile() -> &'static std::path::Path {
    std::path::Path::new(if cfg!(windows) {
        "D:/synthetic-unused/tempo-recovery.json"
    } else {
        "/synthetic-unused/tempo-recovery.json"
    })
}

#[tokio::test]
async fn project_lookup_preserves_large_numeric_identity_and_case_sensitive_key() {
    let fixture = Fixture::new(vec![
        reply(200, r#"{"id":"9007199254740993","key":"SYNTHETIC"}"#),
        reply(200, r#"{"id":"9007199254740993","key":"SYNTHETIC"}"#),
    ]);
    let client = fixture.client();
    assert_eq!(
        client.project("SYNTHETIC").await.unwrap().id.get(),
        9007199254740993
    );
    assert_eq!(
        client.project("9007199254740993").await.unwrap().key,
        "SYNTHETIC"
    );
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /rest/api/3/project/SYNTHETIC HTTP/1.1\r\n"));
    assert!(requests[1].starts_with("GET /rest/api/3/project/9007199254740993 HTTP/1.1\r\n"));
}

#[tokio::test]
async fn project_lookup_rejects_wrong_or_malformed_identity() {
    for (requested, body, expected) in [
        (
            "SYNTHETIC",
            r#"{"id":"7","key":"OTHER"}"#,
            JiraError::WrongProject,
        ),
        (
            "7",
            r#"{"id":"8","key":"SYNTHETIC"}"#,
            JiraError::WrongProject,
        ),
        (
            "synthetic",
            r#"{"id":"7","key":"SYNTHETIC"}"#,
            JiraError::WrongProject,
        ),
        (
            "SYNTHETIC",
            r#"{"id":7,"key":"SYNTHETIC"}"#,
            JiraError::InvalidResponse,
        ),
        (
            "SYNTHETIC",
            r#"{"id":"0","key":"SYNTHETIC"}"#,
            JiraError::InvalidResponse,
        ),
    ] {
        let fixture = Fixture::new(vec![reply(200, body)]);
        assert!(
            matches!(fixture.client().project(requested).await, Err(error) if error == expected)
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn project_lookup_has_no_global_success_or_authentication_failure_cache() {
    let fixture = Fixture::new(vec![
        reply(401, "private auth response"),
        reply(200, r#"{"id":"7","key":"SYNTHETIC"}"#),
    ]);
    let client = fixture.client();
    assert!(matches!(
        client.project("SYNTHETIC").await,
        Err(JiraError::Authentication)
    ));
    assert_eq!(client.project("SYNTHETIC").await.unwrap().id.get(), 7);
    let other = Fixture::new(vec![reply(200, r#"{"id":"9","key":"SYNTHETIC"}"#)]);
    assert_eq!(
        other.client().project("SYNTHETIC").await.unwrap().id.get(),
        9
    );
    for invalid in [
        "",
        "../SYNTHETIC",
        "EX?token=x",
        "0",
        "9223372036854775808",
        "EX-1",
    ] {
        assert!(matches!(
            client.project(invalid).await,
            Err(JiraError::InvalidProject)
        ));
    }
    assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    assert_eq!(other.requests.lock().unwrap().len(), 1);
}
fn recovery_draft(author: &str) -> crate::tempo::Draft {
    crate::tempo::Draft::create(
        AccountId::parse(author).unwrap(),
        IssueId::parse(123).unwrap(),
        "2026-10-16".parse().unwrap(),
        600,
    )
    .unwrap()
}

#[tokio::test]
async fn checkpoint_and_inspection_reverify_identity_without_tempo_writes_or_cached_auth_fallback()
{
    use crate::tempo::recovery::{self, RecoveryError};
    use crate::tempo::{Client, WritePhase};
    let fixture = Fixture::new(vec![
        reply(200, r#"{"accountId":"synthetic-user"}"#),
        reply(200, r#"{"accountId":"synthetic-user"}"#),
        reply(401, "private authentication body"),
    ]);
    let client = Client::new("synthetic-tempo-secret").unwrap();
    let attempt = client.capture(recovery_draft("synthetic-user")).unwrap();
    let bytes = client
        .checkpoint(&attempt, recovery_profile(), &fixture.client())
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-tempo-secret"));
    assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-jira-token"));
    let restored = recovery::inspect(&bytes, recovery_profile(), &fixture.client())
        .await
        .unwrap();
    assert_eq!(restored.phase(), WritePhase::Unconfirmed);
    assert!(matches!(
        recovery::inspect(&bytes, recovery_profile(), &fixture.client()).await,
        Err(RecoveryError::Identity(JiraError::Authentication))
    ));
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|request| request.starts_with("GET /rest/api/3/myself HTTP/1.1\r\n"))
    );
}

#[tokio::test]
async fn inspection_rejects_another_profile_origin_or_current_author_after_live_identity_read() {
    use crate::tempo::recovery::{self, RecoveryError};
    use crate::tempo::{Client, Error};
    let fixture = Fixture::new(vec![
        reply(200, r#"{"accountId":"synthetic-user"}"#),
        reply(200, r#"{"accountId":"synthetic-user"}"#),
        reply(200, r#"{"accountId":"foreign-user"}"#),
    ]);
    let client = Client::new("synthetic-tempo-token").unwrap();
    let attempt = client.capture(recovery_draft("synthetic-user")).unwrap();
    let bytes = client
        .checkpoint(&attempt, recovery_profile(), &fixture.client())
        .await
        .unwrap();
    let other_path = recovery_profile().with_file_name("other-profile.json");
    assert!(matches!(
        recovery::inspect(&bytes, &other_path, &fixture.client()).await,
        Err(RecoveryError::Domain(Error::WrongConnection))
    ));
    assert!(matches!(
        recovery::inspect(&bytes, recovery_profile(), &fixture.client()).await,
        Err(RecoveryError::Domain(Error::WrongConnection))
    ));
    let other_origin = Fixture::new(vec![reply(200, r#"{"accountId":"synthetic-user"}"#)]);
    assert!(matches!(
        recovery::inspect(&bytes, recovery_profile(), &other_origin.client()).await,
        Err(RecoveryError::Domain(Error::WrongConnection))
    ));
    assert_eq!(fixture.requests.lock().unwrap().len(), 3);
    assert_eq!(other_origin.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_foreign_tempo_connection_or_draft_author_cannot_checkpoint_another_accounts_change() {
    use crate::tempo::recovery::RecoveryError;
    use crate::tempo::{Client, Error};
    let fixture = Fixture::new(vec![reply(200, r#"{"accountId":"synthetic-user"}"#)]);
    let client = Client::new("synthetic-tempo-token").unwrap();
    let other = Client::new("other-synthetic-token").unwrap();
    let attempt = client.capture(recovery_draft("synthetic-user")).unwrap();
    assert!(matches!(
        other
            .checkpoint(&attempt, recovery_profile(), &fixture.client())
            .await,
        Err(RecoveryError::Domain(Error::WrongConnection))
    ));
    assert!(fixture.requests.lock().unwrap().is_empty());
    let foreign = client.capture(recovery_draft("foreign-user")).unwrap();
    assert!(matches!(
        client
            .checkpoint(&foreign, recovery_profile(), &fixture.client())
            .await,
        Err(RecoveryError::Domain(Error::WrongConnection))
    ));
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[test]
fn credentials_require_a_bare_https_origin_without_embedded_auth_or_unsafe_fields() {
    for origin in [
        "http://jira.example.invalid",
        "https://user:secret@jira.example.invalid",
        "https://jira.example.invalid/path",
        "https://jira.example.invalid?token=private",
        "https://jira.example.invalid/#private",
    ] {
        assert!(matches!(
            JiraClient::new(origin, "synthetic@example.invalid", "token"),
            Err(JiraError::InvalidConnection)
        ));
    }
    for (email, token) in [
        ("", "token"),
        ("user:other", "token"),
        ("user\r\n", "token"),
        ("user", ""),
        ("user", "token\n"),
    ] {
        assert!(matches!(
            JiraClient::new("https://jira.example.invalid", email, token),
            Err(JiraError::InvalidConnection)
        ));
    }
    let client = JiraClient::new(
        "https://jira.example.invalid",
        "synthetic@example.invalid",
        "token",
    )
    .unwrap();
    assert_eq!(client.origin(), "https://jira.example.invalid/");
}

#[tokio::test]
async fn profile_preserves_the_service_zone_and_does_not_invent_an_absent_one() {
    let fixture = Fixture::new(vec![
        reply(
            200,
            r#"{"accountId":"synthetic-user","timeZone":"America/New_York"}"#,
        ),
        reply(200, r#"{"accountId":"synthetic-user"}"#),
    ]);
    let client = fixture.client();
    let first = client.myself().await.unwrap();
    assert!(
        first.account.as_str() == "synthetic-user"
            && first.time_zone.as_deref() == Some("America/New_York")
    );
    assert!(client.myself().await.unwrap().time_zone.is_none());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.starts_with("GET /rest/api/3/myself HTTP/1.1\r\n"))
    );
}

#[tokio::test]
async fn authentication_failure_cannot_reuse_a_previous_profile_or_another_connection() {
    let first = Fixture::new(vec![
        reply(200, r#"{"accountId":"first-user"}"#),
        reply(401, "private-error"),
    ]);
    let second = Fixture::new(vec![reply(200, r#"{"accountId":"second-user"}"#)]);
    let client = first.client();
    assert!(client.myself().await.unwrap().account.as_str() == "first-user");
    assert!(matches!(
        client.myself().await,
        Err(JiraError::Authentication)
    ));
    assert!(second.client().myself().await.unwrap().account.as_str() == "second-user");
    assert_eq!(first.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn issue_selection_keeps_large_integer_identity_and_optional_summary() {
    let fixture = Fixture::new(vec![
        reply(
            200,
            r#"{"id":"9007199254740993","key":"TEST-7","fields":{"summary":"Synthetic issue"}}"#,
        ),
        reply(200, r#"{"id":"9007199254740993","key":"TEST-7"}"#),
    ]);
    let client = fixture.client();
    let issue = client.issue("test-7").await.unwrap();
    assert!(
        issue.id.get() == 9007199254740993
            && issue.key == "TEST-7"
            && issue.summary.as_deref() == Some("Synthetic issue")
    );
    assert!(
        client
            .issue("9007199254740993")
            .await
            .unwrap()
            .summary
            .is_none()
    );
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0].starts_with("GET /rest/api/3/issue/test-7?fields=id,key,summary HTTP/1.1\r\n")
    );
}

#[tokio::test]
async fn mismatched_issue_identity_and_invalid_int64_ids_are_rejected() {
    for (input, body, expected) in [
        (
            "TEST-7",
            r#"{"id":"7","key":"OTHER-8"}"#,
            JiraError::WrongIssue,
        ),
        ("7", r#"{"id":"8","key":"TEST-7"}"#, JiraError::WrongIssue),
        (
            "TEST-7",
            r#"{"id":"9223372036854775808","key":"TEST-7"}"#,
            JiraError::InvalidResponse,
        ),
        (
            "TEST-7",
            r#"{"id":"0","key":"TEST-7"}"#,
            JiraError::InvalidResponse,
        ),
        (
            "TEST-7",
            r#"{"id":"7","key":"TEST-7","fields":{"summary":42}}"#,
            JiraError::InvalidResponse,
        ),
    ] {
        let fixture = Fixture::new(vec![reply(200, body)]);
        assert!(matches!(fixture.client().issue(input).await, Err(error) if error == expected));
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn invalid_issue_input_does_not_start_a_request() {
    let fixture = Fixture::new(vec![]);
    for input in [
        "",
        "../myself",
        "TEST-7?token=private",
        "TEST/7",
        "TEST-7\n",
    ] {
        assert!(matches!(
            fixture.client().issue(input).await,
            Err(JiraError::InvalidIssue)
        ));
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn missing_identity_and_invalid_profile_fields_do_not_become_usable_accounts() {
    for body in [
        r#"{}"#,
        r#"{"accountId":""}"#,
        r#"{"accountId":"user","timeZone":""}"#,
        r#"{"accountId":"user","timeZone":"private\nzone"}"#,
    ] {
        let fixture = Fixture::new(vec![reply(200, body)]);
        assert!(matches!(
            fixture.client().myself().await,
            Err(JiraError::InvalidResponse)
        ));
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn redirects_are_not_followed_and_private_error_bodies_stay_out_of_diagnostics() {
    for (response, expected) in [
        ("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(), JiraError::Redirect),
        (reply(429, "synthetic-private-error"), JiraError::RateLimited),
        (reply(404, "synthetic-private-error"), JiraError::NotFound),
        (reply(200, "synthetic-private-error"), JiraError::InvalidResponse),
    ] {
        let fixture = Fixture::new(vec![response]);
        let error = match fixture.client().issue("TEST-7").await { Err(error) => error, Ok(_) => panic!("unexpected success") };
        assert_eq!(error, expected);
        assert!(!error.to_string().contains("synthetic-private"));
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn both_declared_and_streamed_oversize_responses_are_bounded() {
    let huge = "x".repeat(MAX_RESPONSE_BYTES + 1);
    for response in [
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            huge.len()
        ),
        format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{huge}"),
    ] {
        let fixture = Fixture::new(vec![response]);
        assert!(matches!(
            fixture.client().myself().await,
            Err(JiraError::TooLarge)
        ));
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}
