use serde_json::{Value, json};

use super::details::{valid_node_id, valid_sha};
use super::{
    AccountScope, ApiRequest, CommentRef, GitHubClient, GitHubError, PullRef, RequestTicket,
    SelectionState, ThreadRef,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    ThumbsUp,
    ThumbsDown,
    Laugh,
    Hooray,
    Confused,
    Heart,
    Rocket,
    Eyes,
}

impl Reaction {
    pub const ALL: [Self; 8] = [
        Self::ThumbsUp,
        Self::ThumbsDown,
        Self::Laugh,
        Self::Hooray,
        Self::Confused,
        Self::Heart,
        Self::Rocket,
        Self::Eyes,
    ];
    pub fn graphql(self) -> &'static str {
        match self {
            Self::ThumbsUp => "THUMBS_UP",
            Self::ThumbsDown => "THUMBS_DOWN",
            Self::Laugh => "LAUGH",
            Self::Hooray => "HOORAY",
            Self::Confused => "CONFUSED",
            Self::Heart => "HEART",
            Self::Rocket => "ROCKET",
            Self::Eyes => "EYES",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::ThumbsUp => "Thumbs up",
            Self::ThumbsDown => "Thumbs down",
            Self::Laugh => "Laugh",
            Self::Hooray => "Hooray",
            Self::Confused => "Confused",
            Self::Heart => "Heart",
            Self::Rocket => "Rocket",
            Self::Eyes => "Eyes",
        }
    }
}

#[derive(Debug, Clone)]
pub enum WriteAction {
    Comment {
        pull: PullRef,
        body: String,
    },
    Approve {
        pull: PullRef,
        head_sha: String,
    },
    RequestCopilot {
        pull: PullRef,
    },
    RequestCodeRabbit {
        pull: PullRef,
    },
    Reply {
        thread: ThreadRef,
        body: String,
    },
    Resolve {
        thread: ThreadRef,
        resolved: bool,
    },
    React {
        comment: CommentRef,
        reaction: Reaction,
    },
}

impl WriteAction {
    pub fn pull(&self) -> &PullRef {
        match self {
            Self::Comment { pull, .. }
            | Self::Approve { pull, .. }
            | Self::RequestCopilot { pull }
            | Self::RequestCodeRabbit { pull } => pull,
            Self::Reply { thread, .. } | Self::Resolve { thread, .. } => thread.pull(),
            Self::React { comment, .. } => comment.pull(),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Comment { .. } => "Post comment",
            Self::Approve { .. } => "Approve commit",
            Self::RequestCopilot { .. } => "Request Copilot review",
            Self::RequestCodeRabbit { .. } => "Request CodeRabbit review",
            Self::Reply { .. } => "Reply to thread",
            Self::Resolve { resolved: true, .. } => "Resolve thread",
            Self::Resolve {
                resolved: false, ..
            } => "Reopen thread",
            Self::React { .. } => "Add reaction",
        }
    }

    pub fn body(&self) -> Option<&str> {
        match self {
            Self::Comment { body, .. } | Self::Reply { body, .. } => Some(body),
            Self::RequestCodeRabbit { .. } => Some("@coderabbitai full review"),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), GitHubError> {
        PullRef::new(self.pull().repository.clone(), self.pull().number)?;
        if let Some(body) = self.body() {
            valid_body(body)?;
        }
        match self {
            Self::Approve { head_sha, .. } => valid_sha(head_sha),
            Self::Reply { thread, .. } | Self::Resolve { thread, .. } => valid_node_id(thread.id()),
            Self::React { comment, .. } => valid_node_id(comment.id()),
            _ => Ok(()),
        }
    }
}

fn valid_body(body: &str) -> Result<(), GitHubError> {
    if body.trim().is_empty() {
        Err(GitHubError::EmptyComment)
    } else if body.chars().count() > 65_536 {
        Err(GitHubError::CommentTooLong)
    } else {
        Ok(())
    }
}

/// The pending action cannot be retargeted after the user has reviewed it.
pub struct PendingWrite {
    ticket: RequestTicket,
    action: WriteAction,
}

impl PendingWrite {
    pub fn new(selection: &SelectionState, action: WriteAction) -> Result<Self, GitHubError> {
        action.validate()?;
        let ticket = selection.ticket().ok_or(GitHubError::InvalidAccount)?;
        let key = action.pull().key();
        if ticket.resource != key && !ticket.resource.starts_with(&(key + ":")) {
            return Err(GitHubError::StaleConfirmation);
        }
        Ok(Self { ticket, action })
    }

