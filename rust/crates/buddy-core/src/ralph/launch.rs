//! What a launch asks for, how it is checked, and the exact command it becomes.
//!
//! Arguments go to `pwsh -File` as separate values without a shell. A value that starts
//! with `-` would still be read as a parameter name by PowerShell, so free text (the
//! prompt) is always passed as a file and the remaining fields are validated to rule it out.
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::scripts::{Catalog, Scripts, declared_parameters};

pub const MAX_ITERATIONS: u32 = 100;
pub const MAX_CYCLES: u32 = 50;
const MAX_PROMPT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "file", rename_all = "kebab-case")]
pub enum Script {
    /// Full loop: work iterations, then a pull request handed to `ralph-pr.ps1`.
    Ralph,
    /// Shepherd an existing pull request through review.
    Pr,
    /// Scan the repository and file issues.
    Issues,
    /// A template from `scripts/`, by file name.
    Template(String),
}

impl Script {
    pub fn relative_path(&self) -> String {
        match self {
            Self::Ralph => "ralph.ps1".into(),
            Self::Pr => "ralph-pr.ps1".into(),
            Self::Issues => "ralph-issues.ps1".into(),
            Self::Template(file) => format!("scripts/{file}"),
        }
    }
    pub fn uses_iterations(&self) -> bool {
        !matches!(self, Self::Pr)
    }
    pub fn uses_cycles(&self) -> bool {
        matches!(self, Self::Ralph | Self::Template(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewAgent {
    pub role: String,
    /// `provider:model`, or `None` for the role's default model.
    pub model: Option<String>,
}

impl ReviewAgent {
    fn spec(&self) -> String {
        match &self.model {
            Some(model) => format!("{}@{model}", self.role),
            None => self.role.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    pub repository: PathBuf,
    pub script: Script,
    /// Empty strings mean the catalog default.
    pub model: String,
    pub provider: String,
    pub dev_agent: String,
    pub review_agents: Vec<ReviewAgent>,
    pub iterations: u32,
    /// How many times the whole script runs, one cycle after another.
    pub cycles: u32,
    pub branch: String,
    pub prompt: String,
    pub pr_number: Option<u32>,
    pub labels: String,
    pub dry_run: bool,
    pub auto_approve: bool,
    /// Off passes `-NoAudio`: the scripts otherwise need edge-tts and write an mp3 into the repository.
    pub audio: bool,
}

impl Default for Launch {
    fn default() -> Self {
        Self {
            repository: PathBuf::new(),
            script: Script::Ralph,
            model: String::new(),
            provider: String::new(),
            dev_agent: String::new(),
            review_agents: vec![],
            iterations: 3,
            cycles: 1,
            branch: String::new(),
            prompt: String::new(),
            pr_number: None,
            labels: String::new(),
            dry_run: false,
            auto_approve: true,
            audio: false,
        }
    }
}

/// The external programs a loop needs, resolved before anything starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tools {
    pub pwsh: PathBuf,
    /// `PATH` for the loop: the app's own plus common install folders that exist.
    pub path: OsString,
}

#[derive(Clone, Copy)]
struct Tool {
    name: &'static str,
    missing: &'static str,
}

const TOOLS: [Tool; 4] = [
    Tool {
        name: "pwsh",
        missing: "PowerShell 7 (pwsh) was not found. Install it from https://aka.ms/powershell, then restart Buddy.",
    },
    Tool {
        name: "git",
        missing: "Git was not found. Install it from https://git-scm.com/downloads, then restart Buddy.",
    },
    Tool {
        name: "gh",
        missing: "GitHub CLI (gh) was not found. Install it from https://cli.github.com/ and run gh auth login, then restart Buddy.",
    },
    Tool {
        name: "copilot",
        missing: "GitHub Copilot CLI (copilot) was not found. Install it with npm install -g @github/copilot, then restart Buddy.",
    },
];

/// Folders a desktop session may leave off `PATH` (a macOS app started from Finder gets
/// only the system folders).
fn extra_directories() -> Vec<PathBuf> {
    let mut directories = vec![];
    if cfg!(unix) {
        directories.extend(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/snap/bin",
                "/usr/bin",
            ]
            .map(PathBuf::from),
        );
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            directories.extend(
                [
                    ".local/bin",
                    ".dotnet/tools",
                    ".npm-global/bin",
                    ".volta/bin",
                ]
                .map(|folder| home.join(folder)),
            );
        }
    } else {
        // The default install folders of each tool, and the Store's PowerShell alias.
        let folder = |name: &str| std::env::var_os(name).map(PathBuf::from);
        if let Some(programs) = folder("ProgramFiles") {
            directories
                .extend(["PowerShell/7", "Git/cmd", "GitHub CLI"].map(|tool| programs.join(tool)));
        }
        if let Some(local) = folder("LOCALAPPDATA") {
            directories.push(local.join("Microsoft/WindowsApps"));
            directories.push(local.join("Programs/Git/cmd"));
        }
        if let Some(roaming) = folder("APPDATA") {
            directories.push(roaming.join("npm"));
        }
    }
    directories
}

/// [`discover`] with this process's `PATH` and the common install folders.
pub fn discover_installed() -> Result<Tools, String> {
    discover(std::env::var_os("PATH"), &extra_directories())
}

fn candidates(name: &str, extensions: &[OsString]) -> Vec<OsString> {
    if cfg!(windows) {
        extensions
            .iter()
            .map(|extension| {
                let mut file = OsString::from(name);
                file.push(extension);
                file
            })
            .collect()
    } else {
        vec![name.into()]
    }
}

fn runnable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Finds every tool on `path`, then in `extra` folders, or names the first missing one.
pub fn discover(path: Option<OsString>, extra: &[PathBuf]) -> Result<Tools, String> {
    let mut directories = path
        .as_ref()
        .map(|path| std::env::split_paths(path).collect::<Vec<_>>())
        .unwrap_or_default();
    for directory in extra {
        if directory.is_dir() && !directories.contains(directory) {
            directories.push(directory.clone());
        }
    }
    directories.retain(|directory| directory.is_absolute());
    let extensions = std::env::var_os("PATHEXT")
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(|extension| OsString::from(extension.to_ascii_lowercase()))
                .collect::<Vec<_>>()
        })
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| {
            [".com", ".exe", ".bat", ".cmd"]
                .map(OsString::from)
                .to_vec()
        });
    let find = |name: &str| {
        directories.iter().find_map(|directory| {
            candidates(name, &extensions)
                .into_iter()
                .map(|file| directory.join(file))
                .find(|path| runnable(path))
        })
    };
    let mut pwsh = None;
    for tool in TOOLS {
        match find(tool.name) {
            Some(found) if tool.name == "pwsh" => pwsh = Some(found),
            Some(_) => {}
            None => return Err(tool.missing.into()),
        }
    }
    Ok(Tools {
        pwsh: pwsh.ok_or(TOOLS[0].missing)?,
        path: std::env::join_paths(&directories).map_err(|_| "PATH contains an invalid folder.")?,
    })
}

