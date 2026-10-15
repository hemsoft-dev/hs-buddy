//! Explicit recipient lookup and a consumed confirmation for Slack PR nudges.
//! API contracts: https://docs.slack.dev/reference/methods/users.lookupByEmail/
//! https://docs.slack.dev/reference/methods/auth.test/
//! https://docs.slack.dev/reference/methods/conversations.open/
//! https://docs.slack.dev/reference/methods/chat.postMessage/
use super::{PullRef, RequestTicket, SelectionState};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NudgeError {
    #[error("Configure SLACK_BOT_TOKEN or SLACK_TOKEN, then restart Buddy to send PR nudges.")]
    MissingToken,
    #[error(
        "Enter the author's Slack email address. Buddy does not guess private email addresses."
    )]
    Email,
    #[error(
        "The Slack credential is invalid or inactive. Restore its workspace access before retrying."
    )]
    Authentication,
    #[error("No active Slack user matches this email address in the selected workspace.")]
    Recipient,
    #[error("Slack denied this request. Check the app's email lookup, DM, and message scopes.")]
    Denied,
    #[error("Slack is limiting requests. Wait before trying again.")]
    RateLimited,
    #[error("Slack could not be reached. Check the connection before retrying.")]
    Unavailable,
    #[error("Slack may have sent the message. Check the conversation before trying again.")]
    UnknownSend,
    #[error("The account or PR changed. Look up the recipient again before sending.")]
    Stale,
}

