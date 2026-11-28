mod cli;
mod context;
mod daemon;
mod mcp;
mod model;
mod net;
mod oauth;
mod patch;
mod protocol;
mod sandbox;
mod store;
mod tools;
mod tree_view;
mod tui;

type Result<T> = std::result::Result<T, String>;

#[cfg(test)]
mod formal_tests;

fn main() -> std::process::ExitCode {
    match cli::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("teamagents: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