/// The command a validated launch runs, recorded with the run so a restart never re-plans it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub program: PathBuf,
    pub script: PathBuf,
    /// Everything after `-File <script>`.
    pub arguments: Vec<String>,
    pub directory: PathBuf,
    pub path: OsString,
    pub cycles: u32,
}

impl Plan {
    pub fn command_line(&self) -> Vec<OsString> {
        let mut arguments: Vec<OsString> = vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-File".into(),
        ];
        arguments.push(self.script.clone().into());
        arguments.extend(self.arguments.iter().map(OsString::from));
        arguments
    }
}

fn inside_git_repository(directory: &Path) -> bool {
    directory
        .ancestors()
        .any(|folder| folder.join(".git").exists())
}

/// A value PowerShell will bind as data: no leading `-`, no controls, no surrounding spaces.
fn plain_value(value: &str) -> bool {
    !value.starts_with('-') && value.trim() == value && !value.chars().any(char::is_control)
}

fn valid_branch(branch: &str) -> bool {
    plain_value(branch)
        && !branch.contains("..")
        && !branch.ends_with('/')
        && !branch.ends_with(".lock")
        && !branch.starts_with('/')
        && !branch
            .chars()
            .any(|c| c.is_whitespace() || "~^:?*[\\".contains(c))
}

