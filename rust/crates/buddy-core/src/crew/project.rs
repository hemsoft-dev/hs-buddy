//! Whether a folder can be a Crew project: inside a Git work tree whose `origin` is a
//! GitHub repository. SSH aliases are resolved through `ssh -G`, as Electron does.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A folder that passed [`validate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub git_root: PathBuf,
    /// `owner/repo`.
    pub slug: String,
    pub default_branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Remote {
    pub host: String,
    pub slug: String,
    pub ssh: bool,
}

/// Git from `PATH` or its usual install folders.
pub fn git() -> Result<PathBuf, String> {
    crate::ralph::launch::locate("git").ok_or_else(|| {
        "Git was not found. Install it from https://git-scm.com/downloads, then restart Buddy."
            .into()
    })
}

/// Runs a program without a console window or prompt; stdout when it succeeds.
pub(crate) fn output<I, S>(
    program: &Path,
    directory: Option<&Path>,
    arguments: I,
) -> Option<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = command(program, directory, arguments).output().ok()?;
    output.status.success().then_some(output.stdout)
}

/// The command [`output`] runs, for callers that stream its output.
pub(crate) fn command<I, S>(program: &Path, directory: Option<&Path>, arguments: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    command
}

fn text(bytes: Option<Vec<u8>>) -> Option<String> {
    bytes
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// `owner/repo` with GitHub's allowed characters.
fn valid_slug(slug: &str) -> bool {
    let mut parts = slug.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let fine = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    fine(owner) && fine(repo)
}

/// Electron's `parseGitRemote`: HTTP(S) and SSH remotes, `.git` optional.
pub(crate) fn parse_remote(url: &str) -> Option<Remote> {
    let url = url.trim();
    let finish = |host: &str, path: &str, ssh: bool| {
        let path = path.trim_end_matches('/');
        let slug = path.strip_suffix(".git").unwrap_or(path);
        (!host.is_empty() && valid_slug(slug)).then(|| Remote {
            host: host.to_ascii_lowercase(),
            slug: slug.into(),
            ssh,
        })
    };
    let lower = url.to_ascii_lowercase();
    for scheme in ["https://", "http://", "ssh://"] {
        if lower.starts_with(scheme) {
            let rest = &url[scheme.len()..];
            let (authority, path) = rest.split_once('/')?;
            let host = authority.rsplit('@').next()?;
            let host = if scheme == "ssh://" {
                host.split(':').next()?
            } else {
                host
            };
            return finish(host, path, scheme == "ssh://");
        }
    }
    // scp-like: [user@]host:owner/repo
    let (authority, path) = url.split_once(':')?;
    if authority.contains('/') || path.starts_with('/') {
        return None;
    }
    let host = authority.rsplit('@').next()?;
    finish(host, path, true)
}

pub(crate) fn github_host(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    host == "github.com" || host.ends_with(".github.com")
}

/// The real host behind an SSH alias such as `github-work`.
fn resolve_ssh_host(host: &str) -> Option<String> {
    if host.starts_with('-') {
        return None;
    }
    let ssh = crate::ralph::launch::locate("ssh")?;
    let config = String::from_utf8(output(&ssh, None, ["-G", host])?).ok()?;
    config
        .lines()
        .find_map(|line| line.strip_prefix("hostname "))
        .map(|name| name.trim().to_owned())
}

/// Git prints `C:/x/y` on Windows; show the platform's separators.
fn native_path(text: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(text.replace('/', "\\"))
    } else {
        PathBuf::from(text)
    }
}

pub fn validate(folder: &Path) -> Result<Found, String> {
    validate_with(&git()?, folder, resolve_ssh_host)
}

