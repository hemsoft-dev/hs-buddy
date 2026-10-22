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
    /// Only a Ralph Loop can merge its pull request (`-MergeWhenClean`), and a further
    /// cycle builds on the previous one only once that pull request is merged.
    pub fn uses_cycles(&self) -> bool {
        matches!(self, Self::Ralph)
    }
    /// The scripts that can merge their pull request when it is clean.
    pub fn can_merge(&self) -> bool {
        matches!(self, Self::Ralph | Self::Pr)
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
    /// Ralph PR only: merge the pull request when it is clean. With `-Autopilot` the
    /// script then runs `gh pr merge --squash --delete-branch --admin`, which bypasses
    /// branch protection, so it is off unless chosen.
    pub merge: bool,
    /// The GitHub account the loop works as: the host gives the loop that account's
    /// `gh auth token` as `GH_TOKEN`. `None` leaves gh's active account in charge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
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
            merge: false,
            account: None,
        }
    }
}

/// The external programs a loop needs, resolved before anything starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tools {
    pub pwsh: PathBuf,
    pub git: PathBuf,
    pub gh: PathBuf,
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
    let mut found = vec![];
    for tool in TOOLS {
        found.push(find(tool.name).ok_or(tool.missing)?);
    }
    Ok(Tools {
        pwsh: found[0].clone(),
        git: found[1].clone(),
        gh: found[2].clone(),
        path: std::env::join_paths(&directories).map_err(|_| "PATH contains an invalid folder.")?,
    })
}

/// The command a validated launch runs, recorded with the run so a restart never re-plans it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub program: PathBuf,
    /// Git, for fast-forwarding between repeat cycles.
    #[serde(default)]
    pub git: PathBuf,
    /// GitHub CLI, for the account's token when the launch names one.
    #[serde(default)]
    pub gh: PathBuf,
    #[serde(default)]
    pub account: Option<String>,
    pub script: PathBuf,
    /// Everything after `-File <script>`.
    pub arguments: Vec<String>,
    pub directory: PathBuf,
    pub path: OsString,
    pub cycles: u32,
}

/// A PowerShell single-quoted literal. PowerShell also ends such strings at typographic
/// single quotes, so every quote character is doubled.
fn quoted(value: &str) -> String {
    let mut text = String::from("'");
    for c in value.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            text.push(c);
        }
        text.push(c);
    }
    text.push('\'');
    text
}

fn parameter_name(argument: &str) -> bool {
    argument
        .strip_prefix('-')
        .is_some_and(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic()))
}

impl Plan {
    /// The run's launcher: UTF-8 output, then the script with each value as a literal.
    ///
    /// The console's code page decides PowerShell's output encoding otherwise, and on
    /// Windows that is a legacy code page that cannot show most text.
    pub fn launcher(&self) -> String {
        let mut call = format!("& {}", quoted(&self.script.to_string_lossy()));
        for argument in &self.arguments {
            call.push(' ');
            if parameter_name(argument) {
                call.push_str(argument);
            } else {
                call.push_str(&quoted(argument));
            }
        }
        // `$?` is false only when the script itself ends with a non-zero `exit`. A script
        // that ends normally succeeded, even if a native command it handled failed last
        // and left its code in $LASTEXITCODE.
        format!(
            "# Written by Buddy for this run.\n[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n$global:LASTEXITCODE = 0\n{call}\nif ($?) {{ exit 0 }}\nif ($LASTEXITCODE) {{ exit $LASTEXITCODE }}\nexit 1\n"
        )
    }

    pub fn command_line(&self, launcher: &Path) -> Vec<OsString> {
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-File".into(),
            launcher.as_os_str().to_owned(),
        ]
    }
}

