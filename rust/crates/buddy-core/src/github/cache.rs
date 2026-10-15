use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{AccountScope, ApiRequest, ApiResponse, GitHubError, HttpTransport, Transport};

const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 64;
const CACHE_LIFETIME: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    account: AccountScope,
    request: String,
}

struct Entry {
    inserted: Instant,
    bytes: usize,
    response: ApiResponse,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<CacheKey, Entry>,
    pending: HashMap<CacheKey, u64>,
    sequence: u64,
}

impl Cache {
    fn begin(&mut self, key: CacheKey) -> u64 {
        self.sequence = self.sequence.wrapping_add(1);
        self.entries.remove(&key);
        self.pending.insert(key, self.sequence);
        // Dropped futures cannot leave an unbounded pending-key table.
        while self.pending.len() > MAX_CACHE_ENTRIES {
            let oldest = self
                .pending
                .iter()
                .min_by_key(|(_, sequence)| **sequence)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.pending.remove(&oldest);
            }
        }
        self.sequence
    }

    fn complete(&mut self, key: CacheKey, sequence: u64, response: Option<ApiResponse>) {
        if self.pending.get(&key) != Some(&sequence) {
            return;
        }
        self.pending.remove(&key);
        if let Some(response) = response {
            self.insert(key, response);
        }
    }

    fn insert(&mut self, key: CacheKey, response: ApiResponse) {
        let bytes = response.body.to_string().len();
        if bytes > MAX_CACHE_BYTES {
            return;
        }
        self.entries.insert(
            key,
            Entry {
                inserted: Instant::now(),
                bytes,
                response,
            },
        );
        while self.entries.len() > MAX_CACHE_ENTRIES
            || self
                .entries
                .values()
                .map(|entry| entry.bytes)
                .sum::<usize>()
                > MAX_CACHE_BYTES
        {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

/// All views can share this cache without sharing an account's identity.
#[derive(Clone)]
pub struct GitHubClients {
    transport: Arc<dyn Transport>,
    cache: Arc<Mutex<Cache>>,
}

impl Default for GitHubClients {
    fn default() -> Self {
        Self::with_transport(Arc::new(HttpTransport::default()))
    }
}

impl GitHubClients {
    pub fn with_transport(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            cache: Arc::new(Mutex::new(Cache::default())),
        }
    }

    pub fn for_account(&self, account: AccountScope) -> GitHubClient {
        GitHubClient {
            account,
            shared: self.clone(),
        }
    }

    pub fn clear(&self) {
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        cache.entries.clear();
        cache.pending.clear();
    }
}

#[derive(Clone)]
pub struct GitHubClient {
    pub account: AccountScope,
    shared: GitHubClients,
}

pub struct Loaded<T> {
    pub value: T,
    /// A cached snapshot cannot acknowledge a possibly committed write.
    pub from_cache: bool,
}

impl GitHubClient {
    pub fn invalidate(&self) {
        let mut cache = self.shared.cache.lock().unwrap_or_else(|p| p.into_inner());
        cache.entries.retain(|key, _| key.account != self.account);
        cache.pending.retain(|key, _| key.account != self.account);
    }

    pub async fn request(
        &self,
        request: ApiRequest,
        force: bool,
    ) -> Result<Loaded<ApiResponse>, GitHubError> {
        let key = CacheKey {
            account: self.account.clone(),
            request: request.cache_key(),
        };
        if request.is_write {
            // Also invalidate on uncertain/failed writes. A prior cached GET
            // must not be mistaken for post-write verification.
            self.invalidate();
        } else if !force {
            let cached = self
                .shared
                .cache
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .entries
                .get(&key)
                .filter(|entry| entry.inserted.elapsed() < CACHE_LIFETIME)
                .map(|entry| entry.response.clone());
            if let Some(response) = cached {
                return Ok(Loaded {
                    value: response,
                    from_cache: true,
                });
            }
        }
        let is_write = request.is_write;
        let sequence = if !is_write {
            Some(
                self.shared
                    .cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .begin(key.clone()),
            )
        } else {
            None
        };
        let result = self
            .shared
            .transport
            .request(&self.account, request)
            .await
            .and_then(|mut response| {
                response.body = response.clone().checked(is_write)?;
                Ok(response)
            });
        if is_write {
            // Reads begun during the write may also predate its commit.
            self.invalidate();
        } else if let Some(sequence) = sequence {
            self.shared
                .cache
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .complete(key, sequence, result.as_ref().ok().cloned());
        }
        Ok(Loaded {
            value: result?,
            from_cache: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    struct DeferredTransport {
        calls: std::sync::atomic::AtomicUsize,
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    impl Transport for DeferredTransport {
        fn request<'a>(
            &'a self,
            _: &'a AccountScope,
            _: ApiRequest,
        ) -> super::super::transport::ApiFuture<'a> {
            Box::pin(async move {
                let sequence = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if sequence == 0 {
                    self.started.notify_one();
                    self.release.notified().await;
                }
                Ok(ApiResponse {
                    status: 200,
                    headers: Default::default(),
                    body: json!({"sequence":sequence}),
                })
            })
        }
    }

    #[tokio::test]
    async fn a_late_older_read_cannot_replace_a_forced_refresh_in_the_cache() {
        let transport = Arc::new(DeferredTransport::default());
        let clients = GitHubClients::with_transport(transport.clone());
        let client = clients.for_account(AccountScope::new("first", "team").unwrap());
        let old_client = client.clone();
        let old = tokio::spawn(async move {
            old_client
                .request(ApiRequest::get("/repos/team/repo"), false)
                .await
        });
        transport.started.notified().await;
        assert_eq!(
            client
                .request(ApiRequest::get("/repos/team/repo"), true)
                .await
                .unwrap()
                .value
                .body["sequence"],
            1
        );
        transport.release.notify_one();
        assert_eq!(old.await.unwrap().unwrap().value.body["sequence"], 0);
        let cached = client
            .request(ApiRequest::get("/repos/team/repo"), false)
            .await
            .unwrap();
        assert!(cached.from_cache);
        assert_eq!(
            cached.value.body["sequence"], 1,
            "late old reads must not replace the acknowledged refresh"
        );
    }

    #[tokio::test]
    async fn clearing_cache_during_a_read_prevents_late_repopulation() {
        let transport = Arc::new(DeferredTransport::default());
        let clients = GitHubClients::with_transport(transport.clone());
        let client = clients.for_account(AccountScope::new("first", "team").unwrap());
        let old_client = client.clone();
        let old = tokio::spawn(async move {
            old_client
                .request(ApiRequest::get("/repos/team/repo"), false)
                .await
        });
        transport.started.notified().await;
        clients.clear();
        transport.release.notify_one();
        old.await.unwrap().unwrap();
        let current = client
            .request(ApiRequest::get("/repos/team/repo"), false)
            .await
            .unwrap();
        assert!(!current.from_cache);
        assert_eq!(current.value.body["sequence"], 1);
    }

    #[derive(Default)]
    struct RecordingTransport {
        calls: Mutex<Vec<(String, ApiRequest)>>,
    }

    impl Transport for RecordingTransport {
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
                if request.is_write {
                    return Err(GitHubError::UnknownWriteResult);
                }
                Ok(ApiResponse {
                    status: 200,
                    headers: Default::default(),
                    body: json!({"account":account.key(),"request":request.path}),
                })
            })
        }
    }

    #[tokio::test]
    async fn shared_cache_separates_accounts_queries_and_pages_and_force_really_fetches() {
        let transport = Arc::new(RecordingTransport::default());
        let clients = GitHubClients::with_transport(transport.clone());
        let first = clients.for_account(AccountScope::new("first", "team").unwrap());
        let second = clients.for_account(AccountScope::new("second", "team").unwrap());
        let request = ApiRequest::get("/repos/team/private/issues?page=1");
        assert_eq!(
            first
                .request(request.clone(), false)
                .await
                .unwrap()
                .value
                .body["account"],
            "team/first"
        );
        let loaded = second.request(request.clone(), false).await.unwrap();
        assert!(!loaded.from_cache);
        assert_eq!(loaded.value.body["account"], "team/second");
        assert!(
            first
                .request(request.clone(), false)
                .await
                .unwrap()
                .from_cache
        );
        assert_eq!(transport.calls.lock().unwrap().len(), 2);
        assert!(
            !first
                .request(request.clone(), true)
                .await
                .unwrap()
                .from_cache
        );
        first
            .request(ApiRequest::get("/repos/team/private/issues?page=2"), false)
            .await
            .unwrap();
        for cursor in ["first-page", "next-page"] {
            first
                .request(
                    ApiRequest::graphql("query { fixture }", json!({"cursor":cursor}), false),
                    false,
                )
                .await
                .unwrap();
        }
        assert_eq!(transport.calls.lock().unwrap().len(), 6);
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls[0].0, "team/first");
        assert_eq!(calls[1].0, "team/second");
        assert!(calls[3].1.path.ends_with("page=2"));
    }

    #[tokio::test]
    async fn unknown_writes_are_not_replayed_and_invalidate_only_their_accounts_snapshots() {
        let transport = Arc::new(RecordingTransport::default());
        let clients = GitHubClients::with_transport(transport.clone());
        let first = clients.for_account(AccountScope::new("first", "team").unwrap());
        let second = clients.for_account(AccountScope::new("second", "team").unwrap());
        let read = ApiRequest::get("/repos/team/private/pulls/7");
        first.request(read.clone(), false).await.unwrap();
        second.request(read.clone(), false).await.unwrap();
        let write = ApiRequest::post(
            "/repos/team/private/issues/7/comments",
            json!({"body":"one confirmed write"}),
        );
        assert!(matches!(
            first.request(write, false).await,
            Err(GitHubError::UnknownWriteResult)
        ));
        assert!(!first.request(read.clone(), false).await.unwrap().from_cache);
        assert!(second.request(read, false).await.unwrap().from_cache);
        assert_eq!(
            transport
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, r)| r.is_write)
                .count(),
            1
        );
        clients.clear();
        assert!(
            !second
                .request(ApiRequest::get("/repos/team/private/pulls/7"), false)
                .await
                .unwrap()
                .from_cache
        );
    }
}
