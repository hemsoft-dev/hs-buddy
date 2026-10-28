//! Everything the Bookmarks section asks of the outside world, behind one interface
//! so the view can be tested without Convex, the network or a model.
use std::sync::Arc;
use std::time::Duration;

use buddy_core::bookmarks::client::{self, BackendError, Snapshot};
use buddy_core::bookmarks::{Draft, title};
use buddy_core::copilot_assistant::{self, RunOptions};
use buddy_core::copilot_runs::{Context as RunContext, Phase};
use futures::future::BoxFuture;

/// A suggestion that takes longer than this is abandoned and its run stopped.
const SUGGEST_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a stopped suggestion may take to clean up.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Who answers the AI suggestion: the Copilot account and model from Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SuggestRequest {
    pub account: String,
    pub model: String,
    pub prompt: String,
}

pub(super) type Reply<T> = BoxFuture<'static, Result<T, BackendError>>;

pub(super) trait Service: Send + Sync {
    fn load(&self) -> Reply<Snapshot>;
    fn create(&self, draft: Draft) -> Reply<String>;
    fn update(&self, id: String, draft: Draft) -> Reply<()>;
    fn remove(&self, id: String) -> Reply<()>;
    fn reorder(&self, updates: Vec<(String, f64)>) -> Reply<()>;
    fn record_visit(&self, id: String) -> Reply<()>;
    /// The page's title, or nothing; failures are quiet, as in Electron.
    fn page_title(&self, url: String) -> BoxFuture<'static, Option<String>>;
    /// The model's raw reply, or nothing when Copilot is unavailable or fails.
    fn suggest(&self, request: SuggestRequest) -> BoxFuture<'static, Option<String>>;
}

/// Convex at the app's deployment URL, the guarded title fetch, and Chat's tool-less
/// assistant.
pub(super) struct Live {
    url: String,
}

impl Live {
    pub fn service() -> Arc<dyn Service> {
        Arc::new(Self {
            url: buddy_core::convex_data::convex_url(),
        })
    }
}

impl Service for Live {
    fn load(&self) -> Reply<Snapshot> {
        let url = self.url.clone();
        Box::pin(async move { client::load(&url).await })
    }
    fn create(&self, draft: Draft) -> Reply<String> {
        let url = self.url.clone();
        Box::pin(async move { client::create(&url, &draft).await })
    }
    fn update(&self, id: String, draft: Draft) -> Reply<()> {
        let url = self.url.clone();
        Box::pin(async move { client::update(&url, &id, &draft).await })
    }
    fn remove(&self, id: String) -> Reply<()> {
        let url = self.url.clone();
        Box::pin(async move { client::remove(&url, &id).await })
    }
    fn reorder(&self, updates: Vec<(String, f64)>) -> Reply<()> {
        let url = self.url.clone();
        Box::pin(async move { client::reorder(&url, &updates).await })
    }
    fn record_visit(&self, id: String) -> Reply<()> {
        let url = self.url.clone();
        Box::pin(async move { client::record_visit(&url, &id).await })
    }
    fn page_title(&self, url: String) -> BoxFuture<'static, Option<String>> {
        Box::pin(async move { title::fetch_page_title(&url).await.ok() })
    }
    fn suggest(&self, request: SuggestRequest) -> BoxFuture<'static, Option<String>> {
        Box::pin(suggest(request))
    }
}

async fn suggest(request: SuggestRequest) -> Option<String> {
    let owned_directory = buddy_core::config::config_path()
        .ok()?
        .parent()?
        .join("native-assistant");
    let executable = std::env::current_exe().ok()?;
    let (run, future) = copilot_assistant::start(RunOptions {
        context: RunContext {
            account: request.account,
            model: request.model,
            review: None,
        },
        prompt: request.prompt,
        saved: false,
        review: None,
        backend: None,
        executable,
        owned_directory,
    })
    .ok()?;
    let mut future = Box::pin(future);
    if tokio::time::timeout(SUGGEST_TIMEOUT, &mut future)
        .await
        .is_err()
    {
        run.cancel();
        let _ = tokio::time::timeout(STOP_TIMEOUT, future).await;
        return None;
    }
    let state = run.snapshot();
    (state.phase == Phase::Completed).then_some(state.output)
}
