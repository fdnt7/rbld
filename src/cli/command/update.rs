use std::{
    path::Path,
    process::{Command, ExitCode},
};

use {
    crate::util::{
        deploy::{self, Outcome},
        generation::Activation,
        git, lock,
        render::{DISALLOWED, HEADLINE, file_path, paint, rooted},
        terminal::{Interrupts, done, has_terminal, refuse, step, stopped},
    },
    git2::Repository,
    termtree::Tree,
};

/// the one file an update has any business writing
const FLAKE_LOCK: &str = "flake.lock";

pub(in crate::cli) fn update(flake: &str, asked: Option<Activation>) -> anyhow::Result<ExitCode> {
    // asked at the end but answered for here: an update runs to a rebuild,
    // the rebuild stops to be confirmed, and there is nothing to be gained
    // from locking new inputs in and committing them on the way to a
    // question nobody is there to answer
    if !has_terminal() {
        return Ok(ExitCode::FAILURE);
    }

    let repo = Repository::open(flake)?;
    let workdir = git::workdir(&repo, flake);
    let paths = git::changed_paths(&repo)?;

    if !paths.is_empty() {
        let leaves = paths
            .iter()
            .map(|path| Tree::new(file_path(DISALLOWED, path)));

        refuse(
            paint(HEADLINE, "cannot update on a dirty working directory"),
            rooted(workdir).with_leaves(leaves),
        );

        return Ok(ExitCode::FAILURE);
    }

    step("updating flake inputs");

    let lock_path = Path::new(workdir).join(FLAKE_LOCK);
    let before = std::fs::read_to_string(&lock_path)?;

    // nix writes to the terminal as it would on its own, progress bar and
    // all: nothing is read back from it, the summary coming from the lock
    // file instead. It shares our terminal, so a ctrl-c reaches it and us
    // both; ours is held, to be here to put the lock file back after it
    let interrupts = Interrupts::held()?;
    let status = Command::new("nix")
        .args(["flake", "update", "--flake", workdir])
        .status()?;
    drop(interrupts);

    // nix has already said what went wrong, in its own words, on the way
    // through: repeating it here would only bury it. What it may have
    // written before it stopped is another matter, and goes back the way
    // everything else this command writes goes back
    if !status.success() {
        git::undo(&repo, &repo.head()?.peel_to_commit()?, FLAKE_LOCK)?;

        stopped(
            "the flake inputs were not updated",
            "the lock file is back as it was found",
        );

        return Ok(ExitCode::FAILURE);
    }

    // the tree was clean going in, so a lock file nix left untouched is
    // one it found nothing newer for
    if !repo.status_file(Path::new(FLAKE_LOCK))?.is_wt_modified() {
        done("flake inputs are already current");

        return Ok(ExitCode::SUCCESS);
    }

    let summary = lock::summarise(&before, &std::fs::read_to_string(&lock_path)?)?;
    let subject = format!("build({FLAKE_LOCK}): update");
    let commit_content = if summary.is_empty() {
        subject
    } else {
        format!("{subject}\n\n{summary}")
    };

    // a commit is deployed once per machine, each on its own schedule, so
    // the host it was deployed on is what tells the notes apart
    //
    // settled ahead of the commit rather than after it, as is the signature
    // the note is written with: either failing would otherwise stop us with
    // the lock file already committed and nothing yet built from it
    repo.signature()?;
    let host = hostname::get()?.to_string_lossy().into_owned();

    step("committing the lock file");

    let old_head = repo.head()?.peel_to_commit()?;

    // git has said what went wrong on its way out, and what it leaves
    // standing is an update nothing was built from. It goes back whole
    // rather than being left: an update either arrives at a deployed system
    // or leaves nothing behind, which is the refusal at the top of this
    // function read from the other end -- a lock file left modified would
    // meet the next run as dirt it will not start on, and one nobody asked
    // for is a poor thing to be sent to clean up by hand. It goes back to
    // the head we came in on rather than to the current one, so that a
    // commit git managed to make before failing goes with it
    if !git::commit(workdir, &commit_content, FLAKE_LOCK)? {
        git::undo(&repo, &old_head, FLAKE_LOCK)?;

        stopped(
            "could not commit the lock file",
            "the flake inputs are back as they were found",
        );

        return Ok(ExitCode::FAILURE);
    }

    // from here to the end, a ctrl-c is for whichever program has the
    // terminal, and we stay to put things back after it
    let _interrupts = Interrupts::held()?;
    let commit = repo.head()?.peel_to_commit()?;

    // a deployment that never happened, for whatever reason, takes the
    // commit standing behind it along with it, for the same reason the
    // failed commit above takes the update with it
    let headline = match deploy::deploy(&repo, workdir, &host, commit.id(), asked) {
        Ok(Outcome::Deployed) => return Ok(ExitCode::SUCCESS),
        Ok(Outcome::NotDeployed(headline)) => headline.to_owned(),
        Err(e) => format!("{e:#}"),
    };

    git::undo(&repo, &old_head, FLAKE_LOCK)?;

    stopped(
        &headline,
        "the commit is undone and the flake inputs are back as they were found",
    );

    Ok(ExitCode::FAILURE)
}
