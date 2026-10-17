//! Offline qualification is confined to disposable CI roots and synthetic identities.
//! Every provider request is intercepted here; there is no upstream forwarder.
use crate::copilot_runs::Failure;
use async_trait::async_trait;
use bytes::Bytes;
use github_copilot_sdk::copilot_request_handler::{
    CopilotHttpRequest, CopilotHttpResponse, CopilotRequestContext, CopilotRequestError,
    CopilotRequestHandler, CopilotWebSocketHandler, CopilotWebSocketResponse,
};
use github_copilot_sdk::types::{Model, ProviderConfig};
use serde_json::json;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

mod stream_capture;
use stream_capture::StreamCapture;

const MARKER: &str = "hs-buddy native Copilot qualification v1";
const ACCOUNT: &str = "synthetic-account";
const MODEL: &str = "gpt-4.1";
pub(crate) struct Fixture {
    deployment: String,
    report: PathBuf,
    requests: AtomicUsize,
    cancelled: Arc<AtomicUsize>,
    rejected: AtomicUsize,
    stream_capture: Option<StreamCapture>,
}
fn contained(path: &Path, root: &Path) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    if let Ok(path) = path.canonicalize() {
        return path.starts_with(root);
    }
    path.parent()
        .is_some_and(|parent| parent != path && contained(parent, root))
}
pub(crate) fn load(owned: &Path) -> Result<Option<Arc<Fixture>>, Failure> {
    if std::env::var_os("BUDDY_COPILOT_FIXTURE").is_none() {
        return Ok(None);
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("BUDDY_COPILOT_FIXTURE").as_deref() != Ok("1")
    {
        return Err(Failure::InvalidContext);
    }
    let runner = std::env::var_os("RUNNER_TEMP")
        .map(PathBuf::from)
        .ok_or(Failure::InvalidContext)?;
    let config = std::env::var_os(crate::config::CONFIG_PATH_ENV)
        .map(PathBuf::from)
        .ok_or(Failure::InvalidContext)?;
    let endpoint = std::env::var("BUDDY_CONVEX_URL").map_err(|_| Failure::InvalidContext)?;
    let mut fixture = validate(owned, &runner, &config, &endpoint)?;
    if let Some(path) = std::env::var_os("BUDDY_COPILOT_STREAM_CAPTURE") {
        let directory = config.canonicalize().map_err(|_| Failure::InvalidContext)?;
        let capture = StreamCapture::new(
            PathBuf::from(path),
            directory.parent().ok_or(Failure::InvalidContext)?,
        )?;
        Arc::get_mut(&mut fixture)
            .ok_or(Failure::InvalidContext)?
            .stream_capture = Some(capture);
    }
    Ok(Some(fixture))
}
fn validate(
    owned: &Path,
    runner: &Path,
    config: &Path,
    endpoint: &str,
) -> Result<Arc<Fixture>, Failure> {
    let root = runner.canonicalize().map_err(|_| Failure::InvalidContext)?;
    let config = config
        .canonicalize()
        .ok()
        .filter(|path| path.starts_with(&root) && path.is_file())
        .ok_or(Failure::InvalidContext)?;
    let directory = config.parent().ok_or(Failure::InvalidContext)?;
    if !contained(owned, directory) {
        return Err(Failure::InvalidContext);
    }
    let marker = directory
        .join("copilot-fixture.allow")
        .canonicalize()
        .map_err(|_| Failure::InvalidContext)?;
    if !marker.starts_with(directory)
        || !marker
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 128)
    {
        return Err(Failure::InvalidContext);
    }
    let mut text = String::new();
    std::fs::File::open(marker)
        .map_err(|_| Failure::InvalidContext)?
        .take(129)
        .read_to_string(&mut text)
        .map_err(|_| Failure::InvalidContext)?;
    if text.len() > 128 {
        return Err(Failure::InvalidContext);
    }
    let mut lines = text.lines();
    if lines.next() != Some(MARKER)
        || lines
            .next()
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .is_none_or(|id| id.get_version_num() != 4)
        || lines.next().is_some()
    {
        return Err(Failure::InvalidContext);
    }
    let url = reqwest::Url::parse(endpoint).map_err(|_| Failure::InvalidContext)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none_or(|port| port == 0)
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Failure::InvalidContext);
    }
    Ok(Arc::new(Fixture {
        deployment: url.as_str().trim_end_matches('/').into(),
        report: directory.join(format!("copilot-provider-{}.json", uuid::Uuid::new_v4())),
        requests: AtomicUsize::new(0),
        cancelled: Arc::new(AtomicUsize::new(0)),
        rejected: AtomicUsize::new(0),
        stream_capture: None,
    }))
}
impl Fixture {
    pub(crate) fn matches_backend(&self, url: &str) -> bool {
        url.trim_end_matches('/') == self.deployment
    }
    pub(crate) fn identity(&self, account: &str, model: Option<&str>) -> Result<(), Failure> {
        if account != ACCOUNT || model.is_some_and(|model| model != MODEL) {
            return Err(Failure::InvalidContext);
        }
        Ok(())
    }
    pub(crate) fn provider(&self) -> ProviderConfig {
        ProviderConfig::new("https://hs-buddy-fixture.invalid/v1")
            .with_provider_type("openai")
            .with_wire_api("completions")
            .with_api_key("synthetic-nonsecret")
    }
    fn observe(&self) {
        let value = json!({"offline":true,"upstreamForwarder":false,"requests":self.requests.load(Ordering::SeqCst),"cancelled":self.cancelled.load(Ordering::SeqCst),"rejected":self.rejected.load(Ordering::SeqCst)});
        if let Ok(bytes) = serde_json::to_vec(&value) {
            let _ = std::fs::write(&self.report, bytes);
        }
    }
}
#[async_trait]
impl github_copilot_sdk::ListModelsHandler for Fixture {
    async fn list_models(&self) -> github_copilot_sdk::Result<Vec<Model>> {
        Ok(vec![serde_json::from_value(json!({"id":MODEL,"name":"Synthetic offline model","capabilities":{"supports":{},"limits":{}}})).expect("fixed fixture model")])
    }
}
#[async_trait]
impl CopilotRequestHandler for Fixture {
    async fn send_request(
        &self,
        request: CopilotHttpRequest,
        ctx: &CopilotRequestContext,
    ) -> Result<CopilotHttpResponse, CopilotRequestError> {
        let body = serde_json::from_slice::<serde_json::Value>(&request.body).ok();
        let cancelling = body
            .as_ref()
            .is_some_and(|body| body.to_string().contains("BUDDYCANCEL73429"));
        let streaming = body
            .as_ref()
            .is_some_and(|body| body.to_string().contains("BUDDYSTREAM61527"));
        if request.method != "POST"
            || request.url != "https://hs-buddy-fixture.invalid/v1/chat/completions"
            || (!cancelling && !streaming)
            || (streaming
                && self
                    .stream_capture
                    .as_ref()
                    .is_some_and(|capture| !capture.pending()))
            || body.as_ref().is_none_or(|body| {
                body["stream"] != true
                    || body
                        .get("tools")
                        .is_some_and(|tools| !tools.as_array().is_some_and(Vec::is_empty))
            })
        {
            self.rejected.fetch_add(1, Ordering::SeqCst);
            self.observe();
            return Err(CopilotRequestError::message(
                "Offline qualification rejects this provider request.",
            ));
        }
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.observe();
        let pieces = if cancelling {
            vec!["Partial synthetic answer 界"]
        } else {
            vec![
                "## Synthetic assistant\n\n",
                "Unicode: 界 ✓\n\n",
                "| Check | Result |\n| --- | --- |\n| Offline stream | Passed |\n\n`native Rust` output.",
            ]
        };
        let mut chunks:Vec<Bytes>=pieces.into_iter().map(|content|Bytes::from(format!("data: {}\n\n",json!({"id":"buddy-offline","object":"chat.completion.chunk","created":0,"model":MODEL,"choices":[{"index":0,"delta":{"content":content},"finish_reason":null}]})))).collect();
        if !cancelling {
            chunks.push(Bytes::from(format!("data: {}\n\n",json!({"id":"buddy-offline","object":"chat.completion.chunk","created":0,"model":MODEL,"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13}}))));
            chunks.push(Bytes::from_static(b"data: [DONE]\n\n"));
        }
        let cancel = ctx.cancel.clone();
        let cancelled = self.cancelled.clone();
        let report = self.report.clone();
        let requests = self.requests.load(Ordering::SeqCst);
        let rejected = self.rejected.load(Ordering::SeqCst);
        if cancelling {
            let observed = cancel.clone();
            let cancelled = cancelled.clone();
            let report = report.clone();
            tokio::spawn(async move {
                observed.cancelled().await;
                cancelled.fetch_add(1, Ordering::SeqCst);
                let _=std::fs::write(report,serde_json::to_vec(&json!({"offline":true,"upstreamForwarder":false,"requests":requests,"cancelled":cancelled.load(Ordering::SeqCst),"rejected":rejected})).unwrap_or_default());
            });
        }
        let capture = if cancelling {
            None
        } else {
            self.stream_capture.clone()
        };
        let stream = futures::stream::unfold(
            (chunks.into_iter(), cancel, capture, false),
            move |(mut chunks, cancel, mut capture, emitted)| async move {
                // Keep the first real delta visible until the CI driver captures
                // it. OCR speed must not decide whether streaming is exercised.
                if emitted
                    && let Some(capture) = capture.take()
                    && !capture
                        .wait(cancel.cancelled(), Duration::from_secs(60))
                        .await
                {
                    return Some((
                        Err(CopilotRequestError::message(
                            "Offline stream capture was not acknowledged.",
                        )),
                        (Vec::new().into_iter(), cancel, None, true),
                    ));
                }
                if let Some(chunk) = chunks.next() {
                    tokio::time::sleep(Duration::from_millis(if cancelling { 350 } else { 800 }))
                        .await;
                    Some((Ok(chunk), (chunks, cancel, capture, true)))
                } else if cancelling {
                    cancel.cancelled().await;
                    None
                } else {
                    None
                }
            },
        );
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "content-type",
            http::HeaderValue::from_static("text/event-stream"),
        );
        Ok(CopilotHttpResponse::new(
            200,
            Some("OK".into()),
            headers,
            Box::pin(stream),
        ))
    }
    async fn open_websocket(
        &self,
        _: &CopilotRequestContext,
        _: CopilotWebSocketResponse,
    ) -> Result<Box<dyn CopilotWebSocketHandler>, CopilotRequestError> {
        self.rejected.fetch_add(1, Ordering::SeqCst);
        self.observe();
        Err(CopilotRequestError::message(
            "Offline qualification never opens upstream WebSockets.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonexistent_children_remain_confined_but_parent_traversal_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        assert!(contained(&root.join("new").join("state"), &root));
        assert!(!contained(&root.join("..").join("outside"), &root));
        assert!(!contained(Path::new("relative"), &root));
    }
    #[test]
    fn valid_fixture_never_accepts_other_identities_backends_or_roots() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("owned");
        std::fs::create_dir(&directory).unwrap();
        let config = directory.join("config.json");
        std::fs::write(&config, "{}").unwrap();
        let marker = directory.join("copilot-fixture.allow");
        std::fs::write(&marker, format!("{MARKER}\n{}", uuid::Uuid::new_v4())).unwrap();
        let owned = directory.join("native-assistant");
        let fixture = validate(&owned, temp.path(), &config, "http://127.0.0.1:3210").unwrap();
        assert!(fixture.identity(ACCOUNT, Some(MODEL)).is_ok());
        assert!(fixture.identity("another-account", Some(MODEL)).is_err());
        assert!(fixture.identity(ACCOUNT, Some("another-model")).is_err());
        assert!(fixture.matches_backend("http://127.0.0.1:3210"));
        assert!(!fixture.matches_backend("https://deployment.convex.cloud"));
        assert!(
            validate(
                &temp.path().join("outside"),
                temp.path(),
                &config,
                "http://127.0.0.1:3210"
            )
            .is_err()
        );
        for endpoint in [
            "https://deployment.convex.cloud",
            "http://localhost:3210",
            "http://127.0.0.1:3210/private",
            "http://user@127.0.0.1:3210",
        ] {
            assert!(validate(&owned, temp.path(), &config, endpoint).is_err());
        }
        std::fs::write(&marker, "invalid marker").unwrap();
        assert!(validate(&owned, temp.path(), &config, "http://127.0.0.1:3210").is_err());
        assert!(!owned.exists());
    }
}
