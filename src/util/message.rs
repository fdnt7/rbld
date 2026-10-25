//! a commit message written in the editor and checked by the message hook,
//! as git would on a commit, for the commands that write one for a commit
//! of their own

use {
    crate::util::{
        git,
        render::{HEADLINE, paint},
        terminal::{key, say, step, told_to_stop},
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
/// shown under the scissors, until the message hook takes one, handing it
/// back as the hook left it, or nothing where it was given up: emptied, left
/// as it was offered, the editor failing, or quit once refused
///
/// A refusal stops on the terminal, where the hook has just said why, rather
/// than reopening the editor at once, which reads as a second commit to
/// write, as in a rebase. It asks as `git add -p` does, a letter for each
/// choice and none taken by default: e edits the message again, q or ctrl-c
/// gives up, doing what `quit` says, and any other key asks again, so that
/// a stray enter decides nothing. Edited again, the message opens as
/// written, with a line above it saying it was refused.
///
/// There is no keeping a message the hook refused, a placeholder included:
/// a commit message is written while the change is fresh, and one that turns
/// out wrong later is for `rbld reword`, not one never written.
pub fn write(
    workdir: &str,
    start: &str,
    below: &str,
    quit: &str,
) -> anyhow::Result<Result<String, GaveUp>> {
    let mut message = start.to_owned();
    let mut refused = false;

    step("writing the commit message");

    loop {
        // a stop leaves nobody to write it
        if told_to_stop() {
            return Ok(Err(GaveUp::Stopped));
        }

        let Some(edited) = git::edit(workdir, &offer(&message, refused, below))? else {
            return Ok(Err(GaveUp::EditorFailed));
        };
        let edited = cleaned(&edited);
        if edited.is_empty() {
            return Ok(Err(GaveUp::Emptied));
        }
        if edited == cleaned(&message) {
            return Ok(Err(GaveUp::Unchanged));
        }
        message = edited;

        let refusal = if message.contains(PLACEHOLDER) {
            format!("the message still has the placeholder's {PLACEHOLDER} in it")
        } else if let Some(taken) = git::check_message(workdir, &message)? {
            return Ok(Ok(taken));
        } else {
            "the commit-msg hook refused the message".to_owned()
        };

        say(paint(HEADLINE, &refusal));
        let edit = loop {
            let pressed = key(&format!("[e]dit it again, or [q]uit and {quit}?"))?;
            match pressed.map(choice) {
                Some(Some(edit)) => {
                    say(if edit { "edit" } else { "quit" });
                    break edit;
                }
                // asked again, on a line of its own
                Some(None) => say(""),
                // the terminal has gone
                None => {
                    say("");
                    break false;
                }
            }
        };
        if told_to_stop() {
            return Ok(Err(GaveUp::Stopped));
        }
        if !edit {
            return Ok(Err(GaveUp::NotEditedAgain));
        }
        refused = true;
    }
}

/// how a message came to be given up, which the command giving up on it
/// says, rather than only that it was
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GaveUp {
    /// saved as it was offered, or the editor quit without saving
    Unchanged,
    /// saved with nothing in it
    Emptied,
    /// the editor exited with an error, as vim's `:cq` makes it
    EditorFailed,
    /// refused, and quit at the question rather than edited again
    NotEditedAgain,
    /// rbld was told to stop while it was being written
    Stopped,
}

impl GaveUp {
    /// the headline it is given up under
    pub const fn why(self) -> &'static str {
        match self {
            Self::Unchanged => "the message was left as it was offered",
            Self::Emptied => "the message was emptied",
            Self::EditorFailed => "the editor exited with an error",
            Self::NotEditedAgain => "the refused message was not edited again",
            Self::Stopped => "rbld was told to stop",
        }
    }
}

/// what a key chooses: to edit, e; to quit, q or ctrl-c, which the raw
/// terminal hands over as a key; or nothing, any other
const fn choice(key: u8) -> Option<bool> {
    match key {
        b'e' | b'E' => Some(true),
        b'q' | b'Q' | 0x03 => Some(false),
        _ => None,
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
    fn only_e_and_q_choose() {
        assert_eq!(choice(b'e'), Some(true));
        assert_eq!(choice(b'q'), Some(false));
        // ctrl-c
        assert_eq!(choice(0x03), Some(false));
        for stray in *b"\r\n yn" {
            assert_eq!(choice(stray), None);
        }
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
