//! a commit message written in the editor and checked by the message hook,
//! as git would on a commit, for the commands that write one for a commit
//! of their own

use {
    crate::util::{
        git,
        terminal::{ask, step, told_to_stop},
    },
    git2::Repository,
};

/// what a placeholder message is told apart by, being what no conventional
/// commit message has in it
pub const PLACEHOLDER: &str = "FIXME_";

/// what a refused message opens again under, above it, where it is seen
/// first; the reasons are left where the hook gave them
const REFUSED: &str = "# Refused by the commit-msg hook; its reasons are on the terminal.\n";

/// git's own scissors line, from which down a message is shown and not kept
const SCISSORS: &str = "# ------------------------ >8 ------------------------";

/// asks for a message in the editor, starting from `start`, with `below`
/// shown under the scissors, until the message hook takes one,
/// handing it back as the hook left it, or nothing where it was given up:
/// emptied, left as it was offered, the editor failing, or not edited again
/// once refused
///
/// A refusal stops on the terminal, where the hook has just said why, and
/// asks whether to edit the message again, rather than reopening the editor
/// at once: an editor that comes straight back reads as a second commit to
/// write, as in a rebase. Edited again, the message opens as written, with a
/// line above it saying it was refused.
pub fn write(workdir: &str, start: &str, below: &str) -> anyhow::Result<Option<String>> {
    let mut message = start.to_owned();
    let mut refused = false;

    step("writing the commit message");

    loop {
        // a stop leaves nobody to write it
        if told_to_stop() {
            return Ok(None);
        }

        let Some(edited) = git::edit(workdir, &offer(&message, refused, below))? else {
            return Ok(None);
        };
        let edited = cleaned(&edited);
        if edited.is_empty() || edited == cleaned(&message) {
            return Ok(None);
        }
        message = edited;

        let question = if message.contains(PLACEHOLDER) {
            format!("the message still has the placeholder's {PLACEHOLDER} in it; edit it again?")
        } else if let Some(taken) = git::check_message(workdir, &message)? {
            return Ok(Some(taken));
        } else {
            "the commit-msg hook refused the message; edit it again?".to_owned()
        };

        if !ask(&question)? || told_to_stop() {
            return Ok(None);
        }
        refused = true;
    }
}

/// what the editor opens on: `message`, with a line above it where it was
/// refused, and `below` under the scissors, to write the message from
/// without it being kept
fn offer(message: &str, refused: bool, below: &str) -> String {
    let above = if refused { REFUSED } else { "" };

    format!(
        "{above}{}\n\n{SCISSORS}\n{}\n",
        message.trim_end(),
        below.trim_end()
    )
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
pub fn keep(repo: &Repository, message: &str) -> anyhow::Result<String> {
    let path = repo.path().join("RBLD_EDITMSG");
    std::fs::write(&path, message)?;
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diff_sits_below_the_scissors() {
        let text = offer(
            "FIXME_type(f): FIXME_desc\n",
            false,
            "ADDED\n[A.] posy-cursors 1.6\n",
        );
        let (kept, shown) = text.split_once(SCISSORS).unwrap_or_default();

        assert_eq!(kept, "FIXME_type(f): FIXME_desc\n\n");
        assert!(shown.contains("[A.] posy-cursors 1.6"));
    }

    #[test]
    fn a_refused_message_comes_back_marked_above() {
        let text = offer(
            "fix(f): a subject well over the fifty characters allowed\n",
            true,
            "",
        );

        assert!(text.starts_with(
            "# Refused by the commit-msg hook; its reasons are on the terminal.\nfix(f): a subject well over the fifty characters allowed\n\n"
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
