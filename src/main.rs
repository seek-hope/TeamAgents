mod cli;
mod daemon;
mod model;
mod net;
mod oauth;
mod sandbox;
mod store;
mod tools;
mod tree_view;

type Result<T> = std::result::Result<T, String>;

fn main() -> std::process::ExitCode {
    match cli::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("teamagents: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
