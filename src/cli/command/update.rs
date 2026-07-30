use std::{
    io::Read,
    path::Path,
    process::{Child, Command, ExitCode},
};

use {
    crate::util::{
        git,
        render::{DISALLOWED, HEADLINE, file_path, paint, rooted},
        terminal::{
            self, RawMode, done, forward_input, has_terminal, refuse, relay, step, stopped,
        },
    },
    anstream::adapter::strip_str,
    chrono::{Local, SecondsFormat},
    git2::Repository,
    pty_process::blocking,
    rustix::termios::{OptionalActions, OutputModes, tcgetattr, tcsetattr},
    terminal_size::terminal_size_of,
    termtree::Tree,
};

/// what nix marks each input it changed with
const BULLET: char = '\u{2022}';

/// how nh names the generation being replaced, and so where its diff opens
const OUTGOING: &str = "<<<";

/// the last of the totals nh closes its diff with
const TOTAL_DIFF: &str = "DIFF:";

/// The reference used to store rebuild notes in the Git notes database.
///
/// The spelling of "rebuild" instead of "rbld" is intentional for backwards compatibility.
const REBUILD_NOTES_REF_PREFIX: &str = "refs/notes/rebuild";

/// the one file an update has any business writing
const FLAKE_LOCK: &str = "flake.lock";

impl crate::cli::Cli {
    pub(in crate::cli) fn update(self) -> anyhow::Result<ExitCode> {
        // asked at the end but answered for here: an update runs to a rebuild,
        // the rebuild stops to be confirmed, and there is nothing to be gained
        // from locking new inputs in and committing them on the way to a
        // question nobody is there to answer
        if !has_terminal() {
            return Ok(ExitCode::FAILURE);
        }

        let repo = Repository::open(&self.flake)?;
        let workdir = git::workdir(&repo, &self.flake);
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

        let (mut child, output) = spawn_update(workdir)?;
        let output = relay(output)?;

        // nix has already said what went wrong, in its own words, on the way
        // through: repeating it here would only bury it. What it may have
        // written before it stopped is another matter, and goes back the way
        // everything else this command writes goes back
        if !child.wait()?.success() {
            git::undo(&repo, &repo.head()?.peel_to_commit()?, FLAKE_LOCK)?;

            stopped(
                "the flake inputs were not updated",
                "the lock file is back as it was found",
            );

            return Ok(ExitCode::FAILURE);
        }

        let summary = summarise(&output);

        // the tree was clean going in, so a lock file nix left untouched is
        // one it found nothing newer for
        if !repo.status_file(Path::new(FLAKE_LOCK))?.is_wt_modified() {
            done("flake inputs are already current");

            return Ok(ExitCode::SUCCESS);
        }

        let commit_content = [format!("build({FLAKE_LOCK}): update"), String::new()]
            .into_iter()
            .chain(summary)
            .collect::<Vec<_>>()
            .join("\n");

        // a commit is rebuilt once per machine, each on its own schedule, so
        // the host it was rebuilt on is both what tells the notes apart and
        // what the note names as the scope of what it changed
        //
        // both are settled ahead of the commit rather than after it: a
        // signature git cannot put together, or a host that will not name
        // itself, would otherwise stop us with the lock file already committed
        // and nothing yet built from it
        let signature = repo.signature()?;
        let system = hostname::get()?.to_string_lossy().into_owned();
        let notes_ref = format!("{REBUILD_NOTES_REF_PREFIX}/{system}");

        step("committing the lock file");

        let old_head = repo.head()?.peel_to_commit()?;

        // git has said what went wrong on its way out, and what it leaves
        // standing is an update nothing was built from. It goes back whole
        // rather than being left: an update either arrives at a rebuilt system
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

        let commit = repo.head()?.peel_to_commit()?;
        let head = commit.id();

        // said ahead of the terminal being handed over rather than after,
        // while there is still something on this end to return the carriage
        step("rebuilding the system");

        // the terminal goes back to being ours before anything is made of what
        // came back over the pty, whichever way the rebuild went
        let raw = RawMode::enter()?;
        let (mut child, pty) = spawn_switch(workdir)?;
        forward_input(&pty)?;
        let output = relay(pty)?;
        let status = child.wait()?;
        drop(raw);

        // nh has said what went wrong on the way through, a refusal at its own
        // prompt included; what it has not said is that the commit standing
        // behind the rebuild goes when the rebuild does, for the same reason
        // the failed commit above takes the update with it
        if !status.success() {
            git::undo(&repo, &commit.parent(0)?, FLAKE_LOCK)?;

            stopped(
                "the system was not rebuilt",
                "the commit is undone and the flake inputs are back as they were found",
            );

            return Ok(ExitCode::FAILURE);
        }

        let note = note(&system, summarise_rebuild(&output));

        repo.note(&signature, &signature, Some(&notes_ref), head, &note, false)?;

        done(&format!("rebuilt and recorded on {system}"));

        Ok(ExitCode::SUCCESS)
    }
}