/// Asks Git, as the scripts do (`git rev-parse --show-toplevel`), whether `repository` is
/// in a working tree, and returns the repository's shared Git folder. An empty `.git`
/// folder or a stale worktree link passes the cheap ancestor check but fails here, before
/// anything is recorded. Every subfolder and linked worktree of one repository shares
/// that folder, so it identifies the repository for the one-loop guard.
pub fn verify_repository(repository: &Path, tools: &Tools) -> Result<PathBuf, String> {
    let mut command = std::process::Command::new(&tools.git);
    command
        .args(["rev-parse", "--show-toplevel", "--git-common-dir"])
        .current_dir(repository)
        .env("PATH", &tools.path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|error| format!("Git could not check the repository: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    match (output.status.success(), text.lines().nth(1)) {
        (true, Some(common)) if !common.trim().is_empty() => {
            // Older Git prints it relative to the folder it ran in.
            let common = repository.join(common.trim());
            Ok(common.canonicalize().unwrap_or(common))
        }
        _ => Err(format!(
            "{} is not inside a Git repository.",
            repository.display()
        )),
    }
}

/// Git's trimmed output in `repository`, or `None` when it fails.
fn git_output(repository: &Path, tools: &Tools, arguments: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new(&tools.git);
    command
        .args(arguments)
        .current_dir(repository)
        .env("PATH", &tools.path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// Repeat cycles build on each cycle's pull request, which merges into the remote's
/// default branch, and Buddy only fast-forwards the checked-out branch between cycles. So
/// that branch must track the default branch; Buddy never switches it.
pub fn verify_cycle_base(repository: &Path, tools: &Tools) -> Result<(), String> {
    let git = |arguments: &[&str]| git_output(repository, tools, arguments);
    let untracked = "Repeat cycles need the checked-out branch to track the remote's default branch, where each cycle's pull request merges.";
    let upstream = git(&["rev-parse", "--abbrev-ref", "@{upstream}"]).ok_or(untracked)?;
    // The branch's own remote setting: a remote name may contain '/', so the
    // abbreviated upstream (`team/origin/main`) cannot be split to find it.
    let branch = git(&["symbolic-ref", "-q", "HEAD"]).ok_or(untracked)?;
    let remote =
        git(&["for-each-ref", "--format=%(upstream:remotename)", &branch]).ok_or(untracked)?;
    // ralph.ps1 pushes each cycle's branch to origin and opens its pull request there, and
    // the host's fast-forward between cycles pulls the branch's upstream: both must be
    // origin, or the next cycle starts without the merged work.
    if remote != "origin" {
        return Err(format!(
            "Repeat cycles push each pull request to origin, so check out a branch that tracks origin's default branch (this one tracks {upstream})."
        ));
    }
    let default = git(&["rev-parse", "--abbrev-ref", &format!("{remote}/HEAD")]).ok_or_else(|| {
        format!(
            "Git does not know {remote}'s default branch. Run git remote set-head {remote} --auto, then launch again."
        )
    })?;
    if upstream == default {
        Ok(())
    } else {
        Err(format!(
            "Repeat cycles build on pull requests merged into {default}, so check out a branch that tracks it (this one tracks {upstream})."
        ))
    }
}

/// A quick check before Git runs. Symlinks are resolved first, so a link to a folder
/// inside a repository passes; [`verify_repository`] decides.
fn inside_git_repository(directory: &Path) -> bool {
    let directory = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    directory
        .ancestors()
        .any(|folder| folder.join(".git").exists())
}

/// A value PowerShell will bind as data: no leading `-`, no controls, no surrounding spaces.
fn plain_value(value: &str) -> bool {
    !value.starts_with('-') && value.trim() == value && !value.chars().any(char::is_control)
}

/// A branch name `git check-ref-format --branch` accepts, and that PowerShell binds as data.
fn valid_branch(branch: &str) -> bool {
    plain_value(branch)
        && branch != "@"
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.ends_with('.')
        && !branch
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
        // Every slash-separated part: not empty (no leading, trailing or double slash),
        // not dot-prefixed, and not ending in .lock.
        && branch
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

impl Launch {
    /// This launch with Default replaced by what it means now: the provider, the concrete
    /// model (an alias or tier resolved), the work agent, and each review agent's
    /// `provider:model`. A run records these, so Retry repeats them even after the bundled
    /// catalog changes.
    pub fn resolved(&self, catalog: &Catalog) -> Self {
        let mut launch = self.clone();
        let model = catalog.effective_model(&self.provider, &self.model);
        launch.model = catalog
            .model(model)
            .map_or_else(|| model.to_owned(), |choice| choice.key.clone());
        launch.provider = catalog.effective_provider(&self.provider).to_owned();
        if launch.dev_agent.is_empty() {
            launch.dev_agent = catalog.default_dev_agent.clone();
        }
        // As `Resolve-RalphAgent`: the provider's defaultModel, else the role's tier.
        let provider = launch.provider.clone();
        let default_model = catalog
            .provider(&provider)
            .and_then(|choice| choice.default_model.clone());
        for agent in launch
            .review_agents
            .iter_mut()
            .filter(|agent| agent.model.is_none())
        {
            let name = default_model.clone().or_else(|| {
                catalog
                    .review_agents
                    .iter()
                    .find(|role| role.key == agent.role)
                    .map(|role| role.tier.clone())
                    .filter(|tier| !tier.is_empty())
            });
            agent.model = name
                .and_then(|name| catalog.model(&name))
                .map(|model| format!("{provider}:{}", model.key));
        }
        launch
    }

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
        if let Some(account) = &self.account
            && !crate::gh::is_valid_slug(account)
        {
            return Err(format!("{account} is not a valid GitHub account name."));
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
                catalog.effective_provider(&self.provider),
                catalog.effective_model(&self.provider, &self.model)
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
        if self.cycles > 1 && !self.merge {
            return Err("Repeat cycles build on each merged pull request. Turn on merging, or run one cycle.".into());
        }
        if !self.branch.is_empty() && !valid_branch(&self.branch) {
            return Err(
                "Branch names must be valid Git branches: no spaces, '..', '@{' or ~ ^ : ? * [ \\, no part that is empty, starts with '.' or ends with '.lock', and no leading '-'."
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
        // ralph-issues.ps1 opens no pull request and ignores review roles.
        let reviews = self
            .review_agents
            .iter()
            .filter(|_| self.script != Script::Issues)
            .map(ReviewAgent::spec);
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
        // The scripts' audio clips are not shipped, and `edge-tts` would write an mp3 into
        // the repository, so loops always run silent.
        add("NoAudio", None);
        // ralph-pr.ps1 merges with -AutoApprove; ralph.ps1 passes -MergeWhenClean on as that.
        if self.merge {
            match self.script {
                Script::Pr => {
                    add("AutoApprove", None);
                    // A merge Buddy was asked for that did not happen is a failed run.
                    add("RequireMerge", None);
                }
                Script::Ralph => add("MergeWhenClean", None),
                _ => {}
            }
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
            check(&|l| l.account = Some("--user".into())),
            "--user is not a valid GitHub account name."
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
        // git-check-ref-format's branch rules.
        for branch in [
            "-x",
            "a b",
            "a..b",
            "feature/",
            "x.lock",
            "a:b",
            ".topic",
            "topic//next",
            "topic.",
            "topic@{next",
            "@",
            "a/.b",
            "a/b.lock/c",
            "/a",
        ] {
            assert!(
                check(&|l| l.branch = branch.into()).starts_with("Branch names"),
                "{branch}"
            );
        }
        for branch in ["feature/x", "fix/issue-12", "a.b", "release@2"] {
            let mut launch = base.clone();
            launch.branch = branch.into();
            let scripts = scripts();
            assert!(
                launch
                    .validate(&scripts.catalog().unwrap(), &scripts)
                    .is_ok(),
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
        launch.merge = true;
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
                "-NoAudio",
                "-AutoApprove",
                "-RequireMerge",
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
                "-DryRun",
                "-NoAudio"
            ]
        );
    }

    #[test]
    fn the_launcher_sets_utf8_and_passes_every_value_as_a_literal() {
        let plan = Plan {
            program: "pwsh".into(),
            git: "git".into(),
            gh: "gh".into(),
            account: None,
            script: PathBuf::from("D:/scripts/it's/ralph.ps1"),
            arguments: [
                "-Autopilot",
                "-Branch",
                "fix/a’b",
                "-Prompt",
                "C:/runs/x y/prompt.md",
                "-Max",
                "3",
            ]
            .map(String::from)
            .to_vec(),
            directory: "D:/repo".into(),
            path: Default::default(),
            cycles: 1,
        };
        let text = plan.launcher();
        assert!(text.contains("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n"));
        assert!(text.contains(
            "& 'D:/scripts/it''s/ralph.ps1' -Autopilot -Branch 'fix/a’’b' -Prompt 'C:/runs/x y/prompt.md' -Max '3'\n"
        ));
        assert!(text.ends_with(
            "\nif ($?) { exit 0 }\nif ($LASTEXITCODE) { exit $LASTEXITCODE }\nexit 1\n"
        ));
        assert_eq!(
            plan.command_line(Path::new("launch.ps1")),
            ["-NoProfile", "-NonInteractive", "-File", "launch.ps1"].map(OsString::from)
        );
    }

    #[test]
    fn only_a_chosen_ralph_pr_merge_passes_auto_approve() {
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
        // Merging is never the default.
        assert!(
            !launch
                .arguments(&declared, None)
                .contains(&"-AutoApprove".into())
        );
        let launch = Launch {
            script: Script::Pr,
            merge: true,
            ..Launch::default()
        };
        assert!(
            launch
                .arguments(&declared, None)
                .contains(&"-AutoApprove".into())
        );
        // A Ralph Loop merges through ralph.ps1's own switch, never -AutoApprove.
        let declared: BTreeSet<String> =
            ["AutoApprove".to_string(), "MergeWhenClean".to_string()].into();
        let launch = Launch {
            merge: true,
            ..Launch::default()
        };
        let arguments = launch.arguments(&declared, None);
        assert!(arguments.contains(&"-MergeWhenClean".into()));
        assert!(!arguments.contains(&"-AutoApprove".into()));
    }

    #[test]
    fn ralph_issues_never_receives_review_roles() {
        let declared: BTreeSet<String> = ["Agents".to_string()].into();
        let launch = Launch {
            script: Script::Issues,
            dev_agent: "anvil".into(),
            review_agents: vec![ReviewAgent {
                role: "pr-review-general".into(),
                model: None,
            }],
            ..Launch::default()
        };
        assert_eq!(launch.arguments(&declared, None), ["-Agents", "anvil"]);
    }

    #[test]
    fn repeat_cycles_need_merging_and_only_ralph_loops_repeat() {
        let repository = repository();
        let scripts = scripts();
        let catalog = scripts.catalog().unwrap();
        let mut launch = launch(repository.path());
        launch.cycles = 2;
        assert!(
            launch
                .validate(&catalog, &scripts)
                .unwrap_err()
                .starts_with("Repeat cycles build on each merged pull request.")
        );
        launch.merge = true;
        assert!(launch.validate(&catalog, &scripts).is_ok());
        launch.script = Script::Template("ralph-simplisticate.ps1".into());
        assert!(
            launch
                .validate(&catalog, &scripts)
                .unwrap_err()
                .starts_with("Repeat cycles must be")
        );
        let ralph = std::fs::read_to_string(scripts.path("ralph.ps1")).unwrap();
        assert!(declared_parameters(&ralph).contains("MergeWhenClean"));
    }

    #[test]
    fn a_run_records_what_default_meant() {
        let catalog = scripts().catalog().unwrap();
        let resolved = Launch::default().resolved(&catalog);
        assert_eq!(resolved.provider, catalog.default_provider);
        assert_eq!(resolved.dev_agent, catalog.default_dev_agent);
        // The catalog default is a tier; the run records the model it stands for.
        let tier = catalog.model(&catalog.default_model).unwrap();
        assert_eq!(resolved.model, tier.key);
        let resolved = Launch {
            provider: "openrouter".into(),
            ..Launch::default()
        }
        .resolved(&catalog);
        assert_eq!(resolved.model, "kimi-k2.6");
        let chosen = Launch {
            model: "sonnet".into(),
            dev_agent: "developer-senior".into(),
            ..Launch::default()
        }
        .resolved(&catalog);
        assert_eq!(
            (chosen.model.as_str(), chosen.dev_agent.as_str()),
            ("claude-sonnet-4.6", "developer-senior")
        );
        // Review agents on Default record what their role resolves to; a chosen model stays.
        let reviews = |provider: &str| {
            Launch {
                provider: provider.into(),
                review_agents: vec![
                    ReviewAgent {
                        role: "pr-review-general".into(),
                        model: None,
                    },
                    ReviewAgent {
                        role: "pr-review-quality".into(),
                        model: Some("copilot:sonnet".into()),
                    },
                ],
                ..Launch::default()
            }
            .resolved(&catalog)
            .review_agents
        };
        let general = catalog
            .review_agents
            .iter()
            .find(|role| role.key == "pr-review-general")
            .unwrap();
        let tier = catalog.model(&general.tier).unwrap();
        let resolved = reviews("");
        assert_eq!(
            resolved[0].model,
            Some(format!("{}:{}", catalog.default_provider, tier.key))
        );
        assert_eq!(resolved[1].model.as_deref(), Some("copilot:sonnet"));
        // A provider's defaultModel wins over the role's tier, as in the scripts.
        assert_eq!(
            reviews("openrouter")[0].model.as_deref(),
            Some("openrouter:kimi-k2.6")
        );
        let resolved_launch = Launch {
            review_agents: vec![ReviewAgent {
                role: "pr-review-general".into(),
                model: None,
            }],
            ..Launch::default()
        }
        .resolved(&catalog);
        let repository = repository();
        let scripts = scripts();
        let launch = Launch {
            repository: repository.path().to_path_buf(),
            script: Script::Pr,
            pr_number: Some(1),
            ..resolved_launch
        };
        assert!(launch.validate(&catalog, &scripts).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_into_a_repository_passes_the_quick_check() {
        let repository = repository();
        let inner = repository.path().join("packages/app");
        std::fs::create_dir_all(&inner).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let link = elsewhere.path().join("project");
        std::os::unix::fs::symlink(&inner, &link).unwrap();
        assert!(inside_git_repository(&link));
    }

    #[test]
    fn repeat_cycles_need_a_checkout_that_tracks_the_default_branch() {
        let tools = Tools {
            pwsh: PathBuf::from("pwsh"),
            git: PathBuf::from("git"),
            gh: PathBuf::from("gh"),
            path: std::env::var_os("PATH").unwrap_or_default(),
        };
        let git = |directory: &Path, arguments: &[&str]| {
            let status = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=Buddy Test",
                    "-c",
                    "user.email=test@example.invalid",
                ])
                .args(arguments)
                .current_dir(directory)
                .status()
                .unwrap();
            assert!(status.success(), "git {arguments:?}");
        };
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remote.git");
        git(
            root.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
        );
        let seed = root.path().join("seed");
        git(
            root.path(),
            &["init", "-q", "-b", "main", seed.to_str().unwrap()],
        );
        git(&seed, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(&seed, &["push", "-q", remote.to_str().unwrap(), "main"]);
        let clone = root.path().join("clone");
        git(
            root.path(),
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        assert_eq!(verify_cycle_base(&clone, &tools), Ok(()));
        git(&clone, &["checkout", "-q", "-b", "topic"]);
        assert!(
            verify_cycle_base(&clone, &tools)
                .unwrap_err()
                .starts_with("Repeat cycles need the checked-out branch to track")
        );
        git(&clone, &["push", "-q", "-u", "origin", "topic"]);
        assert!(
            verify_cycle_base(&clone, &tools)
                .unwrap_err()
                .starts_with("Repeat cycles build on pull requests merged into origin/main")
        );
        // The seed repository has no remote at all.
        assert!(verify_cycle_base(&seed, &tools).is_err());
        // Another remote's default branch: the scripts push to origin, so it is refused,
        // and a remote name with a slash is read whole from the branch's settings.
        git(&clone, &["checkout", "-q", "main"]);
        git(
            &clone,
            &["remote", "add", "team/upstream", remote.to_str().unwrap()],
        );
        git(&clone, &["fetch", "-q", "team/upstream"]);
        git(&clone, &["remote", "set-head", "team/upstream", "main"]);
        git(
            &clone,
            &["branch", "-q", "--set-upstream-to", "team/upstream/main"],
        );
        assert_eq!(
            verify_cycle_base(&clone, &tools),
            Err("Repeat cycles push each pull request to origin, so check out a branch that tracks origin's default branch (this one tracks team/upstream/main).".into())
        );
        git(
            &clone,
            &["branch", "-q", "--set-upstream-to", "origin/main"],
        );
        assert_eq!(verify_cycle_base(&clone, &tools), Ok(()));
    }

    #[test]
    fn git_confirms_the_repository_before_a_run_is_recorded() {
        let tools = Tools {
            pwsh: PathBuf::from("pwsh"),
            git: PathBuf::from("git"),
            gh: PathBuf::from("gh"),
            path: std::env::var_os("PATH").unwrap_or_default(),
        };
        // An empty .git folder passes the quick check but is no repository.
        let fake = repository();
        assert!(inside_git_repository(fake.path()));
        let error = verify_repository(fake.path(), &tools).unwrap_err();
        assert!(
            error.ends_with("is not inside a Git repository."),
            "{error}"
        );
        let real = tempfile::Builder::new()
            .prefix("real repo ")
            .tempdir()
            .unwrap();
        let git = |directory: &Path, arguments: &[&str]| {
            let status = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=Buddy Test",
                    "-c",
                    "user.email=test@example.invalid",
                ])
                .args(arguments)
                .current_dir(directory)
                .status()
                .unwrap();
            assert!(status.success(), "git {arguments:?}");
        };
        git(real.path(), &["init", "-q"]);
        git(
            real.path(),
            &["commit", "-q", "--allow-empty", "-m", "first"],
        );
        let key = verify_repository(real.path(), &tools).unwrap();
        // A subfolder and a linked worktree are the same repository for the one-loop guard.
        let nested = real.path().join("sub folder");
        std::fs::create_dir(&nested).unwrap();
        assert_eq!(verify_repository(&nested, &tools), Ok(key.clone()));
        let linked = real.path().with_extension("worktree");
        git(
            real.path(),
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                linked.to_str().unwrap(),
            ],
        );
        assert_eq!(verify_repository(&linked, &tools), Ok(key.clone()));
        git(
            real.path(),
            &["worktree", "remove", "--force", linked.to_str().unwrap()],
        );
        let other = tempfile::tempdir().unwrap();
        git(other.path(), &["init", "-q"]);
        assert_ne!(verify_repository(other.path(), &tools), Ok(key));
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