impl Launch {
    /// Checks every input against the catalog and the scripts folder.
    pub fn validate(
        &self,
        catalog: &Catalog,
        scripts: &Scripts,
    ) -> Result<(PathBuf, BTreeSet<String>), String> {
        let repository = &self.repository;
        if repository.as_os_str().is_empty() {
            return Err("Choose a repository folder.".into());
        }
        if !repository.is_absolute() {
            return Err("The repository path must be absolute.".into());
        }
        if !repository.is_dir() {
            return Err(format!(
                "The repository folder does not exist: {}",
                repository.display()
            ));
        }
        if !inside_git_repository(repository) {
            return Err(format!(
                "{} is not inside a Git repository.",
                repository.display()
            ));
        }
        if let Script::Template(file) = &self.script
            && !scripts
                .templates()
                .iter()
                .any(|template| &template.file == file)
        {
            return Err(format!("Template script not found: {file}"));
        }
        let script = scripts.path(&self.script.relative_path());
        let text = std::fs::read_to_string(&script)
            .map_err(|_| format!("Script not found: {}", script.display()))?;
        if !self.provider.is_empty() && catalog.provider(&self.provider).is_none() {
            return Err(format!("Unknown provider: {}", self.provider));
        }
        if !self.model.is_empty() && catalog.model(&self.model).is_none() {
            return Err(format!("Unknown model: {}", self.model));
        }
        if !catalog.supports(&self.provider, &self.model) {
            return Err(format!(
                "The {} provider cannot run {}. Choose another model or provider.",
                if self.provider.is_empty() {
                    &catalog.default_provider
                } else {
                    &self.provider
                },
                if self.model.is_empty() {
                    &catalog.default_model
                } else {
                    &self.model
                }
            ));
        }
        if !self.dev_agent.is_empty()
            && !catalog
                .dev_agents
                .iter()
                .any(|agent| agent.key == self.dev_agent)
        {
            return Err(format!("Unknown work agent: {}", self.dev_agent));
        }
        for agent in &self.review_agents {
            if !catalog
                .review_agents
                .iter()
                .any(|known| known.key == agent.role)
            {
                return Err(format!("Unknown review agent: {}", agent.role));
            }
            if let Some(spec) = &agent.model {
                let valid = spec.split_once(':').is_some_and(|(provider, model)| {
                    catalog.provider(provider).is_some()
                        && catalog.model(model).is_some()
                        && catalog.supports(provider, model)
                });
                if !valid {
                    return Err(format!(
                        "The review model {spec} for {} is not available.",
                        agent.role
                    ));
                }
            }
        }
        if self.script == Script::Pr && self.pr_number.is_none_or(|number| number == 0) {
            return Err("Enter the pull request number for Ralph PR.".into());
        }
        if self.script.uses_iterations() && !(1..=MAX_ITERATIONS).contains(&self.iterations) {
            return Err(format!(
                "Iterations must be a whole number from 1 through {MAX_ITERATIONS}."
            ));
        }
        if !(1..=MAX_CYCLES).contains(&self.cycles)
            || (!self.script.uses_cycles() && self.cycles != 1)
        {
            return Err(format!(
                "Repeat cycles must be a whole number from 1 through {MAX_CYCLES}."
            ));
        }
        if !self.branch.is_empty() && !valid_branch(&self.branch) {
            return Err(
                "Branch names cannot start with '-' or contain spaces, '..' or ~ ^ : ? * [ \\."
                    .into(),
            );
        }
        if !self.labels.is_empty()
            && !self
                .labels
                .split(',')
                .all(|label| !label.trim().is_empty() && plain_value(label.trim()))
        {
            return Err("Labels are comma-separated names that cannot start with '-'.".into());
        }
        if self.prompt.len() > MAX_PROMPT || self.prompt.contains('\0') {
            return Err("The prompt must be under 64 KB.".into());
        }
        Ok((script, declared_parameters(&text)))
    }

