use super::*;
use buddy_core::github::RepositoryRef;

fn pull() -> PullRef {
    PullRef::new(RepositoryRef::parse("fixture-org/native-demo").unwrap(), 8).unwrap()
}

/// Electron's `buildAddressCommentsPrompt` output for the same PR,
/// copied from assistantPrompts.ts:8.
#[test]
fn the_prompt_is_electrons_word_for_word() {
    assert_eq!(
        prompt(&pull()),
        "Address the unresolved review comments on PR #8 in fixture-org/native-demo (https://github.com/fixture-org/native-demo/pull/8).\n\
         \n\
         For each unresolved review thread:\n\
         1. Fetch the thread and understand what the reviewer is asking for.\n\
         2. Implement the requested change in the codebase.\n\
         3. Reply to the thread with a brief summary of what you changed (e.g., \"Fixed — replaced X with Y in `file.ts`.\").\n\
         4. Mark the thread as resolved.\n\
         \n\
         After addressing all threads, push the changes and confirm the final status."
    );
}

fn thread(id: &str, resolved: bool, bodies: &[&str]) -> (ReviewThread, Vec<Comment>) {
    let page = serde_json::json!({
        "data": {"repository": {"pullRequest": {"reviewThreads": {
            "pageInfo": {"hasNextPage": false, "endCursor": null},
            "nodes": [{
                "id": id, "isResolved": resolved, "isOutdated": false, "path": "src/lib.rs",
                "line": 4, "diffSide": "RIGHT",
                "comments": {"pageInfo": {"hasNextPage": false, "endCursor": null},
                    "nodes": bodies.iter().enumerate().map(|(index, body)| serde_json::json!({
                        "id": format!("{id}_{index}"), "body": body,
                        "createdAt": "2026-10-15T12:00:00Z", "updatedAt": "2026-10-15T12:00:00Z",
                        "url": null, "diffHunk": format!("@@ hunk of {id} @@"),
                        "author": {"login": "fixture-reviewer"}, "reactionGroups": []
                    })).collect::<Vec<_>>()}
            }]
        }}}}
    });
    let thread = futures::executor::block_on(parse(page));
    let comments = thread.comments.items.clone();
    (thread, comments)
}

/// One thread as `GitHubClient::pull_threads` parses it.
async fn parse(body: serde_json::Value) -> ReviewThread {
    struct One(serde_json::Value);
    impl buddy_core::github::Transport for One {
        fn request<'a>(
            &'a self,
            _: &'a buddy_core::github::AccountScope,
            _: buddy_core::github::ApiRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<buddy_core::github::ApiResponse, GitHubError>,
                    > + Send
                    + 'a,
            >,
        > {
            let body = self.0.clone();
            Box::pin(async move {
                Ok(buddy_core::github::ApiResponse {
                    status: 200,
                    headers: Default::default(),
                    body,
                })
            })
        }
    }
    let clients = buddy_core::github::GitHubClients::with_transport(std::sync::Arc::new(One(body)));
    let client =
        clients.for_account(buddy_core::github::AccountScope::new("First", "team").unwrap());
    client
        .pull_threads(&pull(), None, true)
        .await
        .unwrap()
        .items
        .remove(0)
}

#[test]
fn the_context_carries_only_unresolved_threads_with_every_comment() {
    let threads = [
        thread("PRRT_a", true, &["RESOLVEDTEXT"]),
        thread("PRRT_b", false, &["First ask", "Second reply"]),
    ];
    let unresolved = context(&threads).unwrap();
    assert_eq!(unresolved.count, 1);
    assert_eq!(unresolved.label(), "1 unresolved review thread");
    let text = &unresolved.context;
    assert!(text.starts_with(PREAMBLE));
    assert!(text.contains("this assistant has no tools"));
    assert!(!text.contains("RESOLVEDTEXT"));
    let records: Vec<serde_json::Value> = text
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["kind"], "thread");
    assert_eq!(records[0]["path"], "src/lib.rs");
    assert_eq!(records[0]["line"], 4);
    assert_eq!(records[1]["body"], "First ask");
    // The thread's code is the first comment's hunk; replies repeat it.
    assert_eq!(records[1]["diffHunk"], "@@ hunk of PRRT_b @@");
    assert_eq!(records[2]["body"], "Second reply");
    assert!(records[2]["diffHunk"].is_null());
    assert!(context(&threads[..1]).is_none(), "resolved threads only");
}

#[test]
fn threads_past_the_limit_are_counted_and_left_out_whole() {
    let long = "x".repeat(40 * 1024);
    let threads: Vec<_> = (0..4)
        .map(|index| thread(&format!("PRRT_{index}"), false, &[&long]))
        .collect();
    let unresolved = context(&threads).unwrap();
    assert_eq!(unresolved.count, 4);
    assert_eq!(unresolved.label(), "4 unresolved review threads");
    assert!(unresolved.context.len() <= CONTEXT_LIMIT + 200);
    let records: Vec<serde_json::Value> = unresolved
        .context
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).expect("no record is cut"))
        .collect();
    let last = records.last().unwrap();
    assert_eq!(last["kind"], "omitted");
    assert_eq!(last["threads"], 2);
    assert_eq!(records.iter().filter(|r| r["kind"] == "thread").count(), 2);
}
