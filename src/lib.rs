mod auth;
mod cli;
mod dependency_events;
mod github;
mod graph;
mod model;
mod operational;
mod priority;
mod priority_update;
mod replica_sync;
mod repository;
mod store;
mod synchronization;
mod triage;

use std::process::ExitCode;

pub fn run() -> ExitCode {
    match cli::execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
