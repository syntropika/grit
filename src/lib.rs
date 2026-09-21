mod auth;
mod cli;
mod dependency_events;
mod github;
mod model;
mod operational;
mod priority;
mod repository;
mod store;
mod synchronization;

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
