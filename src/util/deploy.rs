//! taking a commit to a running system: build it, show what moves, ask once,
//! activate, and leave a note saying so
//!
//! nh is given the terminal and nothing of its is read back: what the note
//! says comes from the generations themselves, and from dix's diff of them,
//! the same one nh shows when left to itself, kept without its colours.

use std::{
    io,
    process::{Command, ExitStatus},
};

use {
    crate::util::{
        generation::{Activation, Decision, Generation},
        note::{self, Note},
        render::{NOTE, paint},
        terminal::{ask, done, note, quiet_step, say, step, told_to_stop, warn},
        units::Watched,
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

/// the headline for a deployment given up on being told to stop
const STOPPED: &str = "rbld was told to stop";

/// the headline for a deployment its caller was not ready for
const NOT_READY: &str = "the commit message was not written";

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
    decided: Activation,
    reasons: Vec<String>,
    watched: Vec<Watched>,
    generation: Generation,
    diff: String,
}

/// deploys what `workdir` has checked out on `host`, as `asked` or else as
/// decided
///
/// An error is only ever one met before anything was activated, so a caller
/// can treat it as [`Outcome::NotDeployed`]. Once something is, what goes
/// wrong after is said here and the deployment stands.
///
/// `ready` is the caller's last word before anything is activated, given what
/// moves, or where nothing does, the line saying why not: it is asked once the
/// answer is yes, or once there is found to be nothing to activate, and the
/// deployment goes no further where it says no.
pub fn deploy(
    repo: &Repository,
    workdir: &str,
    host: &str,
    asked: Option<Activation>,
    ready: &mut dyn FnMut(&str) -> anyhow::Result<bool>,
) -> anyhow::Result<Outcome> {
    if told_to_stop() {
        return Ok(Outcome::NotDeployed(STOPPED));
    }

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

    // a stop that came while nh was building, and so did not end it, is
    // heeded here, before anything is asked or activated
    if told_to_stop() {
        return Ok(Outcome::NotDeployed(STOPPED));
    }

    let new = Generation::at(&link)?;
    let decision = Decision::new(asked, &new)?;
    let replaced = decision.activation.replaces()?;
    let notes_ref = note::notes_ref(host);

    // a clone with none of this host's notes yet, which is to say a fresh one,
    // takes up the remote's before asking them where this deployment is from
    if let Some(remote) = note::remote(repo)
        && repo.refname_to_id(&notes_ref).is_err()
    {
        quiet_step(&format!("fetching the notes of {host} from {remote}"));

        if let Err(e) = note::adopt(repo, workdir, &remote, &notes_ref) {
            warn(&format!("the notes were not fetched: {e:#}"));
        }
    }

    let from = note::deployed_from(repo, &notes_ref, &replaced)?;
    let deployment = |activation, diff| Deployment {
        host: host.to_owned(),
        from,
        activation,
        decided: decision.activation,
        reasons: if activation.is_some() {
            decision.reasons.clone()
        } else {
            vec![]
        },
        watched: if activation.is_some() {
            decision.watched.clone()
        } else {
            vec![]
        },
        generation: new.clone(),
        diff,
    };

    if new == replaced {
        let source = Source::of(repo, from)?;
        let already = already(decision.activation, &source);

        // the same thing deployed again is all the closing line has to say,
        // and is left to it; changes that came to nothing are news, said here
        // where the question would have been
        if source != Source::Same {
            note(&already);
        }

        if !ready(&already)? {
            return Ok(Outcome::NotDeployed(NOT_READY));
        }
        return Ok(Outcome::Deployed(deployment(None, String::new())));
    }

    let diff = compare(&replaced, &new, &decision)?;
    let verb = decision.activation.verb();
    let yes = ask(&format!("{verb} to this generation?"))?;

    // a stop that came while the question stood outranks its answer
    if told_to_stop() {
        return Ok(Outcome::NotDeployed(STOPPED));
    }

    if !yes {
        return Ok(Outcome::NotDeployed("the system was not activated"));
    }

    if !ready(&strip_str(&diff).to_string())? {
        return Ok(Outcome::NotDeployed(NOT_READY));
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

/// shows what moves from `replaced` to `new`, as dix puts it, and why it is
/// to be activated as decided, handing back dix's diff as it was shown
fn compare(replaced: &Generation, new: &Generation, decision: &Decision) -> io::Result<String> {
    step("comparing with the system it replaces");

    let diff = Command::new("dix")
        .args(["diff", "--color", "always"])
        .arg(replaced.path())
        .arg(new.path())
        .output()?;
    let diff = String::from_utf8_lossy(&diff.stdout).into_owned();
    if !diff.is_empty() {
        say(diff.trim_end_matches('\n'));
    }

    let verb = decision.activation.verb();
    for reason in &decision.reasons {
        note(paint(NOTE, &format!("{verb}, because {reason}")));
    }
    for watched in &decision.watched {
        note(paint(
            NOTE,
            &format!(
                "{verb}ing {}s {} too, which the graphical session is made of",
                watched.change.verb(),
                watched.unit
            ),
        ));
    }

    Ok(diff)
}

/// how what is being deployed stands to what the generation in place was
/// deployed from, which is what tells two ways of having nothing to activate
/// apart: deploying the same thing again, and deploying changes that come to
/// nothing on this host
#[derive(Debug, PartialEq, Eq)]
enum Source {
    /// the very tree, whatever commit it is in: a commit given a new message
    /// is a new commit of the same tree, and the tree is all the flake sees
    Same,
    /// another tree than the one in this commit, abbreviated, so what changed
    /// since is no part of this system
    Changed(String),
    /// no telling: no note names the generation in place, or the commit the
    /// note is on is gone
    Unknown,
}

impl Source {
    /// how what `repo` has checked out stands to `from`, the commit the
    /// generation in place was deployed from
    fn of(repo: &Repository, from: Option<Oid>) -> anyhow::Result<Self> {
        let Some(from) = from.filter(|&from| repo.find_commit(from).is_ok()) else {
            return Ok(Self::Unknown);
        };

        Ok(
            if same_tree(repo, from, repo.head()?.peel_to_commit()?.id()) {
                Self::Same
            } else {
                Self::Changed(short(repo, from)?)
            },
        )
    }
}

/// whether `a` and `b` are commits of one tree, which is to say of one flake
/// as nix sees it, whatever their messages and history; not so where either
/// is gone, there being no telling
fn same_tree(repo: &Repository, a: Oid, b: Oid) -> bool {
    let tree = |oid| repo.find_commit(oid).map(|commit| commit.tree_id()).ok();

    a == b || tree(a).is_some_and(|a| Some(a) == tree(b))
}

/// what is said where the generation is already where `activation` would
/// leave it: what a boot replaces is the generation set to boot next, which a
/// deployment before this one may have left there unbooted
fn already(activation: Activation, source: &Source) -> String {
    let (is, stands) = match activation {
        Activation::Switch => ("is running", "already running"),
        Activation::Boot => ("is set to boot", "already set to boot"),
    };

    let what = match source {
        Source::Same => format!("the system was deployed from this very tree, and {is} it"),
        Source::Changed(from) => format!(
            "what changed since {from} does not reach this system: it builds the generation {stands}"
        ),
        Source::Unknown => match activation {
            Activation::Switch => "the system is already this generation".to_owned(),
            Activation::Boot => "the system is already set to boot this generation".to_owned(),
        },
    };

    format!("{what}; there is nothing to activate")
}

fn activate(activation: Activation, generation: &Generation) -> std::io::Result<ExitStatus> {
    Command::new("nh")
        .args(["os", activation.verb(), "--diff", "never"])
        .arg(generation.path())
        .status()
}

/// the closing line of a deployment that had nothing to deploy, `what` being
/// what `host` already has where `decided` would have put it: tersely, as
/// git says there is nothing to commit, there being nothing more to it
fn nothing_to_deploy(host: &str, decided: Activation, what: &str) -> String {
    let has = match decided {
        Activation::Switch => "already runs",
        Activation::Boot => "is already set to boot",
    };

    format!("nothing to deploy; {host} {has} {what}")
}

impl Deployment {
    /// writes the note for this deployment on `commit`, and keeps the remote
    /// in step, saying rather than failing where either goes wrong: the system
    /// has moved by now, and a commit undone for want of a note would be the
    /// worse record of it
    ///
    /// A deployment that activated nothing, of the tree a note already names
    /// the running generation for, is left unwritten: on the same commit it
    /// would only replace that note, the one saying what the deployment did,
    /// with one saying it did nothing, and on another commit of that tree it
    /// would say changes came to nothing where there were none. There the
    /// note is carried to the commit instead, as it is on a reword. The remote
    /// is still kept in step: a note carried, and notes an earlier push left
    /// behind, are sent now, and the closing line says which it was.
    pub fn record(self, repo: &Repository, workdir: &str, commit: Oid) {
        let host = self.host.clone();
        let notes_ref = note::notes_ref(&host);

        if self.activation.is_none()
            && let Some(from) = self.from.filter(|&from| same_tree(repo, from, commit))
        {
            let short = |oid: Oid| short(repo, oid).unwrap_or_else(|_| oid.to_string());
            let stands = if from == commit {
                nothing_to_deploy(&host, self.decided, &short(commit))
            } else {
                // the note goes with the tree, as it does on a reword, so that
                // the commit checked out is the one the deployment is told by
                let stands = nothing_to_deploy(&host, self.decided, "this tree");

                match note::carry(repo, workdir, &notes_ref, from, commit) {
                    Ok(true) => format!("{stands}, and its note is carried to {}", short(commit)),
                    Ok(false) => format!("{stands}, as {}", short(from)),
                    Err(e) => {
                        warn(&format!(
                            "the note was not carried to {}: {e:#}",
                            short(commit)
                        ));
                        format!("{stands}, as {}", short(from))
                    }
                }
            };

            // a push that failed said the next deployment would see to it, and
            // one that finds nothing to deploy is still the next: left to wait
            // for a commit, the notes stay unseen by every other host. What the
            // remote has was seen on catching up with it, so nothing is asked
            // of it here, and nothing said where it lacks nothing
            let Some(remote) =
                note::remote(repo).filter(|remote| note::unpushed(repo, remote, &notes_ref))
            else {
                done(&stands);
                return;
            };

            quiet_step(&format!("pushing the notes {remote} lacks"));

            match note::push(workdir, &remote, &notes_ref) {
                Ok(()) => done(&format!(
                    "{stands}, and the notes {remote} lacked are pushed"
                )),
                Err(e) => warn(&format!(
                    "{stands}, but {remote} lacks notes that were not pushed; the next deployment pushes them: {e:#}"
                )),
            }
            return;
        }

        // a note saying nothing was activated is a record all the same, of
        // changes that came to nothing here, but no deployment to speak of
        let recorded = if self.activation.is_some() {
            format!("deployed and recorded on {host}")
        } else {
            format!("nothing to activate; recorded on {host}")
        };

        if let Err(e) = self.write(repo, workdir, &notes_ref, commit) {
            warn(&format!("the deployment was not recorded: {e:#}"));
            return;
        }

        match note::remote(repo) {
            None => done(&recorded),
            Some(remote) => {
                // a round trip to the remote, which takes as long as it takes
                quiet_step(&format!("pushing the note to {remote}"));

                match note::push(workdir, &remote, &notes_ref) {
                    Ok(()) => done(&recorded),
                    Err(e) => warn(&format!(
                        "{recorded}, but not pushed; the next deployment pushes it: {e:#}"
                    )),
                }
            }
        }
    }

    fn write(
        self,
        repo: &Repository,
        workdir: &str,
        notes_ref: &str,
        commit: Oid,
    ) -> anyhow::Result<()> {
        let note = Note {
            host: self.host,
            commit: short(repo, commit)?,
            // deploying a commit again comes from itself, which says nothing
            from: self
                .from
                .filter(|&from| from != commit)
                .map(|from| short(repo, from))
                .transpose()?,
            activation: self.activation,
            decided: self.decided,
            reasons: self.reasons,
            watched: self.watched,
            generation: self.generation,
            rebuilt_at: Local::now().fixed_offset(),
            diff: self.diff,
        };

        let before = repo.refname_to_id(notes_ref).ok();
        let signature = repo.signature()?;
        repo.note(
            &signature,
            &signature,
            Some(notes_ref),
            commit,
            &note.render(),
            true,
        )?;

        // a note is signed as every commit here is, or it is not kept: the
        // ref goes back to where it was, and the deployment stands unrecorded
        if let Err(e) = note::sign_tip(workdir, notes_ref) {
            match before {
                Some(before) => {
                    repo.reference(notes_ref, before, true, "rbld: drop an unsigned note")?;
                }
                None => repo.find_reference(notes_ref)?.delete()?,
            }
            return Err(e.context("the note could not be signed, so it was not kept"));
        }

        Ok(())
    }
}

/// `oid` abbreviated as git would, to as few digits as keep it unambiguous
fn short(repo: &Repository, oid: Oid) -> anyhow::Result<String> {
    Ok(repo
        .find_object(oid, None)?
        .short_id()?
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// a repository in `dir` with one commit, of a file saying `text`
    fn repo(dir: &Path, text: &str) -> (Repository, Oid) {
        let repo = Repository::init(dir).expect("repo");
        let first = commit(&repo, text, &[]);
        (repo, first)
    }

    /// commits a file saying `text` on top of `parents`, moving head to it
    fn commit(repo: &Repository, text: &str, parents: &[Oid]) -> Oid {
        let workdir = repo.workdir().expect("workdir");
        std::fs::write(workdir.join("flake.nix"), text).expect("file");

        let mut index = repo.index().expect("index");
        index.add_path(Path::new("flake.nix")).expect("add");
        let tree = repo
            .find_tree(index.write_tree().expect("tree"))
            .expect("tree");
        let signature = git2::Signature::now("rbld", "rbld@localhost").expect("signature");
        let parents: Vec<_> = parents
            .iter()
            .map(|&parent| repo.find_commit(parent).expect("parent"))
            .collect();

        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            text,
            &tree,
            &parents.iter().collect::<Vec<_>>(),
        )
        .expect("commit")
    }

    #[test]
    fn the_commit_deployed_from_is_the_same_source() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (repo, first) = repo(tmp.path(), "a");

        assert_eq!(Source::of(&repo, Some(first)).ok(), Some(Source::Same));
    }

    #[test]
    fn another_commit_of_the_same_tree_is_the_same_source() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (repo, first) = repo(tmp.path(), "a");
        let changed = commit(&repo, "b", &[first]);
        let back = commit(&repo, "a", &[changed]);

        assert_ne!(first, back);
        assert_eq!(Source::of(&repo, Some(first)).ok(), Some(Source::Same));
        assert!(same_tree(&repo, first, back));
        assert!(!same_tree(&repo, first, changed));
    }

    #[test]
    fn another_tree_is_a_changed_source_named_by_where_it_changed_from() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (repo, first) = repo(tmp.path(), "a");
        commit(&repo, "b", &[first]);

        let from = short(&repo, first).expect("short");
        assert_eq!(
            Source::of(&repo, Some(first)).ok(),
            Some(Source::Changed(from))
        );
    }

    #[test]
    fn no_commit_to_go_by_is_an_unknown_source() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (repo, _) = repo(tmp.path(), "a");
        let gone = Oid::from_str("1234567890123456789012345678901234567890").expect("oid");

        assert_eq!(Source::of(&repo, None).ok(), Some(Source::Unknown));
        assert_eq!(Source::of(&repo, Some(gone)).ok(), Some(Source::Unknown));
    }

    #[test]
    fn nothing_to_activate_says_which_way() {
        assert_eq!(
            already(Activation::Switch, &Source::Same),
            "the system was deployed from this very tree, and is running it; there is nothing to activate"
        );
        assert_eq!(
            already(Activation::Boot, &Source::Changed("1f9d10e".to_owned())),
            "what changed since 1f9d10e does not reach this system: it builds the generation already set to boot; there is nothing to activate"
        );
        assert_eq!(
            already(Activation::Switch, &Source::Unknown),
            "the system is already this generation; there is nothing to activate"
        );
    }

    #[test]
    fn nothing_to_deploy_says_where_it_already_stands() {
        assert_eq!(
            nothing_to_deploy("fw16", Activation::Switch, "c47f33c"),
            "nothing to deploy; fw16 already runs c47f33c"
        );
        assert_eq!(
            nothing_to_deploy("fw16", Activation::Boot, "this tree"),
            "nothing to deploy; fw16 is already set to boot this tree"
        );
    }
}
