//! taking a commit to a running system: build it, show what moves, ask once,
//! activate, and leave a note saying so
//!
//! nh and nvd are given the terminal and nothing of theirs is read back: what
//! the note says comes from the generations themselves, and from nvd asked
//! for plain text rather than drawn for a screen.

use std::process::{Command, ExitStatus};

use {
    crate::util::{
        generation::{Activation, Decision, Generation},
        note::{self, Note},
        render::{NOTE, paint},
        terminal::{ask, done, step, warn},
    },
    anstream::adapter::strip_str,
    chrono::Local,
    git2::{Oid, Repository},
};

/// how a deployment ended
pub enum Outcome {
    /// the generation is in place, and a note says so; whatever was committed
    /// for it stays
    Deployed,
    /// it never got that far, whether nh failed or the answer was no, as the
    /// headline says; nothing was activated, and whatever was committed for it
    /// goes
    NotDeployed(&'static str),
}

/// deploys `commit` from `workdir` on `host`, as `asked` or else as decided
///
/// An error is only ever one met before anything was activated, so a caller
/// can treat it as [`Outcome::NotDeployed`]. Once something is, what goes
/// wrong after is said here and the deployment stands.
pub fn deploy(
    repo: &Repository,
    workdir: &str,
    host: &str,
    commit: Oid,
    asked: Option<Activation>,
) -> anyhow::Result<Outcome> {
    step("building the system");

    // the out link keeps the build from being collected between being built
    // and being activated, and goes with the directory once the activation
    // has a profile of its own to keep it
    let dir = tempfile::Builder::new().prefix("rbld-").tempdir()?;
    let link = dir.path().join("result");
    let built = Command::new("nh")
        .args(["os", "build", "--diff", "never", "--out-link"])
        .arg(&link)
        .arg(workdir)
        .status()?;

    if !built.success() {
        return Ok(Outcome::NotDeployed("the system was not built"));
    }

    let new = Generation::at(&link)?;
    let decision = Decision::new(asked);
    let replaced = decision.activation.replaces()?;
    let from = note::deployed_from(repo, &note::notes_ref(host), &replaced)?;

    if new == replaced {
        anstream::eprintln!(
            "{}",
            paint(
                NOTE,
                "the system is already this generation; there is nothing to activate"
            )
        );
        record(
            repo,
            workdir,
            host,
            commit,
            from,
            None,
            &decision,
            &new,
            String::new(),
        );

        return Ok(Outcome::Deployed);
    }

    step("comparing with the system it replaces");

    let diff = Command::new("nvd")
        .args(["--color", "always", "diff"])
        .arg(replaced.path())
        .arg(new.path())
        .output()?;
    let diff = String::from_utf8_lossy(&diff.stdout).into_owned();
    anstream::eprint!("{diff}");

    let verb = decision.activation.verb();
    for reason in &decision.reasons {
        anstream::eprintln!("{}", paint(NOTE, &format!("{verb}, because {reason}")));
    }

    if !ask(&format!("{verb} to this generation?"))? {
        return Ok(Outcome::NotDeployed("the system was not activated"));
    }

    step(&format!("activating by {verb}"));

    let activated = activate(decision.activation, &new)?;

    if !activated.success() {
        // nh says it failed, which is not to say nothing changed: a unit that
        // would not restart fails the switch after the profile has moved
        if !decision.activation.reached(&new)? {
            return Ok(Outcome::NotDeployed("the system was not activated"));
        }

        warn("nh reported a failure, but the new generation is in place, so it is recorded");
    }

    record(
        repo,
        workdir,
        host,
        commit,
        from,
        Some(decision.activation),
        &decision,
        &new,
        strip_str(&diff).to_string(),
    );

    Ok(Outcome::Deployed)
}

fn activate(activation: Activation, generation: &Generation) -> std::io::Result<ExitStatus> {
    Command::new("nh")
        .args(["os", activation.verb(), "--diff", "never"])
        .arg(generation.path())
        .status()
}

/// writes the note for a deployment that has happened, and keeps the remote
/// in step, saying rather than failing where either goes wrong: the system has
/// moved by now, and a commit undone for want of a note would be the worse
/// record of it
#[expect(
    clippy::too_many_arguments,
    reason = "each is one fact the note records"
)]
fn record(
    repo: &Repository,
    workdir: &str,
    host: &str,
    commit: Oid,
    from: Option<Oid>,
    activation: Option<Activation>,
    decision: &Decision,
    generation: &Generation,
    diff: String,
) {
    let notes_ref = note::notes_ref(host);
    let remote = note::remote(repo);

    if let Some(remote) = &remote
        && let Err(e) = note::pull(repo, workdir, remote, &notes_ref)
    {
        warn(&format!("the notes were not fetched: {e:#}"));
    }

    let written = (|| -> anyhow::Result<()> {
        let short = |oid: Oid| -> anyhow::Result<String> {
            Ok(repo
                .find_object(oid, None)?
                .short_id()?
                .as_str()
                .unwrap_or_default()
                .to_owned())
        };

        let note = Note {
            host: host.to_owned(),
            commit: short(commit)?,
            // deploying a commit again comes from itself, which says nothing
            from: from.filter(|&from| from != commit).map(short).transpose()?,
            activation,
            reasons: if activation.is_some() {
                decision.reasons.clone()
            } else {
                vec![]
            },
            generation: generation.clone(),
            rebuilt_at: Local::now().fixed_offset(),
            diff,
        };

        let signature = repo.signature()?;
        repo.note(
            &signature,
            &signature,
            Some(&notes_ref),
            commit,
            &note.render(),
            true,
        )?;

        Ok(())
    })();

    if let Err(e) = written {
        warn(&format!("the deployment was not recorded: {e:#}"));
        return;
    }

    match remote {
        None => done(&format!("deployed and recorded on {host}")),
        Some(remote) => match note::push(workdir, &remote, &notes_ref) {
            Ok(()) => done(&format!("deployed and recorded on {host}, and pushed")),
            Err(e) => warn(&format!(
                "deployed and recorded on {host}, but not pushed; the next deployment pushes it: {e:#}"
            )),
        },
    }
}
