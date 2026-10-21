pub(super) mod check;
pub(super) mod update;

use super::Flake;

#[derive(clap::Subcommand, Debug)]
pub(super) enum Command {
    Switch(Flake),
    Update(Flake),
    Check {
        #[command(subcommand)]
        command: check::CheckCommand,
    },
}
