pub(super) mod check;
pub(super) mod switch;
pub(super) mod update;

use {
    super::{Flake, activation::Asked},
    crate::util::{
        render::{DISALLOWED, HEADLINE, file_path, paint, rooted},
        terminal::refuse,
    },
    termtree::Tree,
};

#[derive(clap::Subcommand, Debug)]
pub(super) enum Command {
    /// Deploy the commit checked out
    Switch {
        #[command(flatten)]
        asked: Asked,
        #[command(flatten)]
        flake: Flake,
    },
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

/// refuses to start on a tree with changes in it, listing them under `workdir`
fn refuse_dirty(headline: &str, workdir: &str, paths: &[String]) {
    let leaves = paths
        .iter()
        .map(|path| Tree::new(file_path(DISALLOWED, path)));

    refuse(
        paint(HEADLINE, headline),
        rooted(workdir).with_leaves(leaves),
    );
}
