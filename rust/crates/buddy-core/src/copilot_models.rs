//! Read-only model discovery through the same packaged SDK runtime as chat.
use crate::copilot_runs::Failure;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
}

fn safe_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 200 && !value.chars().any(char::is_control)
}

pub(crate) fn from_sdk(
    models: Vec<github_copilot_sdk::types::Model>,
) -> Result<Vec<Model>, Failure> {
    if models.len() > 500 {
        return Err(Failure::ServiceUnavailable);
    }
    let mut result = Vec::new();
    for model in models {
        if !safe_text(&model.id)
            || !safe_text(&model.name)
            || result.iter().any(|row: &Model| row.id == model.id)
        {
            return Err(Failure::ServiceUnavailable);
        }
        result.push(Model {
            id: model.id,
            name: model.name,
        });
    }
    Ok(result)
}

pub async fn list(account: &str) -> Result<Vec<Model>, &'static str> {
    if account.trim().is_empty() && std::env::var_os("BUDDY_COPILOT_FIXTURE").is_some() {
        return Err("Choose the synthetic qualification account.");
    }
    let account = if account.trim().is_empty() {
        crate::gh::active_account()
            .await
            .ok_or("Sign in with GitHub CLI before loading models.")?
    } else {
        crate::gh::assert_valid_slug(account).map_err(|_| "Choose a valid GitHub CLI account.")?;
        account.to_owned()
    };
    let owned = crate::config::config_path()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("native-assistant")))
        .ok_or("The private assistant directory is unavailable.")?;
    let executable =
        std::env::current_exe().map_err(|_| "The packaged assistant runtime is unavailable.")?;
    let fixture = crate::copilot_fixture::load(&owned)
        .map_err(|_| "The offline model fixture is invalid.")?;
    if let Some(fixture) = &fixture {
        fixture
            .identity(&account, None)
            .map_err(|_| "Choose the synthetic qualification account.")?;
    }
    let program = crate::copilot_runtime::prepare(executable, owned.join("runtime"))
        .await
        .map_err(|_| "The packaged assistant runtime could not be prepared.")?;
    let token = if fixture.is_some() {
        None
    } else {
        Some(
            crate::gh::auth_token(&account)
                .await
                .map_err(|_| "Sign in with GitHub CLI for the selected account.")?,
        )
    };
    let turns = owned.join("turns");
    crate::copilot_sdk::private_directory(&turns)
        .map_err(|_| "The private assistant directory is unavailable.")?;
    let directory = tempfile::Builder::new()
        .prefix("models-")
        .tempdir_in(turns)
        .map_err(|_| "The private assistant directory is unavailable.")?;
    let runtime = crate::copilot_sdk::AssistantRuntime::start_with_fixture(
        program.program(),
        directory.path(),
        token,
        fixture,
    )
    .await
    .map_err(|_| "The packaged assistant runtime could not start.")?;
    let result = runtime.models(&account).await;
    let stopped = runtime.stop().await;
    drop(runtime);
    let cleaned = directory.close();
    if stopped.is_err() || cleaned.is_err() {
        return Err("Assistant cleanup could not be confirmed. Try loading models again.");
    }
    result.map_err(|failure| match failure {
        Failure::AuthenticationRequired => {
            "Copilot is not authenticated as the selected GitHub account."
        }
        _ => "Model discovery is unavailable. Check account access and retry.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model(id: &str) -> github_copilot_sdk::types::Model {
        serde_json::from_value(
            serde_json::json!({"id":id,"name":id,"capabilities":{"supports":{},"limits":{}}}),
        )
        .unwrap()
    }
    #[test]
    fn rejects_unsafe_duplicate_or_unbounded_model_lists() {
        assert!(from_sdk(vec![model("unsafe\nmodel")]).is_err());
        assert!(from_sdk(vec![model("same"), model("same")]).is_err());
        assert!(
            from_sdk(
                (0..501)
                    .map(|index| model(&format!("model-{index}")))
                    .collect()
            )
            .is_err()
        );
        assert_eq!(
            from_sdk(vec![model("synthetic-model")]).unwrap()[0].id,
            "synthetic-model"
        );
    }
    #[tokio::test]
    async fn invalid_account_never_reads_credentials_or_launches_runtime() {
        assert!(list("invalid/account").await.is_err());
    }
}
