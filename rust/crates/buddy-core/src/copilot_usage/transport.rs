//! Selected credentials and the existing organization-billing fallback.
use crate::config::GitHubAccount;
use crate::github::{AccountScope, ApiRequest, GitHubClient, GitHubClients};

pub(super) struct Reader {
    token: Option<String>,
    fixture: Option<GitHubClient>,
}
impl Reader {
    pub(super) async fn new(account: &GitHubAccount) -> Result<Self, String> {
        if std::env::var_os("BUDDY_COPILOT_FIXTURE").is_some() {
            let owned = crate::config::config_path()
                .ok()
                .and_then(|path| path.parent().map(|parent| parent.join("native-assistant")))
                .ok_or("The usage fixture root is invalid.")?;
            let fixture = crate::copilot_fixture::load(&owned)
                .map_err(|_| "The usage fixture is invalid.")?
                .ok_or("The usage fixture is invalid.")?;
            fixture
                .identity(&account.username, None)
                .map_err(|_| "Choose the synthetic usage account.")?;
            let scope = AccountScope::from_account(account)
                .map_err(|_| "The usage fixture account is invalid.")?;
            if std::env::var("BUDDY_GITHUB_FIXTURE").as_deref() != Ok("1") {
                return Err("The usage GitHub fixture is unavailable.".into());
            }
            return Ok(Self {
                token: None,
                fixture: Some(GitHubClients::configured().for_account(scope)),
            });
        }
        Ok(Self {
            token: crate::gh::auth_token(&account.username).await.ok(),
            fixture: None,
        })
    }
    pub(super) fn selected(&self) -> bool {
        self.fixture.is_some() || self.token.is_some()
    }
    pub(super) async fn api(
        &self,
        path: &str,
        headers: &[&str],
    ) -> Result<String, crate::gh::GhError> {
        if let Some(client) = &self.fixture {
            return client
                .request(ApiRequest::get(path), true)
                .await
                .map(|response| response.value.body.to_string())
                .map_err(|_| {
                    crate::gh::GhError::Failed("The synthetic usage API is unavailable.".into())
                });
        }
        crate::gh::api(path, self.token.as_deref(), headers).await
    }
}
