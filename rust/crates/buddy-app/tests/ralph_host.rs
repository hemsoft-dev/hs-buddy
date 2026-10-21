//! The installed binary's Ralph host runs a loop detached, records its progress and
//! result, and stops exactly the loop's own process tree. A stub script stands in for the
//! agent loop; it needs PowerShell 7, which every CI runner image provides.
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use buddy_core::ralph::{
    host::spawn_host,
    launch::{Launch, Plan},
    progress::Phase,
    store::{RunState, Store},
};

fn pwsh() -> PathBuf {
    let name = if cfg!(windows) { "pwsh.exe" } else { "pwsh" };
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(name))
                .find(|path| path.is_file())
        })
        .expect("PowerShell 7 (pwsh) is required for the Ralph host tests")
}

const STUB: &str = r#"param([int]$Max = 1, [string]$Prompt, [int]$Code = 0, [string]$Beat)
Write-Host "== Iteration 1/$Max"
Write-Host "prompt: $(Get-Content -Raw $Prompt)"
[Console]::Error.WriteLine('warned')
Write-Host "`e[32mgreen`e[0m ünïcödé 資料界"
if ($Beat) {
    $child = Start-Process -FilePath (Get-Process -Id $PID).Path -PassThru -NoNewWindow -ArgumentList @(
        '-NoProfile', '-Command', "while (`$true) { Set-Content -LiteralPath '$Beat' ([DateTime]::UtcNow.Ticks); Start-Sleep -Milliseconds 100 }")
    Write-Host "child $($child.Id)"
    while ($true) { Start-Sleep -Milliseconds 200 }
}
Write-Host "== Iteration 2/$Max"
exit $Code
"#;

struct Fixture {
    _root: tempfile::TempDir,
    repository: PathBuf,
    store: Store,
    cycles: u32,
}

fn fixture() -> Fixture {
    let root = tempfile::Builder::new()
        .prefix("ralph host 界 ")
        .tempdir()
        .unwrap();
    let repository = root.path().join("repo with space");
    std::fs::create_dir_all(&repository).unwrap();
    std::fs::write(repository.join("stub.ps1"), STUB).unwrap();
    let store = Store::at(root.path().join("native-ralph")).unwrap();
    Fixture {
        repository,
        store,
        cycles: 1,
        _root: root,
    }
}

