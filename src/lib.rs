mod atomic_file;
mod auth;
mod cli;
mod github;
mod model;
mod operational;
mod outbox;
mod plan;
mod priority;
mod priority_update;
mod ranking;
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
