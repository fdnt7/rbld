use std::process::ExitCode;

use {
    super::refuse_split,
    crate::util::{
        deploy::{self, Outcome},
        generation::Activation,
        git,
        render::{HEADLINE, NOTE, paint},
        scope::{self, Scope},
        terminal::{Interrupts, has_terminal, refuse, step, stopped, warn},
    },
    git2::Repository,
};

/// what a placeholder message is told apart by, being what no conventional
/// commit message has in it
const PLACEHOLDER: &str = "FIXME_";

/// git's own scissors line, from which down a message is shown and not kept
const SCISSORS: &str = "# ------------------------ >8 ------------------------";

/// deploys the commit checked out, or the changes on top of it
///
/// What is deployed is what a note can be written on, and that is a commit, so
/// changes are committed first, under a placeholder message, and the message
/// asked for once the deployment has happened and there is a diff to write it
/// from. A deployment that never happens takes the commit with it.
pub(in crate::cli) fn switch(flake: &str, asked: Option<Activation>) -> anyhow::Result<ExitCode> {
    // the deployment stops to be confirmed, so there is no starting one
    // without a terminal to confirm it on
    if !has_terminal() {
        return Ok(ExitCode::FAILURE);
    }

    let repo = Repository::open(flake)?;
    let workdir = git::workdir(&repo, flake);
    let host = hostname::get()?.to_string_lossy().into_owned();
    let paths = git::changed_paths(&repo)?;

    if paths.is_empty() {
        return switch_clean(&repo, workdir, &host, asked);
    }

    let scopes = scope::group(paths, &git::hosts(&repo)?);

    // a commit is about one part of the configuration, as the hook has it,
    // and this one would be no different
    let Some(scope) = scopes.keys().next().filter(|_| scopes.len() == 1) else {
        refuse_split(
            &format!(
                "cannot switch on changes spanning {} scopes; commit them one scope at a time",
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
                &format!("cannot switch on {host} to changes to {other}"),
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

    let headline = match deploy::deploy(&repo, workdir, &host, asked) {
        Ok(Outcome::Deployed(deployment)) => {
            step("writing the commit message");

            let written = git::amend(workdir, &template(scope, deployment.diff()))?;
            let commit = repo.head()?.peel_to_commit()?;

            if !written || commit.message().is_ok_and(|m| m.contains(PLACEHOLDER)) {
                warn(
                    "the commit keeps its placeholder message; write one with `git commit --amend`",
                );
            }

            deployment.record(&repo, workdir, commit.id());
            return Ok(ExitCode::SUCCESS);
        }
        Ok(Outcome::NotDeployed(headline)) => headline.to_owned(),
        Err(e) => format!("{e:#}"),
    };

    git::restore(&repo, &old_head, old_index)?;
    stopped(
        &headline,
        "the commit is undone and the changes are back as they were found",
    );

    Ok(ExitCode::FAILURE)
}

fn switch_clean(
    repo: &Repository,
    workdir: &str,
    host: &str,
    asked: Option<Activation>,
) -> anyhow::Result<ExitCode> {
    let commit = repo.head()?.peel_to_commit()?;

    // a ctrl-c is for whichever program has the terminal, and we stay to say
    // what became of it
    let _interrupts = Interrupts::held()?;

    match deploy::deploy(repo, workdir, host, asked)? {
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

/// what the editor opens on: the placeholder, then what the deployment moved,
/// below the scissors, to write the message from without it being kept; git
/// adds its own word on what the scissors do below it
fn template(scope: &Scope, diff: &str) -> String {
    let diff = diff.trim();
    let moved = if diff.is_empty() {
        "nothing was activated: the system was already this generation"
    } else {
        diff
    };

    format!("{}\n{SCISSORS}\n{moved}\n", placeholder(scope))
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

    #[test]
    fn the_diff_sits_below_the_scissors() {
        let text = template(&Scope::User("f".into()), "ADDED\n[A.] posy-cursors 1.6\n");
        let (kept, shown) = text.split_once(SCISSORS).unwrap_or_default();

        assert_eq!(kept, "FIXME_type(f): FIXME_desc\n\n");
        assert!(shown.contains("[A.] posy-cursors 1.6"));
    }
}
