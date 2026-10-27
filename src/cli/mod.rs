mod activation;
mod command;
mod flake;

use {
    crate::util::terminal::warn,
    activation::Asked,
    clap::CommandFactory as _,
    command::{Command, check::CheckCommand},
    flake::Flake,
};

/// Deploy this host's NixOS configuration, committing and recording it
#[derive(clap::Parser)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    // what `rbld` alone takes, being `rbld deploy`
    #[command(flatten)]
    asked: Asked,
    #[command(flatten)]
    flake: Flake,
}

impl Cli {
    pub(super) fn run(self) -> anyhow::Result<std::process::ExitCode> {
        match self.command {
            None => command::deploy::deploy(&resolve(self.flake), self.asked.activation()),
            Some(Command::Deploy { asked, flake }) => {
                command::deploy::deploy(&resolve(flake), asked.activation())
            }
            Some(Command::Switch { asked, flake }) => {
                warn("`rbld switch` is now `rbld deploy`, or `rbld` alone");
                command::deploy::deploy(&resolve(flake), asked.activation())
            }
            Some(Command::Upgrade { asked, flake }) => {
                command::upgrade::upgrade(&resolve(flake), asked.activation())
            }
            Some(Command::Update { asked, flake }) => {
                warn("`rbld update` is now `rbld upgrade`");
                command::upgrade::upgrade(&resolve(flake), asked.activation())
            }
            Some(Command::Reword { flake }) => command::reword::reword(&resolve(flake)),
            Some(Command::Status { flake }) => command::status::status(&resolve(flake)),
            Some(Command::Check { command }) => match command {
                CheckCommand::Scope(flake) => command::check::scope::check_scope(&resolve(flake)),
            },
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