/// A failed assertion must not leave a detached loop running.
impl Drop for Fixture {
    fn drop(&mut self) {
        let active = self
            .store
            .runs()
            .into_iter()
            .filter(|run| run.state.is_active())
            .map(|run| run.request.id)
            .collect::<Vec<_>>();
        for id in active {
            let _ = self.store.request_stop(&id);
            let deadline = Instant::now() + Duration::from_secs(15);
            while self.store.run(&id).is_some_and(|run| run.state.is_active())
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

impl Fixture {
    fn start(&mut self, arguments: &[&str]) -> String {
        let script = self.repository.join("stub.ps1");
        let directory = self.repository.clone();
        let cycles = self.cycles;
        let mut extra = arguments.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let launch = Launch {
            repository: self.repository.clone(),
            prompt: "Fix the bug".into(),
            ..Launch::default()
        };
        let id = self
            .store
            .create(launch, None, |folder| {
                let mut arguments = vec![
                    "-Max".into(),
                    "2".into(),
                    "-Prompt".into(),
                    folder.join("prompt.md").to_string_lossy().into_owned(),
                ];
                arguments.append(&mut extra);
                Plan {
                    program: pwsh(),
                    script,
                    arguments,
                    directory,
                    path: std::env::var_os("PATH").unwrap(),
                    cycles,
                }
            })
            .unwrap();
        spawn_host(
            Path::new(env!("CARGO_BIN_EXE_buddy")),
            &self.store.folder(&id),
        )
        .unwrap();
        id
    }

    fn wait(&mut self, id: &str, done: impl Fn(RunState, &[String]) -> bool) -> RunState {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let state = self.store.run(id).unwrap().state;
            let (output, _) = self.store.output(id);
            if done(state, &output) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "run stayed {state:?}; output: {output:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[test]
fn a_finished_loop_records_its_output_progress_and_result() {
    let mut fixture = fixture();
    let id = fixture.start(&[]);
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Completed
    );
    let run = fixture.store.run(&id).unwrap();
    let status = run.status.unwrap();
    assert_eq!(status.exit_code, Some(0));
    assert_eq!(status.progress.phase, Phase::Iterating);
    assert_eq!(status.progress.iteration, Some(2));
    let (output, _) = fixture.store.output(&id);
    assert!(
        output.contains(&"prompt: Fix the bug".to_string()),
        "{output:?}"
    );
    assert!(
        output.contains(&"[stderr] warned".to_string()),
        "{output:?}"
    );
    assert!(
        output.contains(&"green ünïcödé 資料界".to_string()),
        "{output:?}"
    );
    // The run's folder still names it; a second host for it does nothing.
    let again = Command::new(env!("CARGO_BIN_EXE_buddy"))
        .arg("--ralph-host")
        .arg(fixture.store.folder(&id))
        .status()
        .unwrap();
    assert_eq!(again.code(), Some(4));
    assert_eq!(fixture.store.run(&id).unwrap().state, RunState::Completed);
}

#[test]
fn a_failing_loop_records_its_exit_code() {
    let mut fixture = fixture();
    let id = fixture.start(&["-Code", "3"]);
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Failed
    );
    let status = fixture.store.run(&id).unwrap().status.unwrap();
    assert_eq!(status.exit_code, Some(3));
    assert_eq!(
        status.error.as_deref(),
        Some("The loop exited with code 3.")
    );
}

fn beat(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Whether the heartbeat file changes. A process that has not written yet is given time
/// to start (PowerShell takes a second or more).
fn beating(path: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while beat(path).is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let before = beat(path);
    std::thread::sleep(Duration::from_millis(1500));
    before.is_some() && beat(path) != before
}

struct Bystander(Child);

impl Drop for Bystander {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn unrelated(beat: &Path) -> Bystander {
    Bystander(Command::new(pwsh())
        .args(["-NoProfile", "-Command"])
        .arg(format!(
            "while ($true) {{ Set-Content -LiteralPath '{}' ([DateTime]::UtcNow.Ticks); Start-Sleep -Milliseconds 100 }}",
            beat.display()
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap())
}

#[test]
fn stop_ends_the_loops_own_process_tree_and_nothing_else() {
    let mut fixture = fixture();
    let owned = fixture.repository.join("owned beat");
    let other = fixture.repository.join("other beat");
    let _bystander = unrelated(&other);
    let id = fixture.start(&["-Beat", owned.to_str().unwrap()]);
    fixture.wait(&id, |state, output| {
        state == RunState::Running && output.iter().any(|line| line.starts_with("child "))
    });
    assert!(
        beating(&owned),
        "the loop's child process should be running"
    );
    fixture.store.request_stop(&id).unwrap();
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Stopped
    );
    assert!(!beating(&owned), "the loop's child process survived Stop");
    assert!(beating(&other), "an unrelated process was stopped");
    let (output, _) = fixture.store.output(&id);
    assert_eq!(output.last().map(String::as_str), Some("[Buddy] Stopped."));
}

#[test]
fn a_dead_host_leaves_an_interrupted_run_and_takes_its_loop_along() {
    let mut fixture = fixture();
    let owned = fixture.repository.join("owned beat");
    let id = fixture.start(&["-Beat", owned.to_str().unwrap()]);
    fixture.wait(&id, |state, output| {
        state == RunState::Running && output.iter().any(|line| line.starts_with("child "))
    });
    assert!(beating(&owned));
    let host = fixture.store.run(&id).unwrap().status.unwrap().host_pid;
    let killed = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/F", "/PID", &host.to_string()])
            .stdout(Stdio::null())
            .status()
    } else {
        Command::new("kill")
            .args(["-9", &host.to_string()])
            .status()
    }
    .unwrap();
    assert!(killed.success());
    assert_eq!(
        fixture.wait(&id, |state, _| state == RunState::Interrupted),
        RunState::Interrupted
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while beating(&owned) {
        assert!(Instant::now() < deadline, "the loop outlived its host");
    }
    // An interrupted run is shown, never resumed: a new host refuses it.
    let again = Command::new(env!("CARGO_BIN_EXE_buddy"))
        .arg("--ralph-host")
        .arg(fixture.store.folder(&id))
        .status()
        .unwrap();
    assert_eq!(again.code(), Some(4));
}

fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Buddy Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(arguments)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn repeat_cycles_fast_forward_the_checkout_between_cycles() {
    let mut fixture = fixture();
    let parent = fixture.repository.parent().unwrap().to_path_buf();
    let remote = parent.join("remote.git");
    git(&parent, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    let repository = fixture.repository.clone();
    git(&repository, &["init", "-q", "-b", "main"]);
    git(&repository, &["add", "stub.ps1"]);
    git(&repository, &["commit", "-q", "-m", "stub"]);
    git(
        &repository,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repository, &["push", "-q", "-u", "origin", "main"]);
    // Work merged upstream while the first cycle ran.
    let other = parent.join("other clone");
    git(
        &parent,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    std::fs::write(other.join("merged.txt"), "merged").unwrap();
    git(&other, &["add", "merged.txt"]);
    git(&other, &["commit", "-q", "-m", "merged"]);
    git(&other, &["push", "-q", "origin", "main"]);
    fixture.cycles = 2;
    let id = fixture.start(&[]);
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Completed
    );
    assert_eq!(
        git(&repository, &["rev-parse", "HEAD"]),
        git(&other, &["rev-parse", "HEAD"])
    );
    let (output, _) = fixture.store.output(&id);
    assert!(
        output.contains(
            &"[Buddy] Fast-forwarding from origin/main before the next cycle.".to_string()
        ),
        "{output:?}"
    );
    assert!(
        output.contains(&"[Buddy] Cycle 2 of 2 started.".to_string()),
        "{output:?}"
    );
}
