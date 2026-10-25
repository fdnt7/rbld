mod cli;
mod util;

use std::process::ExitCode;

use util::terminal::fatal;

fn main() -> ExitCode {
    // with COMPLETE set to a shell's name, rbld answers the shell's request
    // for completions, or prints the script that makes it ask, and exits
    clap_complete::CompleteEnv::with_factory(<cli::Cli as clap::CommandFactory>::command)
        .complete();

    match <cli::Cli as clap::Parser>::parse().run() {
        Ok(exit_code) => exit_code,
        Err(e) => {
            fatal(&e);
            ExitCode::FAILURE
        }
    }
}
