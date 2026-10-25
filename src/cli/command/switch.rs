use std::process::ExitCode;

use {
    super::refuse_split,
    crate::util::{
        deploy::{self, Outcome},
        generation::Activation,
        git,
        render::{HEADLINE, NOTE, paint},
        scope::{self, Scope},
        terminal::{
            Interrupts, has_terminal, hold_stops, refuse, step, stopped, told_to_stop, warn,
        },
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
/// changes are committed first, under a placeholder message. The message is
/// asked for once the build shows what moves and the answer is yes, and taken
/// only once the message hook takes it, before anything is activated: a
/// deployment either goes ahead with a message the hooks take, or never
/// happens and takes the commit with it.
///
/// A message is given up by emptying it, or by leaving it as it was offered,
/// as on quitting the editor without saving.
pub(in crate::cli) fn switch(flake: &str, asked: Option<Activation>) -> anyhow::Result<ExitCode> {
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

    let mut message = None;
    let outcome = deploy::deploy(&repo, workdir, &host, asked, &mut |diff| {
        message = write_message(workdir, scope, diff)?;
        Ok(message.is_some())
    });

    let headline = match outcome {
        Ok(Outcome::Deployed(deployment)) => {
            // every deployment that gets here was given a message first
            let message = message.unwrap_or_else(|| placeholder(scope));

            if !git::amend(workdir, &message)? {
                let kept = keep_message(&repo, &message)?;
                warn(&format!(
                    "the commit keeps its placeholder message, as git did not take the one written; it is kept for `git commit --amend --file {kept}`"
                ));
            }

            let commit = repo.head()?.peel_to_commit()?;
            deployment.record(&repo, workdir, commit.id());
            return Ok(ExitCode::SUCCESS);
        }
        Ok(Outcome::NotDeployed(headline)) => headline.to_owned(),
        Err(e) => format!("{e:#}"),
    };

    git::restore(&repo, &old_head, old_index)?;
    let undone = "the commit is undone and the changes are back as they were found";
    match &message {
        // written for nothing, it is kept, so as not to be written again
        Some(message) => {
            let kept = keep_message(&repo, message)?;
            stopped(
                &headline,
                &format!("{undone}; the message written is kept in {kept}"),
            );
        }
        None => stopped(&headline, undone),
    }

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

/// asks for the message in the editor until the message hook takes one,
/// handing it back as the hook left it, or nothing where it was given up:
/// emptied, left as it was offered, or the editor failing
///
/// A refused message opens again as it was written, with what the hook said
/// of it under it, to be fixed rather than written again.
fn write_message(workdir: &str, scope: &Scope, diff: &str) -> anyhow::Result<Option<String>> {
    let mut message = placeholder(scope);
    let mut complaint: Option<String> = None;

    // said once: the editor takes the terminal straight after, so anything
    // said between one opening and the next goes unseen, and a refusal is
    // said in the editor instead, above the message it refused
    step("writing the commit message");

    loop {
        // a stop leaves nobody to write it
        if told_to_stop() {
            return Ok(None);
        }

        let Some(edited) = git::edit(workdir, &offer(&message, complaint.as_deref(), diff))? else {
            return Ok(None);
        };
        let edited = cleaned(&edited);
        if edited.is_empty() || edited == cleaned(&message) {
            return Ok(None);
        }
        message = edited;

        let refusal = if message.contains(PLACEHOLDER) {
            format!("It still has the placeholder's {PLACEHOLDER} in it.")
        } else {
            match git::check_message(workdir, &message)? {
                Ok(taken) => return Ok(Some(taken)),
                Err(said) => said,
            }
        };

        complaint = Some(refusal);
    }
}

/// what the editor opens on: `message`, with what was said against it, if
/// anything, as comments under it, and what the deployment moves below the
/// scissors, to write the message from without it being kept
fn offer(message: &str, complaint: Option<&str>, diff: &str) -> String {
    let diff = diff.trim();
    let moves = if diff.is_empty() {
        "nothing is activated: the system already has this generation in place"
    } else {
        diff
    };

    let mut text = format!("{}\n", message.trim_end());
    if let Some(complaint) = complaint {
        text.push_str("\n# This message was refused:\n");
        for line in complaint.trim().lines() {
            text.push_str(format!("# {line}").trim_end());
            text.push('\n');
        }
    }

    format!("{text}\n{SCISSORS}\n{moves}\n")
}

/// what git keeps of a message written in the editor: everything above the
/// scissors line, without comments, trailing spaces, or blank lines at either
/// end or more than one in a row, as `git stripspace --strip-comments` has it
fn cleaned(text: &str) -> String {
    let mut lines: Vec<&str> = vec![];
    for line in text.lines() {
        if line == SCISSORS {
            break;
        }
        if line.starts_with('#') {
            continue;
        }
        let line = line.trim_end();
        if line.is_empty() && lines.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }

    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

/// keeps `message` where git keeps its own, in the repository, handing back
/// where, for one written but never taken
fn keep_message(repo: &Repository, message: &str) -> anyhow::Result<String> {
    let path = repo.path().join("RBLD_EDITMSG");
    std::fs::write(&path, message)?;
    Ok(path.display().to_string())
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
        let text = offer(
            &placeholder(&Scope::User("f".into())),
            None,
            "ADDED\n[A.] posy-cursors 1.6\n",
        );
        let (kept, shown) = text.split_once(SCISSORS).unwrap_or_default();

        assert_eq!(kept, "FIXME_type(f): FIXME_desc\n\n");
        assert!(shown.contains("[A.] posy-cursors 1.6"));
    }

    #[test]
    fn a_refused_message_comes_back_with_why() {
        let text = offer(
            "fix(f): a subject well over the fifty characters allowed\n",
            Some("line 1: subject is 57 characters, over 50\n\n  more"),
            "",
        );

        assert!(text.starts_with(
            "fix(f): a subject well over the fifty characters allowed\n\n# This message was refused:\n# line 1: subject is 57 characters, over 50\n#\n#   more\n"
        ));
        assert_eq!(
            cleaned(&text),
            "fix(f): a subject well over the fifty characters allowed\n"
        );
    }

    #[test]
    fn cleaning_keeps_what_git_keeps() {
        assert_eq!(
            cleaned("\n\nfix(f): x  \n\n\n# a comment\nbody\n\n"),
            "fix(f): x\n\nbody\n"
        );
        assert_eq!(
            cleaned(&format!("fix(f): x\n{SCISSORS}\nshown only\n")),
            "fix(f): x\n"
        );
        assert_eq!(cleaned("# only a comment\n\n"), "");
    }
}
