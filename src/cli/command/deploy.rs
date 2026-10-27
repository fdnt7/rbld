use std::process::ExitCode;

use {
    super::refuse_split,
    crate::util::{
        deploy::{self, Outcome},
        generation::Activation,
        git,
        message::{self, For, GaveUp, PLACEHOLDER},
        render::{HEADLINE, NOTE, paint},
        scope::{self, Scope},
        terminal::{Interrupts, has_terminal, hold_stops, refuse, step, stopped, warn},
    },
    git2::{Commit, Oid, Repository},
};

/// deploys the commit checked out, or the changes on top of it
///
/// What is deployed is what a note can be written on, and that is a commit, so
/// changes are committed first, under a placeholder message. The message is
/// asked for once the build shows what moves and the answer is yes, and taken
/// only once the message hook takes it, before anything is activated: a
/// deployment either goes ahead with a message the hooks take, or never
/// happens and takes the commit with it.
///
/// A message is given up by emptying it, or by leaving it as it was offered,
/// as on quitting the editor without saving.
pub(in crate::cli) fn deploy(flake: &str, asked: Option<Activation>) -> anyhow::Result<ExitCode> {
    // the deployment stops to be confirmed, so there is no starting one
    // without a terminal to confirm it on
    if !has_terminal() {
        return Ok(ExitCode::FAILURE);
    }

    // anything written from here is undone on the way out, closing the
    // terminal on it included
    hold_stops()?;

    let repo = Repository::open(flake)?;
    let workdir = git::workdir(&repo, flake);
    let host = hostname::get()?.to_string_lossy().into_owned();

    if !super::catch_up(&repo, workdir)? {
        return Ok(ExitCode::FAILURE);
    }

    let paths = git::changed_paths(&repo)?;

    if paths.is_empty() {
        return deploy_clean(&repo, workdir, &host, asked);
    }

    let scopes = scope::group(paths, &git::hosts(&repo)?);

    // a commit is about one part of the configuration, as the hook has it,
    // and this one would be no different
    let Some(scope) = scopes.keys().next().filter(|_| scopes.len() == 1) else {
        refuse_split(
            &format!(
                "cannot deploy changes spanning {} scopes; commit them one scope at a time",
                scopes.len()
            ),
            workdir,
            &scopes,
        );
        return Ok(ExitCode::FAILURE);
    };

    // another host's configuration is never built here, so deploying it
    // here would say nothing of it
    if let Scope::Host(other) = scope
        && *other != host
    {
        refuse(
            paint(
                HEADLINE,
                &format!("cannot deploy changes to {other} on {host}"),
            ),
            paint(NOTE, &format!("deploy them on {other} instead")),
        );
        return Ok(ExitCode::FAILURE);
    }

    // settled ahead of the commit, as in an update: the note is written with
    // the signature, and either failing later would leave a commit nothing
    // was built from
    repo.signature()?;

    let old_head = repo.head()?.peel_to_commit()?;
    let old_index = git::stage_tracked(&repo)?;

    // the hook is run here, on the changes, rather than on the commit, which
    // would run the message hook on a placeholder; the message hook has its
    // turn on the message that replaces it
    step("running the pre-commit hook");

    if !git::hook(workdir, "pre-commit")? {
        git::restore(&repo, &old_head, old_index)?;
        stopped(
            "the pre-commit hook refused the changes",
            "nothing is committed, and what it fixed is left in the working directory",
        );
        return Ok(ExitCode::FAILURE);
    }

    step("committing the changes");

    if !git::commit(workdir, &placeholder(scope), &[])? {
        git::restore(&repo, &old_head, old_index)?;
        stopped(
            "could not commit the changes",
            "they are back as they were found",
        );
        return Ok(ExitCode::FAILURE);
    }

    // from here to the end, a ctrl-c is for whichever program has the
    // terminal, and we stay to put things back after it
    let _interrupts = Interrupts::held()?;

    // a message kept from a switch that did not go through is written
    // from, rather than written again
    let kept = message::kept(&repo, For::Deploy);
    let start = kept.clone().unwrap_or_else(|| placeholder(scope));
    let mut message = None;
    let mut gave_up = None;
    let outcome = deploy::deploy(&repo, workdir, &host, asked, &mut |diff| {
        let moved = moves(diff);
        match message::write(workdir, &start, &moved, "undo the commit", kept.is_some())? {
            Ok(written) => message = Some(written),
            Err(why) => gave_up = Some(why),
        }
        Ok(message.is_some())
    });

    let headline = match outcome {
        Ok(Outcome::Deployed(deployment)) => {
            // every deployment that gets here was given a message first
            let message = message.unwrap_or_else(|| placeholder(scope));

            if git::amend(workdir, &message)? {
                message::forget(&repo, For::Deploy);
            } else {
                let kept = message::keep(&repo, For::Deploy, &message)?;
                warn(&format!(
                    "the commit keeps its placeholder message, as git did not take the one written; it is kept for `git commit --amend --file {kept}`"
                ));
            }

            let commit = repo.head()?.peel_to_commit()?;
            deployment.record(&repo, workdir, commit.id());
            return Ok(ExitCode::SUCCESS);
        }
        // a message given up says how, rather than only that it was; one
        // emptied is let go of, kept or not
        Ok(Outcome::NotDeployed(headline)) => {
            if gave_up == Some(GaveUp::Emptied) {
                message::forget(&repo, For::Deploy);
            }
            gave_up.map_or(headline, GaveUp::why).to_owned()
        }
        Err(e) => format!("{e:#}"),
    };

    undo(&repo, &old_head, old_index, &headline, message.as_deref())?;
    Ok(ExitCode::FAILURE)
}

