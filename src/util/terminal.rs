//! rbld's own voice on the terminal, and the handing of it over to the
//! programs that borrow it

use std::{
    fmt::Display,
    io::{IsTerminal, Read, Write},
    os::fd::AsFd,
    process::Child,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use {
    crate::util::render::{DONE, ERROR, FATAL, HEADLINE, NOTE, STEP, labelled, paint},
    anstream::adapter::strip_str,
    pty_process::blocking,
    rustix::{
        process::{Pid, Signal, kill_process_group},
        termios::{OptionalActions, Termios, tcgetattr, tcsetattr},
    },
    signal_hook::{
        SigId,
        consts::SIGINT,
        flag,
        iterator::{Handle, Signals},
        low_level,
    },
    terminal_size::{Height, Width, terminal_size_of},
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

/// whether a program has left anything on the screen since the last label
///
/// what a label is set apart from is the output above it, so a program that
/// was given the terminal and left nothing on it leaves nothing to be set
/// apart from. Only what comes through [`relay`] is counted: a program left to
/// write to our own streams writes where we cannot see it, which is why a
/// [`step`] keeps its own reckoning instead of reading this one.
static SPOKEN: AtomicBool = AtomicBool::new(false);

/// whether anything of ours has been said yet, and so whether a label has
/// anything above it to be set apart from at all
///
/// [`SPOKEN`] answers this where it can, and where it cannot this stands in.
/// git is handed our own streams rather than a pty, so what it says on its way
/// out is said where there is nothing of ours to count it, and a line that
/// followed it on the reckoning of [`SPOKEN`] alone would arrive believing
/// itself the first thing on the screen. What is counted here instead is only
/// whether the run has opened its mouth yet, which is the one thing that can
/// be known of every program alike.
static ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// whether any of `output` is still on the screen once it has all been written
///
/// bytes are no measure of it. nix writes a line per input it checks and draws
/// every one of them over the last from the start of the line, then wipes the
/// line it borrowed and leaves the cursor sitting at the head of it, having
/// said a great deal and left none of it. What survives is what a newline
/// committed, and of each of those lines only what stood after the last
/// carriage return; the escapes come out last, once they have had their say
/// about what was rubbed out.
fn left_standing(output: &str) -> bool {
    output.lines().any(|line| {
        // a pty returns the carriage on every line it ends, which is not the
        // same as a line drawn over from the start
        let line = line.trim_end_matches('\r');
        let standing = line.rsplit_once('\r').map_or(line, |(_, rest)| rest);

        !strip_str(standing).to_string().trim().is_empty()
    })
}

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
    let gap = if ANNOUNCED.swap(true, Ordering::Relaxed) {
        "\n"
    } else {
        ""
    };

    anstream::eprintln!("{gap}{}", labelled(STEP, "step", message));

    // whatever the program above had to say is said; what the next one says
    // is what the line after this is set apart from
    SPOKEN.store(false, Ordering::Relaxed);
}

/// closes the command, saying what it got done
///
/// a run that ends by simply stopping leaves the reader to make out from the
/// last program's output whether it got where it was going, and that program
/// was only ever answering for its own part of it. This is the one line that
/// answers for the whole.
///
/// it is set apart from the step above it only where that step had something
/// to show for itself: nix says nothing at all for a lock file it found
/// nothing newer for, and a blank line between two lines of our own with
/// silence in between is a gap to nothing.
pub fn done(message: &str) {
    let gap = if SPOKEN.swap(false, Ordering::Relaxed) {
        "\n"
    } else {
        ""
    };

    anstream::eprintln!("{gap}{}", labelled(DONE, "done", message));
}

/// closes the command the other way, saying what stopped it and what became of
/// the work
///
/// the twin of [`done`], and set apart from what is above it the same way.
/// Where a [`refuse`] is a verdict reached by looking at something it can put
/// on the screen, this is a run that got underway and was stopped: whatever
/// stopped it has said why already, in its own voice, on its way out. What is
/// left to say is what was undone on the way here, which is the one thing no
/// other voice can answer for -- and being a sentence rather than a listing, it
/// follows on the next line rather than being set apart below.
///
/// what it is set apart from is read off [`ANNOUNCED`] rather than [`SPOKEN`],
/// a run being stopped by the very programs least likely to have been watched
/// while they said so. Only a run stopped before it started has nothing above
/// it.
pub fn stopped(headline: &str, note: &str) {
    let gap = if ANNOUNCED.load(Ordering::Relaxed) {
        "\n"
    } else {
        ""
    };

    // whatever was standing has been answered for
    SPOKEN.store(false, Ordering::Relaxed);

    anstream::eprintln!(
        "{gap}{}\n{}",
        labelled(ERROR, "error", paint(HEADLINE, headline)),
        labelled(NOTE, "note", paint(NOTE, note))
    );
}

/// opens a pty the size of the terminal, for a command to be run on
///
/// a command that draws to the width it was given gets it wrong by exactly as
/// much as the pty differs from the screen its output is bound for, so the two
/// are made to agree. Where there is no screen to measure, the pty is left at
/// its default and whatever wrapped to it wrapped to something.
pub fn pty() -> anyhow::Result<(blocking::Pty, blocking::Pts)> {
    let (pty, pts) = blocking::open()?;

    if let Some((Width(col), Height(row))) = terminal_size_of(std::io::stderr()) {
        pty.resize(pty_process::Size::new(row, col))?;
    }

    Ok((pty, pts))
}

/// carries what is typed here through to `pty`
///
/// whatever prompt sits on the other end reads from the pty, and a terminal of
/// our own is no use to it unless what that terminal hears is passed on. A
/// thread is left to it and never collected: it spends its life blocked on a
/// keystroke that may never come, and by the time the command is done there is
/// nothing left to wait for it to say.
pub fn forward_input(pty: &blocking::Pty) -> anyhow::Result<()> {
    let mut writer = std::fs::File::from(pty.as_fd().try_clone_to_owned()?);

    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut writer);
    });

    Ok(())
}

