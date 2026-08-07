mod atomic_file;
mod auth;
mod cli;
mod dependency_update;
mod draft_identity;
mod github;
mod issue_create;
mod model;
mod operation_marker;
mod operational;
mod outbox;
mod priority;
mod priority_update;
mod ranking;
mod reconciliation;
mod replica_sync;
mod repository;
mod store;
mod working_graph;

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