/// puts the commit and the index back as they were found, saying why
/// under `headline`, and keeps `message` where one was written for nothing,
/// so as not to be written again
fn undo(
    repo: &Repository,
    old_head: &Commit<'_>,
    old_index: Oid,
    headline: &str,
    message: Option<&str>,
) -> anyhow::Result<()> {
    git::restore(repo, old_head, old_index)?;
    let undone = "the commit is undone and the changes are back as they were found";
    match message {
        Some(message) => {
            message::keep(repo, For::Deploy, message)?;
            stopped(
                headline,
                &format!(
                    "{undone}; the message written is kept, for the next deployment to open on"
                ),
            );
        }
        None => stopped(headline, undone),
    }

    Ok(())
}

pub(super) fn deploy_clean(
    repo: &Repository,
    workdir: &str,
    host: &str,
    asked: Option<Activation>,
) -> anyhow::Result<ExitCode> {
    let commit = repo.head()?.peel_to_commit()?;

    // a ctrl-c is for whichever program has the terminal, and we stay to say
    // what became of it
    let _interrupts = Interrupts::held()?;

    match deploy::deploy(repo, workdir, host, asked, &mut |_| Ok(true))? {
        Outcome::Deployed(deployment) => {
            deployment.record(repo, workdir, commit.id());
            Ok(ExitCode::SUCCESS)
        }
        Outcome::NotDeployed(headline) => {
            stopped(headline, "the system is as it was");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// the message the changes are committed under until one is written: the
/// scope filled in where the part they are in names it, the rest left
fn placeholder(scope: &Scope) -> String {
    let named = scope
        .in_message()
        .map_or_else(|| format!("{PLACEHOLDER}scope"), str::to_owned);

    format!("{PLACEHOLDER}type({named}): {PLACEHOLDER}desc\n")
}

/// what the deployment moves, as the message is written from it, below the
/// scissors
fn moves(diff: &str) -> String {
    let diff = diff.trim();
    if diff.is_empty() {
        "nothing is activated: the system already has this generation in place".to_owned()
    } else {
        diff.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_placeholder_names_what_the_part_does() {
        assert_eq!(
            placeholder(&Scope::Host("fw16".into())),
            "FIXME_type(fw16): FIXME_desc\n"
        );
        assert_eq!(
            placeholder(&Scope::User("f".into())),
            "FIXME_type(f): FIXME_desc\n"
        );
        assert_eq!(
            placeholder(&Scope::FlakeLock),
            "FIXME_type(flake.lock): FIXME_desc\n"
        );
        assert_eq!(
            placeholder(&Scope::Nixos),
            "FIXME_type(FIXME_scope): FIXME_desc\n"
        );
        assert_eq!(
            placeholder(&Scope::Repo),
            "FIXME_type(FIXME_scope): FIXME_desc\n"
        );
    }
}
