mod auth;
mod cli;
mod github;
mod model;
mod operational;
mod plan;
mod priority;
mod ranking;
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
