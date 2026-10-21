pub(super) mod check;
pub(super) mod update;

use super::{Flake, activation::Asked};

#[derive(clap::Subcommand, Debug)]
pub(super) enum Command {
    Switch(Flake),
    /// Update the flake's inputs, commit the lock file, and deploy it
    Update {
        #[command(flatten)]
        asked: Asked,
        #[command(flatten)]
        flake: Flake,
    },
    Check {
        #[command(subcommand)]
        command: check::CheckCommand,
    },
}