/// what becomes of a ctrl-c while a borrowed program is running, for as long
/// as this lives
///
/// a ctrl-c goes to every process in the terminal's foreground group, which is
/// a different set of people depending on how the program at hand was started,
/// and in neither case the set we want. Left to its default it ends us on the
/// spot, mid-run, with whatever we had written by then left standing and no
/// hand left to put it back.
///
/// The handler is only ever installed, never taken down again: what is undone
/// here is our own claim on the interrupt, and a ctrl-c arriving after that
/// lands on a handler with nothing left to do. Where that matters, it is for
/// the caller to say so.
pub struct Interrupts(Claim);

/// the two ways to have a ctrl-c and the way each is given up
enum Claim {
    /// caught and kept, so that a program sharing our terminal takes it alone
    Held(SigId),
    /// caught and passed on, to a program that would not otherwise hear it
    Forwarded(Handle),
}

impl Interrupts {
    /// takes the interrupt for ourselves and does nothing with it
    ///
    /// for a program given our own terminal rather than a pty: the ctrl-c is
    /// delivered to it and to us alike, and what is wanted is for it to end
    /// there. A handler is reset to its default across an exec, so the program
    /// is left to die of the interrupt exactly as it would have.
    pub fn held() -> anyhow::Result<Self> {
        // nothing reads this: the flag is only somewhere for the handler to
        // write, and the handler is only there to be something other than the
        // default
        let claim = flag::register(SIGINT, Arc::new(AtomicBool::new(false)))?;

        Ok(Self(Claim::Held(claim)))
    }

    /// takes the interrupt and passes it on to `child`'s process group
    ///
    /// for a program given a pty, and with it a session of its own: the ctrl-c
    /// is delivered to us and never reaches it, and left alone that ends the
    /// run where it stands and leaves the program to find out later, when the
    /// pty it was writing to closes under it. Passed on, the interrupt arrives
    /// where it was aimed and the program stops the way it would have stopped.
    ///
    /// It goes to the group rather than to the one process: what made the
    /// program a session leader made it a group leader too, and whatever it
    /// spawned to do the work is in there with it.
    pub fn forwarded_to(child: &Child) -> anyhow::Result<Self> {
        let group = Pid::from_raw(i32::try_from(child.id())?)
            .ok_or_else(|| anyhow::anyhow!("the program reported no process id to forward to"))?;
        let mut signals = Signals::new([SIGINT])?;
        let handle = signals.handle();

        std::thread::spawn(move || {
            for _ in &mut signals {
                // the program has gone if this fails, which is what we were
                // asking for
                let _ = kill_process_group(group, Signal::INT);
            }
        });

        Ok(Self(Claim::Forwarded(handle)))
    }
}

impl Drop for Interrupts {
    fn drop(&mut self) {
        match &self.0 {
            Claim::Held(claim) => {
                low_level::unregister(*claim);
            }
            Claim::Forwarded(handle) => handle.close(),
        }
    }
}

/// the terminal, handed over to whatever is on the other end of a pty, and put
/// back as it was found once that is done with it
pub struct RawMode(Termios);

impl RawMode {
    /// stands our terminal down for the duration
    ///
    /// the pty is a terminal in its own right and does the echoing and the
    /// line editing on the command's behalf; ours doing the same on top would
    /// echo every keystroke twice and hold each line back until it was whole,
    /// where a prompt reading single keys wants neither. Output passes through
    /// as untouched as input: the pty has already returned the carriage on
    /// every line it ended, and a terminal that went on doing so itself would
    /// only be indulging the command twice.
    ///
    /// there has to be a terminal to stand down, and it is the caller that
    /// knows what to say where there is not.
    pub fn enter() -> anyhow::Result<Self> {
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

/// passes `output` on to our own stderr as it arrives, keeping a copy
///
/// it is written through unbuffered and undecorated: a progress bar is only
/// worth anything while it is still moving, and every escape the command chose
/// is one it chose for this terminal.
pub fn relay(mut output: impl Read) -> anyhow::Result<String> {
    /// what a pty answers a read with, instead of an end of file, once nothing
    /// holds the other end open any more
    const EIO: i32 = 5;

    let mut buffer = [0; 4096];
    let mut captured = vec![];
    let mut stderr = std::io::stderr().lock();

    loop {
        let read = match output.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.raw_os_error() == Some(EIO) => break,
            Err(e) => return Err(e.into()),
        };

        stderr.write_all(&buffer[..read])?;
        stderr.flush()?;
        captured.extend_from_slice(&buffer[..read]);
    }

    let captured = String::from_utf8_lossy(&captured).into_owned();

    // a program that left something on the screen is what a label after this
    // has to be set apart from; one that rubbed out all it drew leaves nothing
    if left_standing(&captured) {
        SPOKEN.store(true, Ordering::Relaxed);
    }

    Ok(captured)
}
