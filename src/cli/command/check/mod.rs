pub mod scope;

use crate::cli::Flake;

#[derive(clap::Subcommand, Debug)]
pub(in crate::cli) enum CheckCommand {
    /// Refuse a commit whose staged changes span more than one scope
    Scope(Flake),
}