    /// The arguments for `script`, limited to the parameters it declares. The prompt
    /// travels as `prompt_file`, which the scripts read when the value is an existing path.
    pub fn arguments(
        &self,
        declared: &BTreeSet<String>,
        prompt_file: Option<&Path>,
    ) -> Vec<String> {
        let mut arguments = vec![];
        let mut add = |name: &str, value: Option<String>| {
            if declared.contains(name) {
                arguments.push(format!("-{name}"));
                arguments.extend(value);
            }
        };
        if self.script != Script::Issues {
            add("Autopilot", None);
        }
        if !self.model.is_empty() {
            add("Model", Some(self.model.clone()));
        }
        if !self.provider.is_empty() {
            add("Provider", Some(self.provider.clone()));
        }
        let reviews = self.review_agents.iter().map(ReviewAgent::spec);
        let agents = if self.script == Script::Pr {
            if !self.dev_agent.is_empty() {
                add("DevAgent", Some(self.dev_agent.clone()));
            }
            reviews.collect::<Vec<_>>()
        } else {
            (!self.dev_agent.is_empty())
                .then(|| self.dev_agent.clone())
                .into_iter()
                .chain(reviews)
                .collect()
        };
        if !agents.is_empty() {
            add("Agents", Some(agents.join(",")));
        }
        if self.script.uses_iterations() {
            add("Max", Some(self.iterations.to_string()));
        }
        if !self.branch.is_empty() {
            add("Branch", Some(self.branch.clone()));
        }
        if let Some(file) = prompt_file {
            add("Prompt", Some(file.to_string_lossy().into_owned()));
        }
        if self.script == Script::Pr
            && let Some(number) = self.pr_number
        {
            add("PRNumber", Some(number.to_string()));
        }
        if self.script == Script::Issues {
            if !self.labels.is_empty() {
                let labels = self
                    .labels
                    .split(',')
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join(",");
                add("Labels", Some(labels));
            }
            if self.dry_run {
                add("DryRun", None);
            }
        }
        if !self.audio {
            add("NoAudio", None);
        }
        // ralph.ps1 hands its pull request to ralph-pr.ps1 without approval, so only a
        // Ralph PR run can approve.
        if self.auto_approve && self.script == Script::Pr {
            add("AutoApprove", None);
        }
        arguments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts() -> Scripts {
        Scripts::at(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/ralph-loops"))
    }

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::Builder::new()
            .prefix("ralph repo ")
            .tempdir()
            .unwrap();
        std::fs::create_dir(directory.path().join(".git")).unwrap();
        directory
    }

    fn launch(repository: &Path) -> Launch {
        Launch {
            repository: repository.to_path_buf(),
            ..Launch::default()
        }
    }

    fn error(launch: &Launch) -> String {
        let scripts = scripts();
        launch
            .validate(&scripts.catalog().unwrap(), &scripts)
            .unwrap_err()
    }

    #[test]
    fn repository_inputs_are_checked_before_anything_starts() {
        assert_eq!(error(&Launch::default()), "Choose a repository folder.");
        assert_eq!(
            error(&launch(Path::new("relative/repo"))),
            "The repository path must be absolute."
        );
        let missing = std::env::temp_dir().join("buddy-ralph-missing-界");
        assert!(error(&launch(&missing)).starts_with("The repository folder does not exist:"));
        let plain = tempfile::tempdir().unwrap();
        assert!(error(&launch(plain.path())).ends_with("is not inside a Git repository."));
        let repository = repository();
        let nested = repository.path().join("sub folder");
        std::fs::create_dir(&nested).unwrap();
        let scripts = scripts();
        assert!(
            launch(&nested)
                .validate(&scripts.catalog().unwrap(), &scripts)
                .is_ok()
        );
    }

    #[test]
    fn model_provider_agents_and_loop_options_are_validated() {
        let repository = repository();
        let base = launch(repository.path());
        let check = |change: &dyn Fn(&mut Launch)| {
            let mut launch = base.clone();
            change(&mut launch);
            error(&launch)
        };
        assert_eq!(
            check(&|l| l.provider = "nope".into()),
            "Unknown provider: nope"
        );
        assert_eq!(check(&|l| l.model = "nope".into()), "Unknown model: nope");
        assert!(
            check(&|l| {
                l.provider = "copilot".into();
                l.model = "deepseek-v4-pro".into();
            })
            .starts_with("The copilot provider cannot run deepseek-v4-pro.")
        );
        assert_eq!(
            check(&|l| l.dev_agent = "nope".into()),
            "Unknown work agent: nope"
        );
        assert_eq!(
            check(&|l| l.review_agents = vec![ReviewAgent {
                role: "anvil".into(),
                model: None
            }]),
            "Unknown review agent: anvil"
        );
        assert!(
            check(&|l| l.review_agents = vec![ReviewAgent {
                role: "pr-review-general".into(),
                model: Some("copilot:nope".into())
            }])
            .starts_with("The review model copilot:nope")
        );
        assert_eq!(
            check(&|l| l.script = Script::Pr),
            "Enter the pull request number for Ralph PR."
        );
        assert!(check(&|l| l.iterations = 0).starts_with("Iterations must be"));
        assert!(check(&|l| l.iterations = 101).starts_with("Iterations must be"));
        assert!(check(&|l| l.cycles = 51).starts_with("Repeat cycles must be"));
        assert!(
            check(&|l| {
                l.script = Script::Issues;
                l.cycles = 2;
            })
            .starts_with("Repeat cycles must be")
        );
        for branch in ["-x", "a b", "a..b", "feature/", "x.lock", "a:b"] {
            assert!(
                check(&|l| l.branch = branch.into()).starts_with("Branch names"),
                "{branch}"
            );
        }
        assert!(check(&|l| l.labels = "a,,b".into()).starts_with("Labels"));
        assert!(check(&|l| l.labels = "a,-b".into()).starts_with("Labels"));
        assert_eq!(
            check(&|l| l.script = Script::Template("ralph-repeat.ps1".into())),
            "Template script not found: ralph-repeat.ps1"
        );
        assert!(check(&|l| l.prompt = "x".repeat(MAX_PROMPT + 1)).starts_with("The prompt"));
    }

    #[test]
    fn arguments_follow_each_scripts_declared_parameters() {
        let repository = repository();
        let scripts = scripts();
        let catalog = scripts.catalog().unwrap();
        let mut launch = launch(repository.path());
        launch.model = "sonnet".into();
        launch.dev_agent = "anvil".into();
        launch.review_agents = vec![
            ReviewAgent {
                role: "pr-review-general".into(),
                model: None,
            },
            ReviewAgent {
                role: "pr-review-security".into(),
                model: Some("copilot:gpt-5.5".into()),
            },
        ];
        launch.branch = "feature/x".into();
        let (_, declared) = launch.validate(&catalog, &scripts).unwrap();
        let prompt = repository.path().join("prompt with space.md");
        assert_eq!(
            launch.arguments(&declared, Some(&prompt)),
            [
                "-Autopilot",
                "-Model",
                "sonnet",
                "-Agents",
                "anvil,pr-review-general,pr-review-security@copilot:gpt-5.5",
                "-Max",
                "3",
                "-Branch",
                "feature/x",
                "-Prompt",
                prompt.to_str().unwrap(),
                "-NoAudio",
            ]
        );
        launch.script = Script::Pr;
        launch.pr_number = Some(42);
        launch.audio = true;
        let (_, declared) = launch.validate(&catalog, &scripts).unwrap();
        assert_eq!(
            launch.arguments(&declared, None),
            [
                "-Autopilot",
                "-Model",
                "sonnet",
                "-DevAgent",
                "anvil",
                "-Agents",
                "pr-review-general,pr-review-security@copilot:gpt-5.5",
                "-PRNumber",
                "42",
                "-AutoApprove",
            ]
        );
        launch.script = Script::Issues;
        launch.labels = " tech-debt , automated".into();
        launch.dry_run = true;
        launch.review_agents.clear();
        launch.dev_agent.clear();
        let (_, declared) = launch.validate(&catalog, &scripts).unwrap();
        assert_eq!(
            launch.arguments(&declared, None),
            [
                "-Model",
                "sonnet",
                "-Max",
                "3",
                "-Labels",
                "tech-debt,automated",
                "-DryRun"
            ]
        );
    }

    #[test]
    fn only_a_ralph_pr_run_can_auto_approve() {
        let declared: BTreeSet<String> = ["AutoApprove".to_string()].into();
        for script in [
            Script::Ralph,
            Script::Issues,
            Script::Template("x.ps1".into()),
        ] {
            let launch = Launch {
                script,
                ..Launch::default()
            };
            assert!(
                !launch
                    .arguments(&declared, None)
                    .contains(&"-AutoApprove".into())
            );
        }
        let launch = Launch {
            script: Script::Pr,
            ..Launch::default()
        };
        assert!(
            launch
                .arguments(&declared, None)
                .contains(&"-AutoApprove".into())
        );
    }

    #[test]
    fn missing_tools_name_what_to_install() {
        let only = |names: &[&str]| {
            let directory = tempfile::Builder::new()
                .prefix("tools dir ")
                .tempdir()
                .unwrap();
            for name in names {
                let file = directory.path().join(if cfg!(windows) {
                    format!("{name}.exe")
                } else {
                    name.to_string()
                });
                std::fs::write(&file, "").unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                }
            }
            directory
        };
        let path = |directory: &Path| Some(std::env::join_paths([directory]).unwrap());
        let none = only(&[]);
        assert!(
            discover(path(none.path()), &[])
                .unwrap_err()
                .starts_with("PowerShell 7")
        );
        assert!(discover(None, &[]).unwrap_err().starts_with("PowerShell 7"));
        let some = only(&["pwsh", "git", "gh"]);
        assert!(
            discover(path(some.path()), &[])
                .unwrap_err()
                .starts_with("GitHub Copilot CLI")
        );
        // A tool in a common install folder off PATH is found, and that folder joins the loop's PATH.
        let copilot = only(&["copilot"]);
        let tools = discover(path(some.path()), &[copilot.path().to_path_buf()]).unwrap();
        assert!(tools.pwsh.starts_with(some.path()));
        assert!(std::env::split_paths(&tools.path).any(|folder| folder == copilot.path()));
    }

    #[cfg(unix)]
    #[test]
    fn files_without_execute_permission_are_not_tools() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["pwsh", "git", "gh", "copilot"] {
            std::fs::write(directory.path().join(name), "").unwrap();
        }
        let error = discover(Some(std::env::join_paths([directory.path()]).unwrap()), &[]);
        assert!(error.unwrap_err().starts_with("PowerShell 7"));
    }
}
