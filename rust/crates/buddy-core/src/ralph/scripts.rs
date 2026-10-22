//! The loop scripts, their configuration catalog and templates.
//!
//! The scripts in `scripts/ralph-loops` are compiled into the app and written to a
//! content-addressed folder before a launch, so every package format (installer, `.deb`,
//! `.app`, a bare binary) runs the same scripts without a resources folder.
//! `BUDDY_RALPH_SCRIPTS_DIR` selects another folder instead, for development and for the
//! harmless stub used in qualification.
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

pub const SCRIPTS_DIR_ENV: &str = "BUDDY_RALPH_SCRIPTS_DIR";

macro_rules! bundled {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../scripts/ralph-loops/", $path)))),*]
    };
}

/// Every runnable file of `scripts/ralph-loops` (not its README or summary log).
pub(super) const BUNDLED: &[(&str, &str)] = bundled![
    "config/agents.json",
    "config/models.json",
    "config/providers.json",
    "lib/config.ps1",
    "ralph.ps1",
    "ralph-issues.ps1",
    "ralph-pr.ps1",
    "scripts/ralph-improve-crap-score.ps1",
    "scripts/ralph-improve-quality.ps1",
    "scripts/ralph-improve-react-doctor-score.ps1",
    "scripts/ralph-improve-scorecard-score.ps1",
    "scripts/ralph-improve-test-coverage.ps1",
    "scripts/ralph-repeat.ps1",
    "scripts/ralph-run-all.ps1",
    "scripts/ralph-simplisticate.ps1",
];

/// A folder holding `ralph.ps1`, `ralph-pr.ps1`, `ralph-issues.ps1`, `config/` and `scripts/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scripts {
    root: PathBuf,
}

impl Scripts {
    /// The override folder, or the bundled scripts written below `state`.
    pub fn prepare(state: &Path) -> Result<Self, String> {
        if let Some(root) = std::env::var_os(SCRIPTS_DIR_ENV).filter(|value| !value.is_empty()) {
            let root = PathBuf::from(root);
            if !root.is_absolute() || !root.join("ralph.ps1").is_file() {
                return Err(format!(
                    "{SCRIPTS_DIR_ENV} must be an absolute folder containing ralph.ps1."
                ));
            }
            return Ok(Self { root });
        }
        extract(state, BUNDLED)
            .map_err(|error| format!("The bundled Ralph scripts could not be prepared: {error}"))
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn catalog(&self) -> Result<Catalog, String> {
        let read = |name: &str| -> Result<Value, String> {
            let path = self.root.join("config").join(name);
            let text = std::fs::read_to_string(&path)
                .map_err(|_| format!("Ralph configuration not found: {}", path.display()))?;
            serde_json::from_str(&text)
                .map_err(|_| format!("Ralph configuration is not valid JSON: {}", path.display()))
        };
        Catalog::parse(
            &read("models.json")?,
            &read("providers.json")?,
            &read("agents.json")?,
        )
    }

    /// Templates in `scripts/`, sorted by file name. Wrappers that run other scripts are skipped.
    pub fn templates(&self) -> Vec<Template> {
        let Ok(entries) = std::fs::read_dir(self.root.join("scripts")) else {
            return vec![];
        };
        let mut templates = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let file = entry.file_name().into_string().ok()?;
                if !file.ends_with(".ps1") || file.contains("-repeat") || file.contains("-run-all")
                {
                    return None;
                }
                let text = std::fs::read_to_string(entry.path()).ok()?;
                Some(Template::parse(&file, &text))
            })
            .collect::<Vec<_>>();
        templates.sort_by(|a, b| a.file.cmp(&b.file));
        templates
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        relative
            .split('/')
            .fold(self.root.clone(), |path, part| path.join(part))
    }
}

