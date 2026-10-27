pub(super) mod check;
pub(super) mod deploy;
pub(super) mod reword;
pub(super) mod status;
pub(super) mod upgrade;

use std::collections::BTreeMap;

use {
    super::{Flake, activation::Asked},
    crate::util::{
        git::{self, CaughtUp},
        render::{DISALLOWED, HEADLINE, NOTE, file_path, headed, labelled, paint, rooted},
        scope::Scope,
        terminal::{refuse, say, step, warn},
    },
    anstyle::Style,
    git2::{Oid, Repository},
    termtree::Tree,
};

/// how many of the commits a catch up brought in are listed, the latest
const LISTED: usize = 10;

/// a scope's name, bold so it heads the files under it
const SCOPE: Style = Style::new().bold();

#[derive(clap::Subcommand, Debug)]
pub(super) enum Command {
    /// Deploy the commit checked out, committing any changes on top of it
    /// first; what `rbld` alone does
    Deploy {
        #[command(flatten)]
        asked: Asked,
        #[command(flatten)]
        flake: Flake,
    },
    /// `deploy` by its old name, kept a while for the hand that still types it
    #[command(hide = true)]
    Switch {
        #[command(flatten)]
        asked: Asked,
        #[command(flatten)]
        flake: Flake,
    },
    /// Update the flake's inputs, commit the lock file, and deploy it
    Upgrade {
        #[command(flatten)]
        asked: Asked,
        #[command(flatten)]
        flake: Flake,
    },
    /// `upgrade` by its old name, kept a while for the hand that still types it
    #[command(hide = true)]
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
fn catch_up(repo: &Repository, workdir: &str, host: &str) -> anyhow::Result<bool> {
    step("catching up with the remote");

    match git::catch_up(repo, workdir)? {
        CaughtUp::Current => {}
        // a note, but not dimmed as an aside is: what is built and compared
        // below is what came in as well as anything here, and a comparison
        // holding more than the commit made here is not to be taken for a
        // mistake
        CaughtUp::Forwarded { upstream, from, to } => {
            let incoming = git::incoming(repo, from, to, host)?;
            let count = incoming.len();
            let commits = if count == 1 { "commit" } else { "commits" };
            say(labelled(
                NOTE,
                "note",
                format!(
                    "fast forwarded to {upstream}: {count} {commits} came in, and what is built below includes them"
                ),
            ));
            // the tree ends its last line itself, and say ends another
            say(listing(repo, from, to, &incoming)?.to_string().trim_end());
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

/// the commits a catch up brought in, under the range they span, oldest first
/// and the latest [`LISTED`] of them, those that cannot change this host
/// receding
fn listing(
    repo: &Repository,
    from: Oid,
    to: Oid,
    incoming: &[git::Incoming],
) -> anyhow::Result<Tree<String>> {
    let short = |oid: Oid| -> anyhow::Result<String> {
        Ok(repo
            .find_object(oid, None)?
            .short_id()?
            .as_str()
            .unwrap_or_default()
            .to_owned())
    };
    let mut tree = headed(format!("{}..{}", short(from)?, short(to)?));

    let earlier = incoming.len().saturating_sub(LISTED);
    if earlier > 0 {
        tree.push(Tree::new(paint(NOTE, &format!("and {earlier} earlier"))));
    }
    for commit in &incoming[earlier..] {
        let line = format!("{} {}", commit.id, commit.summary);
        tree.push(Tree::new(match &commit.elsewhere {
            Some(other) => paint(NOTE, &format!("{line} ({other} only)")),
            None => line,
        }));
    }

    Ok(tree)
}
