pub(super) mod check;
pub(super) mod reword;
pub(super) mod status;
pub(super) mod switch;
pub(super) mod update;

use std::collections::BTreeMap;

use {
    super::{Flake, activation::Asked},
    crate::util::{
        render::{DISALLOWED, HEADLINE, NOTE, file_path, paint, rooted},
        scope::Scope,
        terminal::refuse,
    },
    anstyle::Style,
    termtree::Tree,
};

/// a scope's name, bold so it heads the files under it
const SCOPE: Style = Style::new().bold();

#[derive(clap::Subcommand, Debug)]
pub(super) enum Command {
    /// Deploy the commit checked out, committing any changes on top of it first
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
    /// Reword the commit checked out, carrying its deployment note along
    Reword {
        #[command(flatten)]
        flake: Flake,
    },
    /// Show each host's latest deployment, fetching the notes first
    Status {
        #[command(flatten)]
        flake: Flake,
    },
    /// Check a commit about to be made, as the git hooks do
    Check {
        #[command(subcommand)]
        command: check::CheckCommand,
    },
    /// Print the completions for a shell, which the package already installs
    /// for bash, fish and zsh
    Completions { shell: clap_complete::Shell },
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

/// refuses changes that span more than one scope, listing them under
/// `workdir` by scope, which is the split to make
fn refuse_split(headline: &str, workdir: &str, scopes: &BTreeMap<Scope, Vec<String>>) {
    let mut tree = rooted(workdir);
    for (scope, paths) in scopes {
        // the kind is said in words as well as set apart, since prek
        // captures the hook and strips the styling when it does
        let heading = format!(
            "{} {}",
            paint(SCOPE, &scope.to_string()),
            paint(NOTE, &format!("({})", scope.kind()))
        );
        let leaves = paths
            .iter()
            .map(|path| Tree::new(file_path(Style::new(), path)));

        tree.push(Tree::new(heading).with_leaves(leaves));
    }

    refuse(paint(HEADLINE, headline), tree);
}
