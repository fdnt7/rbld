pub(super) mod check;
pub(super) mod reword;
pub(super) mod status;
pub(super) mod switch;
pub(super) mod update;

use std::collections::BTreeMap;

use {
    super::{Flake, activation::Asked},
    crate::util::{
        git::{self, CaughtUp},
        render::{DISALLOWED, HEADLINE, NOTE, file_path, paint, rooted},
        scope::Scope,
        terminal::{refuse, say, step, warn},
    },
    anstyle::Style,
    git2::Repository,
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

/// brings head up to the branch it follows before anything is deployed from
/// it, saying how that went, and whether the deployment is to go on
///
/// Another host may have pushed what this one is about to deploy beside, and
/// deploying without it would be deploying an older configuration than there
/// is. A fast forward made here stays, whatever becomes of the deployment: it
/// brings in what is already published rather than anything of rbld's making.
fn catch_up(repo: &Repository, workdir: &str) -> anyhow::Result<bool> {
    step("catching up with the remote");

    match git::catch_up(repo, workdir)? {
        CaughtUp::Current => {}
        CaughtUp::Forwarded { upstream, count } => {
            let commits = if count == 1 { "commit" } else { "commits" };
            say(paint(
                NOTE,
                &format!("fast forwarded to {upstream}, {count} {commits} on"),
            ));
        }
        CaughtUp::Unfetched { remote, why } => {
            warn(&format!(
                "{remote} could not be fetched from, so what is here goes as it is: {why}"
            ));
        }
        CaughtUp::Diverged {
            upstream,
            ahead,
            behind,
        } => {
            refuse(
                paint(HEADLINE, &format!("head and {upstream} have diverged")),
                paint(
                    NOTE,
                    &format!("{ahead} here and {behind} there; rebase or merge, then deploy again"),
                ),
            );
            return Ok(false);
        }
        CaughtUp::Blocked { upstream, why } => {
            refuse(
                paint(
                    HEADLINE,
                    &format!("cannot fast forward to {upstream} over the changes here"),
                ),
                paint(NOTE, &why),
            );
            return Ok(false);
        }
    }

    Ok(true)
}
