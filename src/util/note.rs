//! the record a deployment leaves: one note per host on the commit it deployed
//!
//! A commit means the same on every host; what deploying it changed depends on
//! the host and the generation it came from. So each host keeps a notes ref of
//! its own, under which every commit it deployed carries a note saying how,
//! from what, and what moved. A note is shaped like a commit message, with the
//! facts as trailers in its last paragraph, for `git interpret-trailers` and
//! for `rbld status` to read back.

use std::{path::Path, process::Command};

use {
    crate::util::generation::{Activation, Generation},
    anyhow::{Context as _, bail},
    chrono::{DateTime, FixedOffset, SecondsFormat},
    git2::{Oid, Repository},
};

/// where the notes refs live, one under it per host
///
/// "rebuild" rather than "rbld", which the notes written before the name was
/// shortened are already under
const NOTES_REF_PREFIX: &str = "refs/notes/rebuild";

/// the host's own notes ref
pub fn notes_ref(host: &str) -> String {
    format!("{NOTES_REF_PREFIX}/{host}")
}

/// one deployment of one commit on one host
pub struct Note {
    pub host: String,
    /// the commit deployed, abbreviated
    pub commit: String,
    /// the commit whose deployment this one replaced, abbreviated, where rbld
    /// deployed it
    pub from: Option<String>,
    /// how the generation was activated, or nothing where it was already the
    /// one the activation would have left in place
    pub activation: Option<Activation>,
    pub reasons: Vec<String>,
    pub generation: Generation,
    pub rebuilt_at: DateTime<FixedOffset>,
    /// what moved between the generations, as nvd put it
    pub diff: String,
}

