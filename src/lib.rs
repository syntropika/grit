mod auth;
mod cli;
mod github;
mod model;
mod operational;
mod priority;
mod priority_update;
mod replica_sync;
mod repository;
mod store;

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