pub(crate) fn validate_with(
    git: &Path,
    folder: &Path,
    resolve: impl Fn(&str) -> Option<String>,
) -> Result<Found, String> {
    if !folder.is_absolute() || !folder.is_dir() {
        return Err("Choose an existing folder.".into());
    }
    let root = text(output(git, Some(folder), ["rev-parse", "--show-toplevel"]))
        .ok_or("This folder is not inside a Git repository.")?;
    let root = native_path(&root);
    let origin = text(output(git, Some(&root), ["remote", "get-url", "origin"]))
        .ok_or("This repository has no origin remote.")?;
    let remote = parse_remote(&origin).ok_or("The origin remote is not a GitHub repository.")?;
    let github = github_host(&remote.host)
        || (remote.ssh && resolve(&remote.host).is_some_and(|host| github_host(&host)));
    if !github {
        return Err("The origin remote is not a GitHub repository.".into());
    }
    let default_branch = text(output(
        git,
        Some(&root),
        ["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ))
    .map(|branch| branch.strip_prefix("origin/").unwrap_or(&branch).to_owned())
    // The checked-out branch, which also names a branch that has no commits yet.
    .or_else(|| {
        text(output(
            git,
            Some(&root),
            ["symbolic-ref", "--short", "HEAD"],
        ))
    })
    .unwrap_or_else(|| "main".into());
    Ok(Found {
        git_root: root,
        slug: remote.slug,
        default_branch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_parse_like_electron() {
        let https = parse_remote("https://github.com/hemsoft-dev/hs-buddy.git").unwrap();
        assert_eq!(
            (https.host.as_str(), https.slug.as_str(), https.ssh),
            ("github.com", "hemsoft-dev/hs-buddy", false)
        );
        let token = parse_remote("https://user:x@github.com/o/r").unwrap();
        assert_eq!(
            (token.host.as_str(), token.slug.as_str()),
            ("github.com", "o/r")
        );
        let scp = parse_remote("git@github-work:HemSoft/repo.name.git").unwrap();
        assert_eq!(
            (scp.host.as_str(), scp.slug.as_str(), scp.ssh),
            ("github-work", "HemSoft/repo.name", true)
        );
        let ssh = parse_remote("ssh://git@github.com:22/o/r.git").unwrap();
        assert_eq!(
            (ssh.host.as_str(), ssh.slug.as_str(), ssh.ssh),
            ("github.com", "o/r", true)
        );
        for bad in [
            "",
            "C:\\repos\\x",
            "/srv/git/x.git",
            "https://github.com/only-owner",
            "https://github.com/o/r/extra",
            "git@github.com:../r",
            "file:///tmp/x",
        ] {
            assert_eq!(parse_remote(bad), None, "{bad}");
        }
        assert!(github_host("GitHub.com"));
        assert!(github_host("ssh.github.com"));
        assert!(!github_host("github.com.evil.example"));
        assert!(!github_host("gitlab.com"));
    }

    fn git_or_skip() -> Option<PathBuf> {
        git().ok()
    }

    fn run(git: &Path, directory: &Path, arguments: &[&str]) {
        assert!(
            output(git, Some(directory), arguments).is_some(),
            "git {arguments:?}"
        );
    }

    #[test]
    fn folders_are_checked_for_a_github_origin() {
        let Some(git) = git_or_skip() else { return };
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("project with spaces");
        std::fs::create_dir_all(repository.join("nested")).unwrap();
        let none = |_: &str| None;

        let error = validate_with(&git, &repository, none).unwrap_err();
        assert_eq!(error, "This folder is not inside a Git repository.");
        assert_eq!(
            validate_with(&git, Path::new("relative"), none).unwrap_err(),
            "Choose an existing folder."
        );

        run(&git, &repository, &["init", "-q", "-b", "trunk"]);
        let error = validate_with(&git, &repository, none).unwrap_err();
        assert_eq!(error, "This repository has no origin remote.");

        run(
            &git,
            &repository,
            &["remote", "add", "origin", "git@gitlab.com:o/r.git"],
        );
        let error = validate_with(&git, &repository, none).unwrap_err();
        assert_eq!(error, "The origin remote is not a GitHub repository.");

        // An SSH alias counts when ssh resolves it to GitHub.
        run(
            &git,
            &repository,
            &["remote", "set-url", "origin", "git@work:Owner/Repo.git"],
        );
        assert!(validate_with(&git, &repository, none).is_err());
        let found = validate_with(&git, &repository.join("nested"), |host| {
            (host == "work").then(|| "github.com".into())
        })
        .unwrap();
        assert_eq!(found.slug, "Owner/Repo");
        assert_eq!(found.default_branch, "trunk");
        assert!(
            found.git_root.canonicalize().unwrap() == repository.canonicalize().unwrap(),
            "{:?}",
            found.git_root
        );

        run(
            &git,
            &repository,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        let found = validate_with(&git, &repository, none);
        assert!(found.is_err(), "the alias is not GitHub without ssh");
        run(
            &git,
            &repository,
            &["remote", "set-url", "origin", "https://github.com/o/r"],
        );
        assert_eq!(
            validate_with(&git, &repository, none)
                .unwrap()
                .default_branch,
            "main"
        );
    }
}