fn extract(state: &Path, files: &[(&str, &str)]) -> io::Result<Scripts> {
    let mut digest = Sha256::new();
    for (name, text) in files {
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(text.as_bytes());
        digest.update([0]);
    }
    let hash = digest
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let root = state.join("scripts").join(hash);
    for (name, text) in files {
        let path = name
            .split('/')
            .fold(root.clone(), |path, part| path.join(part));
        if std::fs::read(&path).is_ok_and(|existing| existing == text.as_bytes()) {
            continue;
        }
        let parent = path.parent().ok_or_else(|| io::Error::other("no parent"))?;
        std::fs::create_dir_all(parent)?;
        // A concurrent launch writes the same bytes; the rename makes either copy whole.
        let temporary = parent.join(format!(
            ".{}.{}.tmp",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&temporary, text)?;
        if let Err(error) = std::fs::rename(&temporary, &path) {
            let _ = std::fs::remove_file(&temporary);
            if !std::fs::read(&path).is_ok_and(|existing| existing == text.as_bytes()) {
                return Err(error);
            }
        }
    }
    Ok(Scripts { root })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    /// The file name inside `scripts/`, for example `ralph-simplisticate.ps1`.
    pub file: String,
    pub name: String,
    pub description: String,
    pub default_prompt: Option<String>,
}

impl Template {
    pub(super) fn parse(file: &str, text: &str) -> Self {
        let stem = file
            .trim_end_matches(".ps1")
            .trim_start_matches("ralph-")
            .replace('-', " ");
        let name = stem
            .split(' ')
            .filter(|word| !word.is_empty())
            .map(|word| {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            file: file.to_owned(),
            name,
            description: description(text).unwrap_or_else(|| "Template improvement loop".into()),
            default_prompt: default_prompt(text),
        }
    }
}

/// The text after ` — ` or ` - ` in the first comment, else the next comment that is not a version.
fn description(text: &str) -> Option<String> {
    let comments = text
        .lines()
        .map(str::trim)
        .take_while(|line| line.starts_with('#'))
        .map(|line| line.trim_start_matches('#').trim())
        .collect::<Vec<_>>();
    let first = comments.first()?;
    if let Some((_, rest)) = first
        .split_once(" — ")
        .or_else(|| first.split_once(" - "))
        .filter(|(_, rest)| !rest.trim().is_empty())
    {
        return Some(rest.trim().trim_end_matches('.').to_owned());
    }
    comments
        .iter()
        .skip(1)
        .find(|line| !line.is_empty() && !line.starts_with("Version:"))
        .map(|line| line.trim_end_matches('.').to_owned())
}

/// The first `$somethingPrompt = @'` here-string.
fn default_prompt(text: &str) -> Option<String> {
    let mut lines = text.lines();
    lines.by_ref().find(|line| {
        let line = line.trim();
        line.starts_with('$') && line.ends_with("Prompt = @'")
    })?;
    let body = lines
        .take_while(|line| !line.starts_with("'@"))
        .collect::<Vec<_>>()
        .join("\n");
    (!body.trim().is_empty()).then_some(body)
}

/// The parameter names a script declares in its leading `param(...)` block.
pub fn declared_parameters(text: &str) -> BTreeSet<String> {
    let mut parameters = BTreeSet::new();
    let mut depth = 0usize;
    let mut inside = false;
    for line in text.lines() {
        let code = line.split('#').next().unwrap_or("").trim();
        if !inside {
            let lower = code.to_ascii_lowercase();
            if lower.starts_with("param(") || lower.starts_with("param (") {
                inside = true;
            } else {
                continue;
            }
        }
        for (index, c) in code.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                '$' if depth == 1 => {
                    let name = code[index + 1..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect::<String>();
                    if !name.is_empty() {
                        parameters.insert(name);
                    }
                }
                _ => {}
            }
        }
        if depth == 0 {
            break;
        }
    }
    parameters
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    pub key: String,
    pub label: String,
    /// The vendor a provider must support, for example `anthropic`.
    pub vendor: String,
    pub effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderChoice {
    pub key: String,
    pub description: String,
    pub vendors: Vec<String>,
    /// The model this provider runs when none is chosen (`defaultModel`).
    pub default_model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentChoice {
    pub key: String,
    pub description: String,
    /// The role's model tier, used when neither the launch nor the provider names a model.
    pub tier: String,
}

/// `models.json`, `providers.json` and `agents.json`, reduced to what launching needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    pub models: Vec<ModelChoice>,
    /// Alias or tier name, and the model key it stands for.
    pub aliases: Vec<(String, String)>,
    pub default_model: String,
    pub providers: Vec<ProviderChoice>,
    pub default_provider: String,
    pub dev_agents: Vec<AgentChoice>,
    pub review_agents: Vec<AgentChoice>,
    pub default_dev_agent: String,
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

impl Catalog {
    pub fn parse(models: &Value, providers: &Value, agents: &Value) -> Result<Self, String> {
        let object = |value: &Value, key: &str, file: &str| {
            value
                .get(key)
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| format!("{file} has no \"{key}\" section."))
        };
        let model_map = object(models, "models", "models.json")?;
        let mut aliases = models
            .get("aliases")
            .and_then(Value::as_object)
            .map(|aliases| {
                aliases
                    .iter()
                    .filter_map(|(alias, target)| {
                        Some((alias.clone(), target.as_str()?.to_owned()))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(tiers) = models.get("tiers").and_then(Value::as_object) {
            aliases.extend(tiers.iter().filter_map(|(tier, value)| {
                Some((tier.clone(), value.get("model")?.as_str()?.to_owned()))
            }));
        }
        let agents_map = object(agents, "roles", "agents.json")?;
        let role = |category: &str| {
            agents_map
                .iter()
                .filter(|(_, value)| {
                    value.get("category").and_then(Value::as_str) == Some(category)
                })
                .map(|(key, value)| AgentChoice {
                    key: key.clone(),
                    description: text(value, "description"),
                    tier: text(value, "tier"),
                })
                .collect::<Vec<_>>()
        };
        Ok(Self {
            models: model_map
                .iter()
                .map(|(key, value)| ModelChoice {
                    key: key.clone(),
                    label: Some(text(value, "label"))
                        .filter(|label| !label.is_empty())
                        .unwrap_or_else(|| key.clone()),
                    vendor: text(value, "provider"),
                    effort: value
                        .get("reasoningEffort")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                })
                .collect(),
            aliases,
            default_model: text(models, "default"),
            providers: object(providers, "providers", "providers.json")?
                .iter()
                .map(|(key, value)| ProviderChoice {
                    key: key.clone(),
                    description: text(value, "description"),
                    vendors: value
                        .get("supportedModelProviders")
                        .and_then(Value::as_array)
                        .map(|vendors| {
                            vendors
                                .iter()
                                .filter_map(|vendor| vendor.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                    default_model: Some(text(value, "defaultModel"))
                        .filter(|model| !model.is_empty()),
                })
                .collect(),
            default_provider: text(providers, "default"),
            dev_agents: role("dev"),
            review_agents: role("review"),
            default_dev_agent: agents
                .get("defaults")
                .map(|defaults| text(defaults, "devAgent"))
                .unwrap_or_default(),
        })
    }

    /// The model a key, alias or tier names.
    pub fn model(&self, name: &str) -> Option<&ModelChoice> {
        let key = self
            .aliases
            .iter()
            .find(|(alias, _)| alias == name)
            .map_or(name, |(_, target)| target.as_str());
        self.models.iter().find(|model| model.key == key)
    }

    pub fn provider(&self, name: &str) -> Option<&ProviderChoice> {
        self.providers.iter().find(|provider| provider.key == name)
    }

    /// The provider a launch uses: `provider`, or the catalog default when empty.
    pub fn effective_provider<'a>(&'a self, provider: &'a str) -> &'a str {
        if provider.is_empty() {
            &self.default_provider
        } else {
            provider
        }
    }

    /// The model a launch runs, as `Resolve-RalphEffectiveModel` decides it: the chosen
    /// model, else the provider's `defaultModel`, else the catalog default.
    pub fn effective_model<'a>(&'a self, provider: &'a str, model: &'a str) -> &'a str {
        if !model.is_empty() {
            return model;
        }
        self.provider(self.effective_provider(provider))
            .and_then(|provider| provider.default_model.as_deref())
            .unwrap_or(&self.default_model)
    }

    /// Whether `provider` (empty for the default) can run `model` (empty for its default).
    pub fn supports(&self, provider: &str, model: &str) -> bool {
        let model = self.effective_model(provider, model);
        let provider = self.effective_provider(provider);
        match (self.provider(provider), self.model(model)) {
            (Some(provider), Some(model)) => {
                provider.vendors.is_empty() || provider.vendors.contains(&model.vendor)
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/ralph-loops")
    }

    #[test]
    fn scripts_keep_buddys_account_and_ralph_pr_stops_a_failing_agent() {
        for (name, text) in BUNDLED {
            if text.contains("gh auth token --user") {
                assert!(
                    text.contains("RALPH_GH_ACCOUNT"),
                    "{name} switches gh accounts without honoring RALPH_GH_ACCOUNT"
                );
            }
        }
        let pr = BUNDLED
            .iter()
            .find(|(name, _)| *name == "ralph-pr.ps1")
            .unwrap()
            .1;
        // The work, merge-conflict and review-fix invocations of the dev agent.
        let invocations = pr
            .lines()
            .filter(|line| line.trim_start().starts_with("& $cmdExe @cmdTail"))
            .count();
        let checked = pr.matches("Test-AgentExit $LASTEXITCODE").count();
        assert_eq!((invocations, checked), (4, 3));
    }

    #[test]
    fn every_runnable_script_file_is_bundled() {
        fn walk(root: &Path, directory: &Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, found);
                } else {
                    let name = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    if name != "README.md" && name != "ralph-summary.log" {
                        found.push(name);
                    }
                }
            }
        }
        let root = scripts_dir();
        let mut found = vec![];
        walk(&root, &root, &mut found);
        found.sort();
        let mut bundled = BUNDLED
            .iter()
            .map(|(name, _)| name.to_string())
            .collect::<Vec<_>>();
        bundled.sort();
        assert_eq!(found, bundled, "add new script files to BUNDLED");
    }

    #[test]
    fn bundled_scripts_are_written_once_and_repaired_when_changed() {
        let state = tempfile::tempdir().unwrap();
        let files = [
            ("ralph.ps1", "Write-Host one"),
            ("config/models.json", "{}"),
        ];
        let first = extract(state.path(), &files).unwrap();
        assert_eq!(
            std::fs::read_to_string(first.path("config/models.json")).unwrap(),
            "{}"
        );
        std::fs::write(first.path("ralph.ps1"), "tampered").unwrap();
        let second = extract(state.path(), &files).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            std::fs::read_to_string(second.path("ralph.ps1")).unwrap(),
            "Write-Host one"
        );
        let changed = extract(state.path(), &[("ralph.ps1", "Write-Host two")]).unwrap();
        assert_ne!(changed.root(), first.root());
    }

    #[test]
    fn the_shipped_catalog_lists_models_providers_and_agents() {
        let catalog = Scripts::at(scripts_dir()).catalog().unwrap();
        assert!(catalog.model(&catalog.default_model).is_some());
        assert!(catalog.provider(&catalog.default_provider).is_some());
        assert!(
            catalog
                .dev_agents
                .iter()
                .any(|agent| agent.key == catalog.default_dev_agent)
        );
        assert!(!catalog.review_agents.is_empty());
        assert!(catalog.supports("", ""));
        assert!(catalog.model("sonnet").is_some());
        // As Resolve-RalphEffectiveModel: a provider's defaultModel beats the catalog default.
        assert_eq!(catalog.effective_model("", ""), catalog.default_model);
        assert_eq!(catalog.effective_model("openrouter", ""), "kimi-k2.6");
        assert_eq!(catalog.effective_model("openrouter", "sonnet"), "sonnet");
        assert!(catalog.supports("openrouter", ""));
    }

    #[test]
    fn providers_only_run_models_from_their_vendors() {
        let catalog = Catalog::parse(
            &serde_json::json!({"models":{"a":{"provider":"anthropic"},"d":{"provider":"deepseek"}},
                "aliases":{"short":"d"},"tiers":{"fast":{"model":"a"}},"default":"fast"}),
            &serde_json::json!({"providers":{"copilot":{"supportedModelProviders":["anthropic"]},
                "router":{"supportedModelProviders":["anthropic","deepseek"]}},"default":"copilot"}),
            &serde_json::json!({"roles":{"dev":{"category":"dev"},"rev":{"category":"review"}},
                "defaults":{"devAgent":"dev"}}),
        )
        .unwrap();
        assert!(catalog.supports("", ""));
        assert!(!catalog.supports("copilot", "short"));
        assert!(catalog.supports("router", "short"));
        assert!(!catalog.supports("missing", "a"));
        assert!(!catalog.supports("copilot", "missing"));
        assert_eq!(catalog.dev_agents[0].key, "dev");
        assert_eq!(catalog.review_agents[0].key, "rev");
        assert!(
            Catalog::parse(
                &serde_json::json!({}),
                &serde_json::json!({}),
                &serde_json::json!({})
            )
            .is_err()
        );
    }

    #[test]
    fn templates_have_names_descriptions_and_prompts() {
        let templates = Scripts::at(scripts_dir()).templates();
        assert!(
            templates
                .iter()
                .all(|template| !template.file.contains("-repeat")
                    && !template.file.contains("-run-all"))
        );
        let coverage = templates
            .iter()
            .find(|template| template.file == "ralph-improve-test-coverage.ps1")
            .unwrap();
        assert_eq!(coverage.name, "Improve Test Coverage");
        assert_eq!(coverage.description, "Test coverage and quality improver");
        assert!(coverage.default_prompt.is_some());
        let parsed = Template::parse(
            "ralph-x.ps1",
            "# ralph-x.ps1\n# Version: 1.0\n# Does a thing.\nparam()",
        );
        assert_eq!(parsed.description, "Does a thing");
        assert_eq!(parsed.default_prompt, None);
        assert_eq!(
            Template::parse("ralph-y.ps1", "param()").description,
            "Template improvement loop"
        );
    }

    #[test]
    fn declared_parameters_come_from_the_param_block_only() {
        let declared = declared_parameters(
            "# $Comment\nparam(\n    [ValidateSet('a', 'b')]\n    [string]$Model, # $NotThis\n    [switch]$NoAudio\n)\n$Other = 1",
        );
        assert_eq!(
            declared.into_iter().collect::<Vec<_>>(),
            ["Model", "NoAudio"]
        );
        let ralph = std::fs::read_to_string(scripts_dir().join("ralph.ps1")).unwrap();
        let declared = declared_parameters(&ralph);
        for name in [
            "Max",
            "Prompt",
            "Branch",
            "Model",
            "Provider",
            "Agents",
            "NoAudio",
            "Autopilot",
        ] {
            assert!(declared.contains(name), "{name}");
        }
        assert!(!declared.contains("AutoApprove"));
    }
}
