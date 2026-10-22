//! Ralph loops: validated launches of the bundled agent-loop scripts, each owned by a
//! detached host process so a loop survives the app closing and is never started twice.
//! See `docs/NATIVE-RALPH.md`.
pub mod host;
pub mod launch;
#[cfg(unix)]
mod marked;
pub mod progress;
pub mod scripts;
pub mod store;

use std::path::Path;

pub use host::{run_ralph_exec, run_ralph_host};
use launch::{Launch, Plan};
use scripts::Scripts;
use store::Store;

/// Validates `launch`, checks the external tools, records the run and starts its host.
/// Returns the new run's id.
pub fn start(
    store: &mut Store,
    executable: &Path,
    launch: Launch,
    retry_of: Option<String>,
) -> Result<String, String> {
    let scripts = Scripts::prepare(store.root())?;
    let catalog = scripts.catalog()?;
    let (script, declared) = launch.validate(&catalog, &scripts)?;
    // Recorded with Default spelled out, so Retry repeats the same choices.
    let launch = launch.resolved(&catalog);
    let tools = launch::discover_installed()?;
    let repository_key = launch::verify_repository(&launch.repository, &tools)?;
    let base = if launch.script.uses_cycles() && launch.cycles > 1 {
        Some(launch::verify_cycle_base(&launch.repository, &tools)?)
    } else {
        None
    };
    let cycles = if launch.script.uses_cycles() {
        launch.cycles
    } else {
        1
    };
    let prompt = !launch.prompt.trim().is_empty();
    let id = store.create(launch.clone(), retry_of, Some(repository_key), |folder| {
        Plan {
            program: tools.pwsh.clone(),
            git: tools.git.clone(),
            gh: tools.gh.clone(),
            account: launch.account.clone(),
            base: base.clone(),
            script,
            arguments: launch.arguments(
                &declared,
                prompt.then(|| folder.join(store::PROMPT)).as_deref(),
            ),
            directory: launch.repository.clone(),
            path: tools.path.clone(),
            cycles,
        }
    })?;
    if let Err(error) = host::spawn_host(executable, &store.folder(&id)) {
        store.discard_unstarted(&id);
        return Err(error);
    }
    // Keep the newest runs; each launch is the point where the list grows.
    store.prune();
    Ok(id)
}

/// Starts a finished, failed, stopped or interrupted run again as a new run.
pub fn retry(store: &mut Store, executable: &Path, id: &str) -> Result<String, String> {
    let run = store.run(id).ok_or("This run no longer exists.")?;
    if run.state.is_active() {
        return Err("This run is still active. Stop it before retrying.".into());
    }
    start(store, executable, run.request.launch, Some(id.to_owned()))
}
