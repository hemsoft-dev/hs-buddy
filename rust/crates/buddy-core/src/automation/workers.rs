use std::path::PathBuf;

use super::engine::{Worker, Workers};
use super::journal::{Permit, WorkerResult};
use super::models::WorkerType;
use super::{ai, exec};

pub struct NativeWorkers {
    pub executable: PathBuf,
    pub owned_directory: PathBuf,
    pub skills_directory: PathBuf,
}

impl Workers for NativeWorkers {
    fn start(&mut self, permit: Permit) -> Result<Box<dyn Worker>, WorkerResult> {
        if permit.job.worker_type == WorkerType::Exec
            && permit.job.config.command.as_deref() == Some("__copilot_snapshot__")
        {
            return super::snapshot::Spec::capture(&permit)
                .and_then(|spec| exec::Session::start_snapshot(&self.executable, spec))
                .map(|session| Box::new(session) as Box<dyn Worker>)
                .map_err(|error| WorkerResult {
                    success: false,
                    output: None,
                    error: Some(error.into()),
                    duration: 0.0,
                });
        }
        let started = match permit.job.worker_type {
            WorkerType::Exec => exec::Spec::capture(&permit.job.config)
                .and_then(|spec| exec::Session::start(&self.executable, spec)),
            WorkerType::Ai | WorkerType::Skill => {
                ai::Spec::capture(&permit, &self.owned_directory, &self.skills_directory)
                    .and_then(|spec| exec::Session::start_ai(&self.executable, spec))
            }
        };
        started
            .map(|session| Box::new(session) as Box<dyn Worker>)
            .map_err(|error| WorkerResult {
                success: false,
                output: None,
                error: Some(error.into()),
                duration: 0.0,
            })
    }
}
