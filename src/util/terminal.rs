//! rbld's own voice on the terminal, and the handing of it over to the
//! programs that borrow it

use std::{
    fmt::Display,
    io::{IsTerminal, Read, Write},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

use {
    crate::util::render::{DONE, ERROR, FATAL, HEADLINE, NOTE, STEP, WARNING, labelled, paint},
    rustix::termios::{OptionalActions, Termios, tcgetattr, tcsetattr},
    signal_hook::{
        SigId,
        consts::{SIGHUP, SIGINT, SIGTERM},
        flag, low_level,
    },
};

/// says no, and shows what the command was looking at when it said so
///
/// a refusal is a verdict and not a mishap: the command ran, looked, and came
/// back with something to show for it, so it goes to stdout where what a
/// command has to show belongs, the listing set apart from the line that
/// introduces it.
pub fn refuse(headline: impl Display, body: impl Display) {
    // anstream drops the styling when stdout is not a terminal
    let _ = writeln!(
        anstream::stdout(),
        "\n{}\n\n{body}",
        labelled(ERROR, "error", headline)
    );
}

/// refuses with a sentence rather than a listing, saying why in a note on the
/// line under the headline, as [`stopped`] does
pub fn refuse_because(headline: &str, why: &str) {
    let _ = writeln!(
        anstream::stdout(),
        "\n{}\n{}",
        labelled(ERROR, "error", paint(HEADLINE, headline)),
        labelled(NOTE, "note", paint(NOTE, why))
    );
}

/// says the command never got started
///
/// `fatal` rather than `error`: a command that reports an `error` ran and
/// reached a verdict, whereas reaching here means it never got that far, so
/// there is nothing to show underneath and nowhere but stderr for it to go.
pub fn fatal(error: &anyhow::Error) {
    say(labelled(FATAL, "fatal", error));
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

/// what stands above the line about to be said, and so whether a label has
/// anything to be set apart from: one of [`NOTHING`], [`QUIET_STEP`] and
/// [`SOMETHING`]
///
/// the programs rbld runs are handed our own streams, so what they say is said
/// where there is nothing of ours to count it. What is kept instead is what
/// was last said here, and whether it left the terminal to a program that may
/// have spoken since, which is all that can be known of every program alike.
static ABOVE: AtomicU8 = AtomicU8::new(NOTHING);

/// the run has not opened its mouth yet
const NOTHING: u8 = 0;
/// a [`quiet_step`], and not a word since
const QUIET_STEP: u8 = 1;
/// anything else, a step that gave the terminal away among it, after which
/// there is no telling
const SOMETHING: u8 = 2;

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
    say(format_args!(
        "{}{}",
        step_gap(),
        labelled(STEP, "step", message)
    ));
}

/// names a step that gives the terminal to nobody: what its program says is
/// read here and not shown, so nothing comes under it but what rbld says of
/// it, which is often nothing
///
/// a step with nothing under it has nothing to be set apart from the next by,
/// and a run of them reads as the list it is. So the step after this one
/// comes on the line below, where nothing was said in between.
pub fn quiet_step(message: &str) {
    step(message);
    ABOVE.store(QUIET_STEP, Ordering::Relaxed);
}

/// closes the command, saying what it got done
///
/// a run that ends by simply stopping leaves the reader to make out from the
/// last program's output whether it got where it was going, and that program
/// was only ever answering for its own part of it. This is the one line that
/// answers for the whole.
pub fn done(message: &str) {
    say(format_args!("{}{}", gap(), labelled(DONE, "done", message)));
}

/// says something within the step above, in rbld's own voice
///
/// labelled, as every line in rbld's own words is, to be told apart from what
/// the programs it runs write on the same terminal; but with no gap above it,
/// being part of that step rather than a turning point of the run. The
/// message is left as the caller styles it: plain for what the reader needs
/// for what follows, [`NOTE`]-dimmed for an aside.
pub fn note(message: impl Display) {
    say(labelled(NOTE, "note", message));
}

/// says something went wrong that the run carries on past, its work standing
///
/// the middle ground between [`done`] and [`stopped`]: what was asked for
/// happened, and what did not is no reason to take it back.
pub fn warn(message: &str) {
    say(format_args!(
        "{}{}",
        gap(),
        labelled(WARNING, "warning", message)
    ));
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
    say(format_args!(
        "{}{}\n{}",
        gap(),
        labelled(ERROR, "error", paint(HEADLINE, headline)),
        labelled(NOTE, "note", paint(NOTE, note))
    ));
}

/// says `text` on a line of its own on stderr, or says nothing where there is
/// nowhere left to say it
///
/// a terminal that has hung up answers every write after it with an error,
/// and anstream's own macros panic on one. What is left to do by then is put
/// back what the run wrote, which matters more than telling nobody about it.
pub fn say(text: impl Display) {
    let _ = writeln!(anstream::stderr(), "{text}");
    ABOVE.store(SOMETHING, Ordering::Relaxed);
}

/// the blank line that sets a label apart from what is above it, where
/// anything is; the first has nothing to be set apart from
fn gap() -> &'static str {
    match ABOVE.swap(SOMETHING, Ordering::Relaxed) {
        NOTHING => "",
        _ => "\n",
    }
}

/// the blank line above a step, which one coming straight after a
/// [`quiet_step`] does without, as the first line does
fn step_gap() -> &'static str {
    match ABOVE.swap(SOMETHING, Ordering::Relaxed) {
        SOMETHING => "\n",
        _ => "",
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
    let yes = key(&format!("{question} [y/N]"))?.is_some_and(|key| key.eq_ignore_ascii_case(&b'y'));
    say(if yes { "yes" } else { "no" });

    Ok(yes)
}

/// puts `prompt` and reads a single key as the answer, ctrl-c included, as
/// [`ask`] does; nothing where there is no key to read, the terminal having
/// gone
pub fn key(prompt: &str) -> anyhow::Result<Option<u8>> {
    let mut stderr = anstream::stderr();
    let _ = write!(stderr, "{}{} ", gap(), paint(HEADLINE, prompt));
    let _ = stderr.flush();

    let mut key = [0];
    let read = {
        let _raw = RawMode::enter()?;
        std::io::stdin().read(&mut key)?
    };

    Ok((read == 1).then_some(key[0]))
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

/// whether the run has been told to stop since [`hold_stops`], by its terminal
/// hanging up or by being asked to terminate
static STOP: LazyLock<Arc<AtomicBool>> = LazyLock::new(Arc::default);

/// takes a hangup or a request to terminate as word to stop, rather than as
/// the end of us, from here to the end of the run
///
/// either one ends us on the spot by default, as a ctrl-c does, with whatever
/// we had written by then left standing. A hangup comes from a terminal being
/// closed, and reaches the program that has it as well as us: it dies of it,
/// and we see it fail and put things back, as after a ctrl-c. A request to
/// terminate may be for us alone, and the program carries on to the end of
/// its part; it is then for [`told_to_stop`] to keep the run from going any
/// further than that.
///
/// The claim is never given up: the run is undone on its way out, and ending
/// it outright stays with a kill.
pub fn hold_stops() -> anyhow::Result<()> {
    for signal in [SIGHUP, SIGTERM] {
        flag::register(signal, Arc::clone(&STOP))?;
    }

    Ok(())
}

/// whether the run has been told to stop, and so should go no further than
/// it has
pub fn told_to_stop() -> bool {
    STOP.load(Ordering::Relaxed)
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