    pub fn account(&self) -> &AccountScope {
        &self.ticket.account
    }
    pub fn action(&self) -> &WriteAction {
        &self.action
    }

    pub fn confirm(self, selection: &SelectionState) -> Result<ConfirmedWrite, GitHubError> {
        if !selection.accepts(&self.ticket) {
            return Err(GitHubError::StaleConfirmation);
        }
        Ok(ConfirmedWrite {
            account: self.ticket.account,
            action: self.action,
        })
    }
}

/// Consumed by the client. A completed or uncertain request is never replayed.
pub struct ConfirmedWrite {
    account: AccountScope,
    action: WriteAction,
}

#[derive(Debug)]
pub struct WriteReceipt {
    pub label: &'static str,
    pub pull: PullRef,
    pub id: String,
}

impl GitHubClient {
    pub async fn execute(&self, confirmed: ConfirmedWrite) -> Result<WriteReceipt, GitHubError> {
        if confirmed.account != self.account {
            return Err(GitHubError::StaleConfirmation);
        }
        let action = confirmed.action;
        action.validate()?;
        let pull = action.pull().clone();
        let label = match &action {
            WriteAction::Comment { .. } => "Comment posted",
            WriteAction::Approve { .. } => "Commit approved",
            WriteAction::RequestCopilot { .. } => "Copilot review requested",
            WriteAction::RequestCodeRabbit { .. } => "CodeRabbit review requested",
            WriteAction::Reply { .. } => "Reply posted",
            WriteAction::Resolve { resolved: true, .. } => "Thread resolved",
            WriteAction::Resolve {
                resolved: false, ..
            } => "Thread reopened",
            WriteAction::React { .. } => "Reaction added",
        };
        let path = pull.repository.api_path();
        let (request, ack) = match &action {
            WriteAction::Comment { body, .. } => (
                ApiRequest::post(
                    format!("{path}/issues/{}/comments", pull.number),
                    json!({"body":body}),
                ),
                Ack::Comment,
            ),
            WriteAction::RequestCodeRabbit { .. } => (
                ApiRequest::post(
                    format!("{path}/issues/{}/comments", pull.number),
                    json!({"body":"@coderabbitai full review"}),
                ),
                Ack::Comment,
            ),
            WriteAction::Approve { head_sha, .. } => {
                let current = self.pull(&pull, true).await?;
                if current.head_sha != *head_sha {
                    return Err(GitHubError::StaleConfirmation);
                }
                (
                    ApiRequest::post(
                        format!("{path}/pulls/{}/reviews", pull.number),
                        json!({"event":"APPROVE","body":"Approved","commit_id":head_sha}),
                    ),
                    Ack::Approval(head_sha.clone()),
                )
            }
            WriteAction::RequestCopilot { .. } => (
                ApiRequest::post(
                    format!("{path}/pulls/{}/requested_reviewers", pull.number),
                    json!({"reviewers":["copilot-pull-request-reviewer[bot]"]}),
                ),
                Ack::Reviewer,
            ),
            WriteAction::Reply { thread, body } => {
                let query = "mutation NativeReply($id:ID!,$body:String!) {addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$id,body:$body}) {comment {id}}}";
                (
                    ApiRequest::graphql(query, json!({"id":thread.id(),"body":body}), true),
                    Ack::Reply,
                )
            }
            WriteAction::Resolve { thread, resolved } => {
                let operation = if *resolved {
                    "resolveReviewThread"
                } else {
                    "unresolveReviewThread"
                };
                let query = format!(
                    "mutation NativeResolve($id:ID!) {{{operation}(input:{{threadId:$id}}) {{thread {{id isResolved}}}}}}"
                );
                (
                    ApiRequest::graphql(&query, json!({"id":thread.id()}), true),
                    Ack::Thread {
                        operation,
                        id: thread.id().into(),
                        resolved: *resolved,
                    },
                )
            }
            WriteAction::React { comment, reaction } => {
                let query = "mutation NativeReact($id:ID!,$content:ReactionContent!) {addReaction(input:{subjectId:$id,content:$content}) {reaction {content}}}";
                (
                    ApiRequest::graphql(
                        query,
                        json!({"id":comment.id(),"content":reaction.graphql()}),
                        true,
                    ),
                    Ack::Reaction(*reaction),
                )
            }
        };
        let response = self.request(request, true).await?.value.body;
        let id = ack.verify(&response)?;
        // The acknowledged receipt is independent of a later reload's outcome.
        Ok(WriteReceipt { label, pull, id })
    }
}

enum Ack {
    Comment,
    Approval(String),
    Reviewer,
    Reply,
    Thread {
        operation: &'static str,
        id: String,
        resolved: bool,
    },
    Reaction(Reaction),
}

impl Ack {
    fn verify(self, value: &Value) -> Result<String, GitHubError> {
        let id = match self {
            Self::Comment => value["id"]
                .as_u64()
                .filter(|id| *id > 0)
                .map(|id| id.to_string()),
            Self::Approval(expected)
                if value["state"] == "APPROVED" && value["commit_id"] == expected =>
            {
                value["id"]
                    .as_u64()
                    .filter(|id| *id > 0)
                    .map(|id| id.to_string())
            }
            Self::Reviewer => value["id"]
                .as_u64()
                .filter(|id| *id > 0)
                .map(|id| id.to_string()),
            Self::Reply => value["data"]["addPullRequestReviewThreadReply"]["comment"]["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(Into::into),
            Self::Thread {
                operation,
                id,
                resolved,
            } => {
                let thread = &value["data"][operation]["thread"];
                (thread["id"] == id && thread["isResolved"] == resolved).then_some(id)
            }
            Self::Reaction(reaction) => (value["data"]["addReaction"]["reaction"]["content"]
                == reaction.graphql())
            .then(|| reaction.graphql().into()),
            _ => None,
        };
        id.ok_or(GitHubError::UnknownWriteResult)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ApiResponse, GitHubClients, RepositoryRef, Transport};
    use super::*;
    use std::sync::{Arc, Mutex};

    struct WriteFixture {
        calls: Mutex<Vec<(String, ApiRequest)>>,
        body: Value,
        head: &'static str,
    }

    impl Transport for WriteFixture {
        fn request<'a>(
            &'a self,
            account: &'a AccountScope,
            request: ApiRequest,
        ) -> super::super::transport::ApiFuture<'a> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push((account.key(), request.clone()));
                let body = if !request.is_write {
                    json!({"data":{"repository":{"pullRequest":{"number":7,"repository":{"nameWithOwner":"team/repo"},"title":"A fixture PR","body":"","state":"OPEN","createdAt":"2026-10-01T00:00:00Z","updatedAt":"2026-10-15T00:00:00Z","mergedAt":null,"headRefName":"feature/fixture","baseRefName":"main","headRefOid":self.head,"isDraft":false,"changedFiles":1,"reviewDecision":null,"latestOpinion":{"nodes":[]},"reviewThreads":{"totalCount":1}}}}})
                } else {
                    self.body.clone()
                };
                Ok(ApiResponse {
                    status: 200,
                    headers: Default::default(),
                    body,
                })
            })
        }
    }

    fn target() -> PullRef {
        PullRef::new(RepositoryRef::parse("team/repo").unwrap(), 7).unwrap()
    }
    fn selection(user: &str) -> SelectionState {
        let mut selection = SelectionState::default();
        selection.select(
            Some(AccountScope::new(user, "team").unwrap()),
            target().key(),
        );
        selection
    }
    fn fixture(body: Value, head: &'static str) -> Arc<WriteFixture> {
        Arc::new(WriteFixture {
            calls: Mutex::new(Vec::new()),
            body,
            head,
        })
    }

    #[tokio::test]
    async fn a_confirmed_comment_posts_once_with_the_reviewed_account_repository_and_body() {
        let transport = fixture(
            json!({"id":333}),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let client = GitHubClients::with_transport(transport.clone())
            .for_account(AccountScope::new("Second", "team").unwrap());
        let selection = selection("Second");
        let pending = PendingWrite::new(
            &selection,
            WriteAction::Comment {
                pull: target(),
                body: "A confirmed fixture comment".into(),
            },
        )
        .unwrap();
        let receipt = client
            .execute(pending.confirm(&selection).unwrap())
            .await
            .unwrap();
        assert_eq!(receipt.id, "333");
        let calls = transport.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "the write acknowledgement must not depend on another request"
        );
        assert_eq!(calls[0].0, "team/second");
        assert_eq!(calls[0].1.path, "/repos/team/repo/issues/7/comments");
        assert_eq!(
            calls[0].1.body,
            Some(json!({"body":"A confirmed fixture comment"}))
        );
    }

    #[tokio::test]
    async fn switching_account_or_resource_invalidates_confirmation_before_any_write() {
        let transport = fixture(
            json!({"id":333}),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let client = GitHubClients::with_transport(transport.clone())
            .for_account(AccountScope::new("second", "team").unwrap());
        let mut selection = selection("first");
        let action = WriteAction::RequestCopilot { pull: target() };
        let pending = PendingWrite::new(&selection, action.clone()).unwrap();
        let permit = PendingWrite::new(&selection, action.clone())
            .unwrap()
            .confirm(&selection)
            .unwrap();
        assert!(matches!(
            client.execute(permit).await,
            Err(GitHubError::StaleConfirmation)
        ));
        selection.select(
            Some(AccountScope::new("second", "team").unwrap()),
            target().key(),
        );
        assert!(matches!(
            pending.confirm(&selection),
            Err(GitHubError::StaleConfirmation)
        ));
        selection.select(
            Some(AccountScope::new("second", "team").unwrap()),
            "pull:team/repo#70".into(),
        );
        assert!(matches!(
            PendingWrite::new(&selection, action),
            Err(GitHubError::StaleConfirmation)
        ));
        assert!(transport.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn incomplete_success_payloads_do_not_claim_a_write_succeeded_or_replay_it() {
        let transport = fixture(json!({}), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let client = GitHubClients::with_transport(transport.clone())
            .for_account(AccountScope::new("first", "team").unwrap());
        let selection = selection("first");
        let permit = PendingWrite::new(
            &selection,
            WriteAction::Comment {
                pull: target(),
                body: "One comment".into(),
            },
        )
        .unwrap()
        .confirm(&selection)
        .unwrap();
        assert!(matches!(
            client.execute(permit).await,
            Err(GitHubError::UnknownWriteResult)
        ));
        assert_eq!(transport.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn approval_is_rejected_if_the_head_changed_after_confirmation() {
        let transport = fixture(
            json!({"id":333}),
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        let client = GitHubClients::with_transport(transport.clone())
            .for_account(AccountScope::new("first", "team").unwrap());
        let selection = selection("first");
        let permit = PendingWrite::new(
            &selection,
            WriteAction::Approve {
                pull: target(),
                head_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            },
        )
        .unwrap()
        .confirm(&selection)
        .unwrap();
        assert!(matches!(
            client.execute(permit).await,
            Err(GitHubError::StaleConfirmation)
        ));
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(
            !calls[0].1.is_write,
            "checking the new head cannot approve it"
        );
    }

    #[tokio::test]
    async fn approval_payload_and_acknowledgement_pin_the_reviewed_commit() {
        let head = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let transport = fixture(json!({"id":444,"state":"APPROVED","commit_id":head}), head);
        let client = GitHubClients::with_transport(transport.clone())
            .for_account(AccountScope::new("first", "team").unwrap());
        let selection = selection("first");
        let permit = PendingWrite::new(
            &selection,
            WriteAction::Approve {
                pull: target(),
                head_sha: head.into(),
            },
        )
        .unwrap()
        .confirm(&selection)
        .unwrap();
        assert_eq!(client.execute(permit).await.unwrap().id, "444");
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].1.path, "/repos/team/repo/pulls/7/reviews");
        assert_eq!(
            calls[1].1.body,
            Some(json!({"event":"APPROVE","body":"Approved","commit_id":head}))
        );
    }

    #[tokio::test]
    async fn thread_and_reaction_writes_send_bound_ids_and_validate_the_actual_acknowledgement() {
        let thread = ThreadRef {
            pull: target(),
            id: "PRRT_bound".into(),
        };
        let comment = CommentRef {
            pull: target(),
            id: "IC_bound".into(),
        };
        let cases = [
            (
                WriteAction::Reply {
                    thread: thread.clone(),
                    body: "Confirmed reply".into(),
                },
                json!({"data":{"addPullRequestReviewThreadReply":{"comment":{"id":"PRRC_new"}}}}),
                json!({"id":"PRRT_bound","body":"Confirmed reply"}),
            ),
            (
                WriteAction::Resolve {
                    thread: thread.clone(),
                    resolved: true,
                },
                json!({"data":{"resolveReviewThread":{"thread":{"id":"PRRT_bound","isResolved":true}}}}),
                json!({"id":"PRRT_bound"}),
            ),
            (
                WriteAction::Resolve {
                    thread,
                    resolved: false,
                },
                json!({"data":{"unresolveReviewThread":{"thread":{"id":"PRRT_bound","isResolved":false}}}}),
                json!({"id":"PRRT_bound"}),
            ),
            (
                WriteAction::React {
                    comment,
                    reaction: Reaction::Heart,
                },
                json!({"data":{"addReaction":{"reaction":{"content":"HEART"}}}}),
                json!({"id":"IC_bound","content":"HEART"}),
            ),
        ];
        for (action, ack, variables) in cases {
            let transport = fixture(ack, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
            let selection = selection("Second");
            let client = GitHubClients::with_transport(transport.clone())
                .for_account(AccountScope::new("Second", "team").unwrap());
            client
                .execute(
                    PendingWrite::new(&selection, action)
                        .unwrap()
                        .confirm(&selection)
                        .unwrap(),
                )
                .await
                .unwrap();
            let calls = transport.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "team/second");
            assert_eq!(calls[0].1.path, "/graphql");
            assert_eq!(calls[0].1.body.as_ref().unwrap()["variables"], variables);
        }
        let transport = fixture(
            json!({"data":{"resolveReviewThread":{"thread":{"id":"PRRT_another","isResolved":true}}}}),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let client = GitHubClients::with_transport(transport.clone())
            .for_account(AccountScope::new("Second", "team").unwrap());
        let selection = selection("Second");
        let action = WriteAction::Resolve {
            thread: ThreadRef {
                pull: target(),
                id: "PRRT_bound".into(),
            },
            resolved: true,
        };
        assert!(matches!(
            client
                .execute(
                    PendingWrite::new(&selection, action)
                        .unwrap()
                        .confirm(&selection)
                        .unwrap()
                )
                .await,
            Err(GitHubError::UnknownWriteResult)
        ));
        assert_eq!(transport.calls.lock().unwrap().len(), 1);
    }
}
