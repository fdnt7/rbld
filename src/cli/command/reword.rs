use std::process::ExitCode;

use {
    crate::util::{
        git, message, note,
        render::{HEADLINE, NOTE, paint},
        terminal::{Interrupts, done, has_terminal, hold_stops, refuse, step, stopped, warn},
    },
    git2::Repository,
};

/// rewords the commit checked out, carrying this host's note to the commit it
/// becomes
///
/// For a message that is wrong in what it says, which no hook can catch: the
/// tree stays as it is, so the note, which is of what the tree built, still
/// holds of the commit with the new message. Only a commit not yet pushed is
/// reworded, there being no telling who has pulled one that was, or which
/// host has deployed it.
pub(in crate::cli) fn reword(flake: &str) -> anyhow::Result<ExitCode> {
    // the message is written in the editor, which wants a terminal
    if !has_terminal() {
        return Ok(ExitCode::FAILURE);
    }
    hold_stops()?;

    let repo = Repository::open(flake)?;
    let workdir = git::workdir(&repo, flake);
    let host = hostname::get()?.to_string_lossy().into_owned();
    let head = repo.head()?.peel_to_commit()?;

    if git::published(&repo, head.id())? {
        refuse(
            paint(HEADLINE, "cannot reword a commit that has been pushed"),
            paint(
                NOTE,
                "rewording it would rewrite history another clone may have pulled, or another host deployed",
            ),
        );
        return Ok(ExitCode::FAILURE);
    }

    let current = head.message().unwrap_or_default().to_owned();
    let below = git::stat(workdir)?;
    let Some(message) = message::write(workdir, &current, &below, "leave the commit as it was")?
    else {
        stopped("the message was not changed", "the commit is as it was");
        return Ok(ExitCode::FAILURE);
    };

    let _interrupts = Interrupts::held()?;

    if !git::reword(workdir, &message)? {
        let kept = message::keep(&repo, &message)?;
        stopped(
            "git did not take the message",
            &format!("the commit is as it was; the message is kept in {kept}"),
        );
        return Ok(ExitCode::FAILURE);
    }

    let reworded = repo.head()?.peel_to_commit()?.id();
    let notes_ref = note::notes_ref(&host);

    match note::carry(&repo, workdir, &notes_ref, head.id(), reworded) {
        Ok(false) => done(&format!("reworded; {host} has no note on it to carry")),
        Ok(true) => match note::remote(&repo) {
            None => done(&format!("reworded, with {host}'s note carried to it")),
            Some(remote) => {
                step(&format!("pushing the note to {remote}"));
                match note::push(workdir, &remote, &notes_ref) {
                    Ok(()) => done(&format!("reworded, with {host}'s note carried to it")),
                    Err(e) => warn(&format!(
                        "reworded, with {host}'s note carried to it, but not pushed; the next deployment pushes it: {e:#}"
                    )),
                }
            }
        },
        Err(e) => warn(&format!(
            "reworded, but {host}'s note stays on the commit it was: {e:#}"
        )),
    }

    Ok(ExitCode::SUCCESS)
}