pub type SlackFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, NudgeError>> + Send + 'a>>;
pub trait SlackTransport: Send + Sync {
    fn call(&self, method: &'static str, body: Value) -> SlackFuture<'_>;
}

struct HttpSlack {
    client: reqwest::Client,
    authorization: HeaderValue,
}
impl SlackTransport for HttpSlack {
    fn call(&self, method: &'static str, body: Value) -> SlackFuture<'_> {
        Box::pin(async move {
            let writing = method == "chat.postMessage";
            let failure = || {
                if writing {
                    NudgeError::UnknownSend
                } else {
                    NudgeError::Unavailable
                }
            };
            let mut response = self
                .client
                .post(format!("https://slack.com/api/{method}"))
                .header(AUTHORIZATION, self.authorization.clone())
                .json(&body)
                .send()
                .await
                .map_err(|_| failure())?;
            if response.status().as_u16() == 429 {
                return Err(NudgeError::RateLimited);
            }
            if response.status().as_u16() == 401 {
                return Err(NudgeError::Authentication);
            }
            if !response.status().is_success() {
                return Err(failure());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| failure())? {
                if bytes.len() + chunk.len() > 128 * 1024 {
                    return Err(failure());
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| failure())?;
            checked_response(value, writing)
        })
    }
}

fn checked_response(value: Value, writing: bool) -> Result<Value, NudgeError> {
    if value["ok"].as_bool() == Some(true) {
        return Ok(value);
    }
    Err(match value["error"].as_str() {
        Some(
            "account_inactive" | "invalid_auth" | "not_authed" | "token_expired" | "token_revoked",
        ) => NudgeError::Authentication,
        Some("users_not_found" | "user_not_found") => NudgeError::Recipient,
        Some(
            "missing_scope"
            | "no_permission"
            | "access_denied"
            | "not_allowed_token_type"
            | "restricted_action",
        ) => NudgeError::Denied,
        Some("ratelimited" | "rate_limited") => NudgeError::RateLimited,
        _ if writing => NudgeError::UnknownSend,
        _ => NudgeError::Unavailable,
    })
}

#[derive(Clone)]
pub struct NudgeClient {
    transport: Arc<dyn SlackTransport>,
}
impl NudgeClient {
    pub fn from_environment() -> Result<Self, NudgeError> {
        let token = [
            "SLACK_BOT_TOKEN",
            "SLACK_TOKEN",
            "SLACK_RAE_BOT_USER_OAUTH_TOKEN",
        ]
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .ok_or(NudgeError::MissingToken)?;
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", token.trim()))
            .map_err(|_| NudgeError::Authentication)?;
        authorization.set_sensitive(true);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| NudgeError::Unavailable)?;
        Ok(Self {
            transport: Arc::new(HttpSlack {
                client,
                authorization,
            }),
        })
    }
    pub fn with_transport(transport: Arc<dyn SlackTransport>) -> Self {
        Self { transport }
    }

    pub async fn prepare(
        &self,
        ticket: RequestTicket,
        pull: PullRef,
        title: String,
        email: &str,
    ) -> Result<PreparedNudge, NudgeError> {
        let email = email.trim();
        if email.len() > 254
            || email.chars().any(char::is_whitespace)
            || !email.split_once('@').is_some_and(|(user, domain)| {
                !user.is_empty() && domain.contains('.') && !domain.contains('@')
            })
        {
            return Err(NudgeError::Email);
        }
        if ticket.resource != pull.key() && !ticket.resource.starts_with(&(pull.key() + ":")) {
            return Err(NudgeError::Stale);
        }
        let auth = checked_response(self.transport.call("auth.test", json!({})).await?, false)?;
        let team_id = identifier(&auth["team_id"], &['T'])?;
        let workspace = auth["team"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(NudgeError::Unavailable)?
            .to_string();
        let response = checked_response(
            self.transport
                .call("users.lookupByEmail", json!({"email":email}))
                .await?,
            false,
        )?;
        let user = &response["user"];
        let user_id = identifier(&user["id"], &['U', 'W'])?;
        if user["deleted"].as_bool() == Some(true)
            || user["is_bot"].as_bool() == Some(true)
            || user["team_id"].as_str().is_some_and(|team| team != team_id)
            || user["profile"]["email"]
                .as_str()
                .is_none_or(|resolved| !resolved.eq_ignore_ascii_case(email))
        {
            return Err(NudgeError::Recipient);
        }
        let recipient = user["real_name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .or_else(|| user["name"].as_str())
            .ok_or(NudgeError::Recipient)?
            .to_string();
        Ok(PreparedNudge {
            client: self.clone(),
            ticket,
            pull,
            title,
            workspace,
            recipient,
            email: email.into(),
            user_id,
            team_id,
        })
    }
}
fn identifier(value: &Value, prefixes: &[char]) -> Result<String, NudgeError> {
    let id = value.as_str().ok_or(NudgeError::Unavailable)?;
    if id.len() < 2
        || id.len() > 64
        || !id.starts_with(prefixes)
        || !id
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return Err(NudgeError::Unavailable);
    }
    Ok(id.into())
}

pub struct PreparedNudge {
    client: NudgeClient,
    ticket: RequestTicket,
    pull: PullRef,
    title: String,
    pub workspace: String,
    pub recipient: String,
    pub email: String,
    user_id: String,
    team_id: String,
}
impl PreparedNudge {
    pub fn pull(&self) -> &PullRef {
        &self.pull
    }
    pub fn account(&self) -> &super::AccountScope {
        &self.ticket.account
    }
    pub fn message(&self) -> String {
        format!(
            "Hey! Could you take another look at this pull request when you have a moment?\n{}\n{}",
            self.title,
            self.pull.web_url()
        )
    }
    pub fn confirm(self, selection: &SelectionState) -> Result<ConfirmedNudge, NudgeError> {
        if !selection.accepts(&self.ticket) {
            return Err(NudgeError::Stale);
        }
        Ok(ConfirmedNudge(self))
    }
}
pub struct ConfirmedNudge(PreparedNudge);
impl ConfirmedNudge {
    pub async fn send(self) -> Result<(), NudgeError> {
        let nudge = self.0;
        // The same immutable client/credential resolves and sends to the reviewed workspace.
        let auth = checked_response(
            nudge.client.transport.call("auth.test", json!({})).await?,
            false,
        )?;
        if auth["team_id"].as_str() != Some(&nudge.team_id) {
            return Err(NudgeError::Stale);
        }
        let dm = checked_response(
            nudge
                .client
                .transport
                .call("conversations.open", json!({"users":nudge.user_id}))
                .await?,
            false,
        )?;
        let channel = identifier(&dm["channel"]["id"], &['D'])?;
        let result=checked_response(nudge.client.transport.call("chat.postMessage",json!({"channel":channel,"text":nudge.message(),"mrkdwn":false,"unfurl_links":false,"unfurl_media":false})).await?,true)?;
        if result["channel"].as_str() != Some(&channel)
            || result["ts"].as_str().is_none_or(|ts| {
                ts.is_empty() || !ts.bytes().all(|c| c.is_ascii_digit() || c == b'.')
            })
        {
            return Err(NudgeError::UnknownSend);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Fake {
        calls: Mutex<Vec<(&'static str, Value)>>,
        responses: Mutex<std::collections::VecDeque<Value>>,
    }
    impl SlackTransport for Fake {
        fn call(&self, method: &'static str, body: Value) -> SlackFuture<'_> {
            Box::pin(async move {
                self.calls.lock().unwrap().push((method, body));
                self.responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or(NudgeError::Unavailable)
            })
        }
    }
    fn selection() -> SelectionState {
        let mut s = SelectionState::default();
        s.select(
            Some(super::super::AccountScope::new("Second", "team").unwrap()),
            pull().key(),
        );
        s
    }
    fn pull() -> PullRef {
        PullRef::new(
            super::super::RepositoryRef::parse("team/private").unwrap(),
            7,
        )
        .unwrap()
    }
    fn fixture() -> (NudgeClient, Arc<Fake>) {
        let auth = json!({"ok":true,"team":"Fixture workspace","team_id":"T123"});
        let fake=Arc::new(Fake {calls:Mutex::new(Vec::new()),responses:Mutex::new(vec![auth.clone(),json!({"ok":true,"user":{"id":"U123","team_id":"T123","name":"author","profile":{"email":"author@example.test"}}}),auth,json!({"ok":true,"channel":{"id":"D123"}}),json!({"ok":true,"channel":"D123","ts":"123.456"})].into())});
        (NudgeClient::with_transport(fake.clone()), fake)
    }
    #[tokio::test]
    async fn lookup_previews_workspace_and_recipient_without_opening_or_sending_a_dm() {
        let (client, fake) = fixture();
        let s = selection();
        let prepared = client
            .prepare(
                s.ticket().unwrap(),
                pull(),
                "Fixture PR".into(),
                "author@example.test",
            )
            .await
            .unwrap();
        assert_eq!(prepared.workspace, "Fixture workspace");
        assert_eq!(prepared.recipient, "author");
        assert!(
            prepared
                .message()
                .contains("https://github.com/team/private/pull/7")
        );
        assert_eq!(fake.calls.lock().unwrap().len(), 2);
        let mut changed = s;
        changed.invalidate();
        assert!(matches!(prepared.confirm(&changed), Err(NudgeError::Stale)));
        assert_eq!(fake.calls.lock().unwrap().len(), 2);
    }
    #[tokio::test]
    async fn a_consumed_confirmation_sends_exactly_one_fixture_message_to_the_reviewed_user() {
        let (client, fake) = fixture();
        let s = selection();
        client
            .prepare(
                s.ticket().unwrap(),
                pull(),
                "Fixture PR".into(),
                "author@example.test",
            )
            .await
            .unwrap()
            .confirm(&s)
            .unwrap()
            .send()
            .await
            .unwrap();
        let calls = fake.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|(method, _)| *method == "chat.postMessage")
                .count(),
            1
        );
        assert_eq!(calls[3].1["users"], "U123");
        assert_eq!(calls[4].1["channel"], "D123");
        assert_eq!(calls[4].1["mrkdwn"], false);
    }
    #[test]
    fn inactive_accounts_and_unknown_send_results_are_static_errors() {
        assert_eq!(
            checked_response(
                json!({"ok":false,"error":"account_inactive","secret":"must not escape"}),
                false
            ),
            Err(NudgeError::Authentication)
        );
        assert_eq!(
            checked_response(json!({"ok":true}), true),
            Ok(json!({"ok":true}))
        );
        assert_eq!(
            checked_response(json!({"ok":false,"error":"internal_error"}), true),
            Err(NudgeError::UnknownSend)
        );
    }
}