impl Note {
    pub fn render(&self) -> String {
        let done = self.activation.map_or_else(
            || "already on".to_owned(),
            |activation| format!("{activation} to"),
        );
        let subject = format!("{}: {done} {}", self.host, self.commit);

        let mut trailers = vec![("Host", self.host.clone())];
        trailers.extend(self.from.clone().map(|from| ("From", from)));
        trailers.push((
            "Activation",
            self.activation.map_or("none", Activation::verb).to_owned(),
        ));
        trailers.extend(
            self.reasons
                .iter()
                .map(|reason| ("Activation-reason", reason.clone())),
        );
        trailers.push(("Generation", self.generation.to_string()));
        // whole seconds, with the offset spelled out rather than `Z`, as
        // `date --iso-8601=seconds` prints it
        trailers.push((
            "Rebuilt-at",
            self.rebuilt_at.to_rfc3339_opts(SecondsFormat::Secs, false),
        ));

        let trailers = trailers
            .into_iter()
            .map(|(key, value)| format!("{key}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");

        let diff = self.diff.trim();
        if diff.is_empty() {
            format!("{subject}\n\n{trailers}\n")
        } else {
            format!("{subject}\n\n{diff}\n\n{trailers}\n")
        }
    }
}

/// the trailers of `text`, in order: its last paragraph, where every line of
/// it is one, and nothing otherwise
pub fn trailers(text: &str) -> Vec<(&str, &str)> {
    // a subject is never a trailer, however much it looks like one
    let Some((_, last)) = text.trim_end().rsplit_once("\n\n") else {
        return vec![];
    };

    let parsed: Option<Vec<_>> = last
        .lines()
        .map(|line| {
            let (key, value) = line.split_once(": ")?;
            key.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
                .then_some((key, value))
        })
        .collect();

    parsed.unwrap_or_default()
}

/// the commit whose note under `notes_ref` names `generation`, being the
/// commit that generation was deployed from; the latest such deployment, where
/// the same generation came of more than one
pub fn deployed_from(
    repo: &Repository,
    notes_ref: &str,
    generation: &Generation,
) -> anyhow::Result<Option<Oid>> {
    let notes = match repo.notes(Some(notes_ref)) {
        Ok(notes) => notes,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let wanted = generation.to_string();
    let mut latest: Option<(DateTime<FixedOffset>, Oid)> = None;

    for entry in notes {
        let (blob, commit) = entry?;
        let blob = repo.find_blob(blob)?;
        let Ok(text) = std::str::from_utf8(blob.content()) else {
            continue;
        };

        let trailers = trailers(text);
        let field = |name| {
            trailers
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| *value)
        };

        if field("Generation") != Some(&wanted) {
            continue;
        }

        // a note without a time to it still names the generation, and is
        // taken over nothing at all
        let at = field("Rebuilt-at")
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            .unwrap_or_default();

        if latest.is_none_or(|(seen, _)| at > seen) {
            latest = Some((at, commit));
        }
    }

    Ok(latest.map(|(_, commit)| commit))
}

/// the remote the notes are kept in step with: the one the branch being
/// deployed tracks, else `origin`, else none, in which case they stay here
pub fn remote(repo: &Repository) -> Option<String> {
    let tracked = repo
        .head()
        .ok()
        .and_then(|head| head.name().ok().map(str::to_owned))
        .and_then(|branch| repo.branch_upstream_remote(&branch).ok())
        .and_then(|remote| remote.as_str().ok().map(str::to_owned));

    tracked.or_else(|| {
        repo.find_remote("origin")
            .is_ok()
            .then(|| "origin".to_owned())
    })
}

/// brings `notes_ref` up to what `remote` has of it, saying whether the two
/// now agree to the extent that ours can be pushed
///
/// Only one host writes its own ref, so the remote's copy is ordinarily the
/// same as ours or behind it, behind being a push that never made it. Ahead is
/// another clone on the same host, and is taken. Each having notes the other
/// lacks has no sound way out, and is left for a person: ours stays as it is
/// and is not pushed.
pub fn pull(repo: &Repository, workdir: &str, remote: &str, notes_ref: &str) -> anyhow::Result<()> {
    let listed = git(workdir, ["ls-remote", remote, notes_ref])?;
    let Some(theirs) = listed.split_whitespace().next() else {
        // nothing there yet, which a push will see to
        return Ok(());
    };
    let theirs = Oid::from_str(theirs)?;
    let ours = repo.refname_to_id(notes_ref).ok();

    if ours == Some(theirs)
        || ours.is_some_and(|ours| repo.graph_descendant_of(ours, theirs).unwrap_or(false))
    {
        return Ok(());
    }

    git(workdir, ["fetch", "--quiet", remote, notes_ref])?;

    if let Some(ours) = ours
        && !repo.graph_descendant_of(theirs, ours)?
    {
        bail!("{notes_ref} here and on {remote} each have notes the other lacks");
    }

    repo.reference(
        notes_ref,
        theirs,
        true,
        &format!("rbld: fetched from {remote}"),
    )?;

    Ok(())
}

/// sends `notes_ref` to `remote`, which takes it only as a fast forward
pub fn push(workdir: &str, remote: &str, notes_ref: &str) -> anyhow::Result<()> {
    git(workdir, ["push", "--quiet", remote, notes_ref])?;

    Ok(())
}

/// runs git on `workdir`, handing back what it printed, and what it said on
/// stderr as the error where it failed
fn git<const N: usize>(workdir: &str, args: [&str; N]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(Path::new(workdir))
        .args(args)
        .output()
        .context("running git")?;

    if !output.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(activation: Option<Activation>, diff: &str) -> Note {
        Note {
            host: "fw16".into(),
            commit: "90b4f99".into(),
            from: Some("c5a728e".into()),
            activation,
            reasons: vec!["asked for with --boot".into()],
            generation: Generation::at(Path::new("/")).unwrap_or_else(|_| unreachable!()),
            rebuilt_at: DateTime::parse_from_rfc3339("2026-10-21T21:30:00+07:00")
                .unwrap_or_default(),
            diff: diff.into(),
        }
    }

    #[test]
    fn a_note_reads_back_as_its_trailers() {
        let diff = "<<< /nix/store/a\n>>> /nix/store/b\n\nClosure size: 1 -> 2 (1 paths added, 0 paths removed, delta +1, disk usage +1B).";
        let text = note(Some(Activation::Boot), diff).render();

        assert!(text.starts_with("fw16: boot to 90b4f99\n\n<<< /nix/store/a\n"));
        assert_eq!(
            trailers(&text),
            vec![
                ("Host", "fw16"),
                ("From", "c5a728e"),
                ("Activation", "boot"),
                ("Activation-reason", "asked for with --boot"),
                ("Generation", "/"),
                ("Rebuilt-at", "2026-10-21T21:30:00+07:00"),
            ]
        );
    }

    #[test]
    fn nothing_to_activate_says_so() {
        let text = note(None, "").render();

        assert!(text.starts_with("fw16: already on 90b4f99\n\nHost: fw16\n"));
        assert!(trailers(&text).contains(&("Activation", "none")));
    }

    #[test]
    fn a_closing_paragraph_of_prose_has_no_trailers() {
        assert_eq!(
            trailers("fw16: switch\n\nClosure size: 1 -> 2 (delta +1)."),
            Vec::<(&str, &str)>::new()
        );
        assert_eq!(
            trailers("subject\n\nHost: fw16\nnot a trailer"),
            Vec::<(&str, &str)>::new()
        );
    }
}
