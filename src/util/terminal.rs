//! rbld's own voice on the terminal, and the handing of it over to the
//! programs that borrow it

use std::{
    fmt::Display,
    io::{IsTerminal, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use {
    crate::util::render::{DONE, ERROR, FATAL, HEADLINE, NOTE, STEP, WARNING, labelled, paint},
    rustix::termios::{OptionalActions, Termios, tcgetattr, tcsetattr},
    signal_hook::{SigId, consts::SIGINT, flag, low_level},
};

/// says no, and shows what the command was looking at when it said so
///
/// a refusal is a verdict and not a mishap: the command ran, looked, and came
/// back with something to show for it, so it goes to stdout where what a
/// command has to show belongs, the listing set apart from the line that
/// introduces it.
pub fn refuse(headline: impl Display, body: impl Display) {
    // anstream drops the styling when stdout is not a terminal
    anstream::println!("\n{}\n\n{body}", labelled(ERROR, "error", headline));
}

/// says the command never got started
///
/// `fatal` rather than `error`: a command that reports an `error` ran and
/// reached a verdict, whereas reaching here means it never got that far, so
/// there is nothing to show underneath and nowhere but stderr for it to go.
pub fn fatal(error: &anyhow::Error) {
    anstream::eprintln!("{}", labelled(FATAL, "fatal", error));
}

/// whether there is a terminal to run in front of, saying what is missing
/// where there is not
///
/// a command that stops to be answered has nothing to stop for otherwise, and
/// is better off saying so before it starts than after it has done the work
/// leading up to the question.
pub fn has_terminal() -> bool {
    if std::io::stdin().is_terminal() {
        return true;
    }

    stopped(
        "cannot run without a terminal",
        "there is a question to answer on the way through, and it is read from a terminal",
    );

    false
}

/// whether anything of ours has been said yet, and so whether a label has
/// anything above it to be set apart from at all
///
/// the programs rbld runs are handed our own streams, so what they say is said
/// where there is nothing of ours to count it. What is counted instead is only
/// whether the run has opened its mouth yet, which is the one thing that can
/// be known of every program alike.
static ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// names, on a line of its own, the program about to be given the terminal
///
/// commands run back to back leave nothing to say where one ends and the next
/// begins, and a reader is left telling them apart by voice -- nh's, nix's and
/// git's, none of them ours and no two alike. Saying which is about to speak
/// costs a line and settles it, and saying it the way an `error` is said keeps
/// the one voice that is ours sounding like itself throughout.
///
/// what sets a step apart is the line above it, and the first has nothing to
/// be set apart from: it opens the output rather than breaking it up, and a
/// blank line there is a gap to nothing. Where nix wiped a progress bar off
/// the line before, the blank it left and this one both stand, which is a gap
/// wider than it need be rather than none at all.
///
/// it is written to stderr, where it can be sure of keeping its place: stdout
/// is block buffered the moment it is redirected, and a heading delivered in a
/// batch of its own after the fact heads nothing.
pub fn step(message: &str) {
    anstream::eprintln!("{}{}", gap(), labelled(STEP, "step", message));
}

/// closes the command, saying what it got done
///
/// a run that ends by simply stopping leaves the reader to make out from the
/// last program's output whether it got where it was going, and that program
/// was only ever answering for its own part of it. This is the one line that
/// answers for the whole.
pub fn done(message: &str) {
    anstream::eprintln!("{}{}", gap(), labelled(DONE, "done", message));
}

/// says something went wrong that the run carries on past, its work standing
///
/// the middle ground between [`done`] and [`stopped`]: what was asked for
/// happened, and what did not is no reason to take it back.
pub fn warn(message: &str) {
    anstream::eprintln!("{}{}", gap(), labelled(WARNING, "warning", message));
}

/// closes the command the other way, saying what stopped it and what became of
/// the work
///
/// the twin of [`done`]. Where a [`refuse`] is a verdict reached by looking at
/// something it can put on the screen, this is a run that got underway and was
/// stopped: whatever stopped it has said why already, in its own voice, on its
/// way out. What is left to say is what was undone on the way here, which is
/// the one thing no other voice can answer for -- and being a sentence rather
/// than a listing, it follows on the next line rather than being set apart
/// below.
pub fn stopped(headline: &str, note: &str) {
    anstream::eprintln!(
        "{}{}\n{}",
        gap(),
        labelled(ERROR, "error", paint(HEADLINE, headline)),
        labelled(NOTE, "note", paint(NOTE, note))
    );
}

/// the blank line that sets a label apart from what is above it, where
/// anything is; the first has nothing to be set apart from
fn gap() -> &'static str {
    if ANNOUNCED.swap(true, Ordering::Relaxed) {
        "\n"
    } else {
        ""
    }
}

/// asks `question` and reads the answer as a single key, `y` being yes and
/// anything else no
///
/// a key rather than a line, as nh asks, so a stray enter is a no and nothing
/// waits on a second keystroke. A ctrl-c is read as a key too: the terminal is
/// raw while the question stands, so it arrives as the answer rather than as
/// an interrupt, and the caller is left to undo what it did on the way here
/// the same way it would for a no.
pub fn ask(question: &str) -> anyhow::Result<bool> {
    let mut stderr = std::io::stderr();
    anstream::eprint!("{}{} [y/N] ", gap(), paint(HEADLINE, question));
    stderr.flush()?;

    let mut key = [0];
    let read = {
        let _raw = RawMode::enter()?;
        std::io::stdin().read(&mut key)?
    };

    let yes = read == 1 && key[0].eq_ignore_ascii_case(&b'y');
    anstream::eprintln!("{}", if yes { "yes" } else { "no" });

    Ok(yes)
}

/// what becomes of a ctrl-c while a borrowed program is running, for as long
/// as this lives
///
/// a ctrl-c goes to every process in the terminal's foreground group, which
/// takes in us as well as the program that has the terminal. Left to its
/// default it ends us on the spot, mid-run, with whatever we had written by
/// then left standing and no hand left to put it back.
///
/// The handler is only ever installed, never taken down again: what is undone
/// here is our own claim on the interrupt, and a ctrl-c arriving after that
/// lands on a handler with nothing left to do. Where that matters, it is for
/// the caller to say so.
pub struct Interrupts(SigId);

impl Interrupts {
    /// takes the interrupt for ourselves and does nothing with it
    ///
    /// the ctrl-c is delivered to the program and to us alike, and what is
    /// wanted is for it to end there. A handler is reset to its default across
    /// an exec, so the program is left to die of the interrupt exactly as it
    /// would have, and we to see it fail.
    pub fn held() -> anyhow::Result<Self> {
        // nothing reads this: the flag is only somewhere for the handler to
        // write, and the handler is only there to be something other than the
        // default
        let claim = flag::register(SIGINT, Arc::new(AtomicBool::new(false)))?;

        Ok(Self(claim))
    }
}

impl Drop for Interrupts {
    fn drop(&mut self) {
        low_level::unregister(self.0);
    }
}

/// the terminal, stood down for a single key to be read off it, and put back
/// as it was found once that is done
struct RawMode(Termios);

impl RawMode {
    /// no echo, no waiting for a whole line, and no signals: a key, any key,
    /// ctrl-c included, is read as it is pressed
    ///
    /// there has to be a terminal to stand down, and it is the caller that
    /// knows what to say where there is not.
    fn enter() -> anyhow::Result<Self> {
        let saved = tcgetattr(std::io::stdin())?;
        let mut raw = saved.clone();
        raw.make_raw();
        tcsetattr(std::io::stdin(), OptionalActions::Now, &raw)?;

        Ok(Self(saved))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // a terminal left raw is one the shell after us inherits, so this is
        // worth trying and not worth reporting
        let _ = tcsetattr(std::io::stdin(), OptionalActions::Now, &self.0);
    }
}
