mod cli;
mod util;

use std::process::ExitCode;

use util::terminal::fatal;

fn main() -> ExitCode {
    match cli::Cli::parse_with_config().run() {
        Ok(exit_code) => exit_code,
        Err(e) => {
            fatal(&e);
            ExitCode::FAILURE
        }
    }
}
