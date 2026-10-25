mod activation;
mod command;
mod flake;

use {
    clap::CommandFactory as _,
    command::{Command, check::CheckCommand},
    flake::Flake,
};

#[derive(clap::Parser)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    pub(super) fn run(self) -> anyhow::Result<std::process::ExitCode> {
        match self.command {
            Command::Switch { asked, flake } => {
                command::switch::switch(&resolve(flake), asked.activation())
            }
            Command::Update { asked, flake } => {
                command::update::update(&resolve(flake), asked.activation())
            }
            Command::Reword { flake } => command::reword::reword(&resolve(flake)),
            Command::Status { flake } => command::status::status(&resolve(flake)),
            Command::Check { command } => match command {
                CheckCommand::Scope(flake) => command::check::scope::check_scope(&resolve(flake)),
            },
            // generated from the commands as they are declared, so they cannot
            // fall behind them
            Command::Completions { shell } => {
                clap_complete::generate(
                    shell,
                    &mut Self::command(),
                    "rbld",
                    &mut std::io::stdout(),
                );
                Ok(std::process::ExitCode::SUCCESS)
            }
        }
    }
}

/// the flake a command was pointed at, or the error clap would have given had
/// the argument been required, which is what it amounts to when nothing else
/// names one
fn resolve(flake: Flake) -> String {
    flake.resolve().unwrap_or_else(|e| {
        Cli::command()
            .error(clap::error::ErrorKind::MissingRequiredArgument, e)
            .exit()
    })
}
