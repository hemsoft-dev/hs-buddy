//! Installed release qualification uses fake credentials on a guarded loopback
//! origin. An invalid fixture never falls back to production or GitHub CLI.
use super::{AccountScope, ApiRequest, GitHubClients, GitHubError, Transport};
use std::path::Path;
use std::sync::Arc;

const MARKER: &str = "hs-buddy native GitHub qualification v1";

impl GitHubClients {
    pub fn configured() -> Self {
        let Some(endpoint) = std::env::var_os("BUDDY_GITHUB_FIXTURE_URL") else {
            return Self::default();
        };
        let result = (|| {
            if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
                || std::env::var("BUDDY_GITHUB_FIXTURE").as_deref() != Ok("1")
            {
                return Err(GitHubError::InvalidFixture);
            }
            let config = std::env::var_os(crate::config::CONFIG_PATH_ENV)
                .ok_or(GitHubError::InvalidFixture)?;
            let runner = std::env::var_os("RUNNER_TEMP").ok_or(GitHubError::InvalidFixture)?;
            let base = validate(
                endpoint.to_str().ok_or(GitHubError::InvalidFixture)?,
                Path::new(&config),
                Path::new(&runner),
            )?;
            super::transport::fixture_transport(base)
        })();
        Self::with_transport(result.unwrap_or_else(|_| Arc::new(InvalidFixture)))
    }
}
struct InvalidFixture;
impl Transport for InvalidFixture {
    fn request<'a>(
        &'a self,
        _: &'a AccountScope,
        _: ApiRequest,
    ) -> super::transport::ApiFuture<'a> {
        Box::pin(async { Err(GitHubError::InvalidFixture) })
    }
}
fn validate(endpoint: &str, config: &Path, runner: &Path) -> Result<reqwest::Url, GitHubError> {
    let fail = || GitHubError::InvalidFixture;
    let runner = runner.canonicalize().map_err(|_| fail())?;
    let config = config.canonicalize().map_err(|_| fail())?;
    if !config.starts_with(&runner) || !config.is_file() {
        return Err(fail());
    }
    let marker = config
        .parent()
        .ok_or_else(fail)?
        .join("github-fixture.allow");
    let marker = marker.canonicalize().map_err(|_| fail())?;
    if !marker.starts_with(&runner)
        || std::fs::metadata(&marker).map_err(|_| fail())?.len() > 128
        || std::fs::read_to_string(marker).map_err(|_| fail())?.trim() != MARKER
    {
        return Err(fail());
    }
    let base = reqwest::Url::parse(endpoint).map_err(|_| fail())?;
    if base.scheme() != "http"
        || !matches!(base.host_str(), Some("127.0.0.1" | "[::1]"))
        || base.port().is_none_or(|port| port == 0)
        || !base.username().is_empty()
        || base.password().is_some()
        || base.path() != "/"
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(fail());
    }
    Ok(base)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_requires_owned_files_and_an_exact_numeric_loopback_origin() {
        let runner = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let config = runner.path().join("config.json");
        std::fs::write(&config, "{}").unwrap();
        assert!(validate("http://127.0.0.1:4321/", &config, runner.path()).is_err());
        std::fs::write(runner.path().join("github-fixture.allow"), MARKER).unwrap();
        assert!(validate("http://127.0.0.1:4321/", &config, runner.path()).is_ok());
        for endpoint in [
            "https://api.github.com/",
            "http://localhost:4321/",
            "http://127.0.0.1.evil.test:4321/",
            "http://127.0.0.1:4321/private",
            "http://user@127.0.0.1:4321/",
            "http://127.0.0.1:4321/?token=fixture",
        ] {
            assert!(
                validate(endpoint, &config, runner.path()).is_err(),
                "{endpoint}"
            );
        }
        assert!(validate("http://127.0.0.1:4321/", &config, other.path()).is_err());
    }
    #[tokio::test]
    async fn fixture_credentials_never_call_github_cli_or_accept_a_real_account() {
        let transport = super::super::transport::fixture_transport(
            reqwest::Url::parse("http://127.0.0.1:1/").unwrap(),
        )
        .unwrap();
        let result = transport
            .request(
                &AccountScope::new("HemSoft", "hemsoft-dev").unwrap(),
                ApiRequest::get("/user"),
            )
            .await;
        assert!(matches!(result, Err(GitHubError::Authentication)));
    }
}
