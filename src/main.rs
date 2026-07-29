mod cli;
mod util;

use std::process::ExitCode;

use util::render::{FATAL, paint};

fn main() -> ExitCode {
    match cli::Cli::parse_with_config().run() {
        Ok(exit_code) => exit_code,
        // `fatal` rather than `error`: a command that reports an `error` ran and
        // reached a verdict, whereas reaching here means it never got that far
        Err(e) => {
            anstream::eprintln!("{}: {e}", paint(FATAL, "fatal"));
            ExitCode::FAILURE
        }
    }
}