/// starts `nix flake update` on `flake`, handing back what it writes
///
/// nix draws its progress bar only for what it takes to be a terminal, so when
/// ours is one it is given a pty of the same size rather than a pipe: what
/// comes back is then byte for byte what it would have put on the screen with
/// nothing standing in between, down to the width it wrapped to. Where our own
/// output is redirected there is no screen to stand in for, and a pipe leaves
/// nix free to omit the escapes it would have omitted anyway.
fn spawn_update(flake: &str) -> anyhow::Result<(Child, Box<dyn Read>)> {
    let args = ["flake", "update", "--flake", flake];

    if terminal_size_of(std::io::stderr()).is_none() {
        // the write end lives only as long as the builder, so the read end sees
        // an end of file once nix is the last one holding it
        let (reader, writer) = std::io::pipe()?;
        let child = Command::new("nix").args(args).stderr(writer).spawn()?;

        return Ok((child, Box::new(reader)));
    }

    let (pty, pts) = terminal::pty()?;

    // nix ends a line with a bare newline and leaves the carriage return to
    // whatever it is writing to; left alone the pty would supply one, and the
    // terminal we hand the line on to would supply a second on top of it
    let mut termios = tcgetattr(&pty)?;
    termios.output_modes.remove(OutputModes::ONLCR);
    tcsetattr(&pty, OptionalActions::Now, &termios)?;

    Ok((
        blocking::Command::new("nix").args(args).spawn(pts)?,
        Box::new(pty),
    ))
}

/// what gets written to the notes ref for a rebuild `system` has just carried
/// out, `summary` being what it moved
fn note(system: &str, summary: Vec<String>) -> String {
    // RFC 3339 is ISO 8601 pinned down, and what `date --iso-8601=seconds`
    // prints; asking for the offset in full rather than `Z` keeps the note
    // reading as the clock of whoever ran the rebuild read
    let now = Local::now().to_rfc3339_opts(SecondsFormat::Secs, false);

    [
        format!("build({system}): apply {FLAKE_LOCK} update"),
        String::new(),
    ]
    .into_iter()
    .chain(summary)
    .chain([String::new(), format!("Rebuilt-at: {now}")])
    .collect::<Vec<_>>()
    .join("\n")
}

/// starts `nh os switch` on `flake`, handing back what it writes
///
/// nh splits what it has to say in two: the build log and the confirmation it
/// asks for go to stderr, the diff between the two generations to stdout. Only
/// the diff is anything to us afterwards, but catching that stream alone would
/// leave the other going straight to the terminal, and the two then race — nh
/// reaches its prompt and takes the terminal into raw mode while the diff is
/// still on its way out of the pipe, and the rest of it lands down the screen
/// in a staircase, every newline it had counted the terminal to return the
/// carriage for now arriving after that was switched off.
///
/// So both go down one pty. nh's own ordering is then the only ordering there
/// is, raw mode is something it does to a terminal of its own, and what comes
/// back is a single stream already written for a screen. It is the prompt
/// that makes the pty worth it twice over: nh reads a keystroke rather than a
/// line and refuses outright where it cannot find a terminal to read from, and
/// a pipe standing in for any of the three would be exactly that.
fn spawn_switch(flake: &str) -> anyhow::Result<(Child, blocking::Pty)> {
    let (pty, pts) = terminal::pty()?;

    // unlike nix, nh is left to end its lines the way it would on a terminal:
    // ours is handed over raw for the duration and passes them through as they
    // come rather than ending them a second time
    let child = blocking::Command::new("nh")
        .args(["os", "switch", "--ask", flake])
        .spawn(pts)?;

    Ok((child, pty))
}

/// the inputs nix reported changing, in nix's own words
///
/// nix leads with a `warning:` naming the lock file, which the subject line
/// says already, so the summary picks up at the first input instead. What
/// follows is carried over character for character: nix indents a revision to
/// sit under the one above it, having counted on the width of its own arrow,
/// and respelling that arrow would owe the line above a column back.
fn summarise(output: &str) -> Vec<String> {
    let mut summary: Vec<String> = vec![];

    for line in strip_str(output).to_string().lines() {
        // nix redraws the progress bar over itself from the start of the line,
        // so only what follows the last carriage return was ever left standing
        let line = line
            .rsplit_once('\r')
            .map_or(line, |(_, standing)| standing);

        if summary.is_empty() && !line.starts_with(BULLET) {
            continue;
        }

        summary.push(line.to_owned());
    }

    // nix wipes the line it drew the progress bar on once it has no more use
    // for it, leaving an empty one behind
    while summary.last().is_some_and(String::is_empty) {
        summary.pop();
    }

    summary
}

/// what the rebuild moved between the two generations, in nh's own words
///
/// nh names the generation it came from and the one it arrived at, lists what
/// changed between them, and closes on the totals; the note carries that block
/// whole and stops where it stops, since the prompt that follows it, and the
/// activation after that, are no part of what the rebuild moved. The styling
/// goes: a note is read back out of git, where an escape is nothing but noise.
fn summarise_rebuild(output: &str) -> Vec<String> {
    let mut summary: Vec<String> = vec![];

    for line in strip_str(output).to_string().lines() {
        // the pty returned the carriage on every line nh ended, which the
        // screen wanted and a note does not; what sits before a carriage
        // return anywhere else was drawn over where it stood
        let line = line.trim_end_matches('\r');
        let line = line
            .rsplit_once('\r')
            .map_or(line, |(_, standing)| standing);

        if summary.is_empty() && !line.starts_with(OUTGOING) {
            continue;
        }

        summary.push(line.to_owned());

        if line.starts_with(TOTAL_DIFF) {
            break;
        }
    }

    summary
}
