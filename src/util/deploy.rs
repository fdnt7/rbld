//! taking a commit to a running system: build it, show what moves, ask once,
//! activate, and leave a note saying so
//!
//! nh is given the terminal and nothing of its is read back: what the note
//! says comes from the generations themselves, and from dix's diff of them,
//! the same one nh shows when left to itself, kept without its colours.

use std::process::{Command, ExitStatus};

use {
    crate::util::{
        generation::{Activation, Decision, Generation},
        note::{self, Note},
        render::{NOTE, paint},
        terminal::{ask, done, say, step, warn},
    },
    anstream::adapter::strip_str,
    chrono::Local,
    git2::{Oid, Repository},
};

/// how a deployment ended
pub enum Outcome {
    /// the generation is in place, waiting to be recorded against the commit
    /// it came of; whatever was committed for it stays
    Deployed(Deployment),
    /// it never got that far, whether nh failed or the answer was no, as the
    /// headline says; nothing was activated, and whatever was committed for it
    /// goes
    NotDeployed(&'static str),
}

/// a deployment that has happened, with everything its note will say but the
/// commit it is written on
///
/// That is left to the caller, which may yet give the commit a message of its
/// own, and so an id of its own, before there is anything to write it on.
pub struct Deployment {
    host: String,
    /// the commit the replaced generation was deployed from
    from: Option<Oid>,
    /// how the generation was activated, or nothing where there was nothing
    /// to activate
    activation: Option<Activation>,
    reasons: Vec<String>,
    generation: Generation,
    diff: String,
}

/// deploys what `workdir` has checked out on `host`, as `asked` or else as
/// decided
///
/// An error is only ever one met before anything was activated, so a caller
/// can treat it as [`Outcome::NotDeployed`]. Once something is, what goes
/// wrong after is said here and the deployment stands.
pub fn deploy(
    repo: &Repository,
    workdir: &str,
    host: &str,
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
    let notes_ref = note::notes_ref(host);

    // a clone with none of this host's notes yet, which is to say a fresh one,
    // takes up the remote's before asking them where this deployment is from
    if let Some(remote) = note::remote(repo)
        && repo.refname_to_id(&notes_ref).is_err()
    {
        step(&format!("fetching the notes of {host} from {remote}"));

        if let Err(e) = note::adopt(repo, workdir, &remote, &notes_ref) {
            warn(&format!("the notes were not fetched: {e:#}"));
        }
    }

    let from = note::deployed_from(repo, &notes_ref, &replaced)?;
    let deployment = |activation, diff| Deployment {
        host: host.to_owned(),
        from,
        activation,
        reasons: if activation.is_some() {
            decision.reasons.clone()
        } else {
            vec![]
        },
        generation: new.clone(),
        diff,
    };

    if new == replaced {
        say(paint(
            NOTE,
            "the system is already this generation; there is nothing to activate",
        ));

        return Ok(Outcome::Deployed(deployment(None, String::new())));
    }

    step("comparing with the system it replaces");

    let diff = Command::new("dix")
        .args(["--color", "always"])
        .arg(replaced.path())
        .arg(new.path())
        .output()?;
    let diff = String::from_utf8_lossy(&diff.stdout).into_owned();
    if !diff.is_empty() {
        say(diff.trim_end_matches('\n'));
    }

    let verb = decision.activation.verb();
    for reason in &decision.reasons {
        say(paint(NOTE, &format!("{verb}, because {reason}")));
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

    Ok(Outcome::Deployed(deployment(
        Some(decision.activation),
        strip_str(&diff).to_string(),
    )))
}

fn activate(activation: Activation, generation: &Generation) -> std::io::Result<ExitStatus> {
    Command::new("nh")
        .args(["os", activation.verb(), "--diff", "never"])
        .arg(generation.path())
        .status()
}

impl Deployment {
    /// what moved between the generations, as dix put it, without colours;
    /// empty where nothing was activated
    pub fn diff(&self) -> &str {
        &self.diff
    }

    /// writes the note for this deployment on `commit`, and keeps the remote
    /// in step, saying rather than failing where either goes wrong: the system
    /// has moved by now, and a commit undone for want of a note would be the
    /// worse record of it
    pub fn record(self, repo: &Repository, workdir: &str, commit: Oid) {
        let host = self.host.clone();
        let notes_ref = note::notes_ref(&host);

        if let Err(e) = self.write(repo, &notes_ref, commit) {
            warn(&format!("the deployment was not recorded: {e:#}"));
            return;
        }

        match note::remote(repo) {
            None => done(&format!("deployed and recorded on {host}")),
            Some(remote) => {
                // a round trip to the remote, which takes as long as it takes
                step(&format!("pushing the note to {remote}"));

                match note::push(workdir, &remote, &notes_ref) {
                    Ok(()) => done(&format!("deployed and recorded on {host}")),
                    Err(e) => warn(&format!(
                        "deployed and recorded on {host}, but not pushed; the next deployment pushes it: {e:#}"
                    )),
                }
            }
        }
    }

    fn write(self, repo: &Repository, notes_ref: &str, commit: Oid) -> anyhow::Result<()> {
        let short = |oid: Oid| -> anyhow::Result<String> {
            Ok(repo
                .find_object(oid, None)?
                .short_id()?
                .as_str()
                .unwrap_or_default()
                .to_owned())
        };

        let note = Note {
            host: self.host,
            commit: short(commit)?,
            // deploying a commit again comes from itself, which says nothing
            from: self
                .from
                .filter(|&from| from != commit)
                .map(short)
                .transpose()?,
            activation: self.activation,
            reasons: self.reasons,
            generation: self.generation,
            rebuilt_at: Local::now().fixed_offset(),
            diff: self.diff,
        };

        let signature = repo.signature()?;
        repo.note(
            &signature,
            &signature,
            Some(notes_ref),
            commit,
            &note.render(),
            true,
        )?;

        Ok(())
    }
}
