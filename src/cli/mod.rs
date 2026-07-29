mod command;
mod config;

use {command::Command, config::FileConfig};

#[derive(clap::Parser)]
pub struct Cli {
    #[arg(long, value_name = "PATH", value_parser = FileConfig::load)]
    config: Option<FileConfig>,
    #[command(subcommand)]
    command: Command,
    flake: String,
}

impl Cli {
    pub(super) fn run(self) -> anyhow::Result<std::process::ExitCode> {
        match &self.command {
            Command::Switch => todo!(),
            Command::Update => todo!(),
            Command::Check { command } => match command {
                command::check::CheckCommand::Boundaries => self.check_boundaries(),
            },
        }
    }
}
