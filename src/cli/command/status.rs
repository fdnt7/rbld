use std::{io::Write as _, process::ExitCode};

use {
    super::SCOPE,
    crate::util::{
        generation::Generation,
        git,
        note::{self, Record},
        render::{NOTE, headed, paint},
        terminal::{say, step, warn},
    },
    chrono::{DateTime, FixedOffset, Local, TimeDelta},
    git2::{Oid, Repository},
    termtree::Tree,
};

/// says, for every host with a deployment recorded, the latest: what was
/// deployed, when, how, and where it stands against the commit checked out
///
/// The notes are fetched first, for the other hosts' to be as the remote has
/// them; where that fails, they are shown as they were last fetched, and said
/// to be. Nothing is asked and nothing written but the fetched refs, so there
/// is no terminal wanted.
pub(in crate::cli) fn status(flake: &str) -> anyhow::Result<ExitCode> {
    let repo = Repository::open(flake)?;
    let workdir = git::workdir(&repo, flake);
    let this = hostname::get()?.to_string_lossy().into_owned();

    if let Some(remote) = note::remote(&repo) {
        step(&format!("fetching the notes from {remote}"));

        match note::fetch_all(&repo, workdir, &remote) {
            Ok(kept) => {
                for kept in kept {
                    let host = &kept.host;
                    if kept.ahead {
                        // nothing is lost by it, only not yet sent
                        let sends = if *host == this {
                            "the next deployment here sends them".to_owned()
                        } else {
                            format!("`git push {remote} {}` sends them", note::notes_ref(host))
                        };
                        say(paint(
                            NOTE,
                            &format!("{remote} lacks some of {host}'s notes here; {sends}"),
                        ));
                    } else {
                        warn(&format!(
                            "{host}'s notes here and on {remote} each have some the other lacks, so the ones here are kept as they are"
                        ));
                    }
                }
            }
            Err(e) => warn(&format!(
                "the notes were not fetched, so they are as they were last fetched: {e:#}"
            )),
        }
    }

    let head = repo.head()?.peel_to_commit()?.id();
    let now = Local::now().fixed_offset();
    let mut hosts = vec![];

    for host in note::hosts(&repo)? {
        if let Some(record) = note::latest(&repo, &note::notes_ref(&host))? {
            hosts.push(host_tree(&repo, head, now, &host, host == this, &record)?);
        }
    }

    if hosts.is_empty() {
        say(paint(NOTE, "no host has a deployment recorded yet"));
        return Ok(ExitCode::SUCCESS);
    }

    let mut out = anstream::stdout();
    for tree in hosts {
        let _ = write!(out, "\n{tree}");
    }

    Ok(ExitCode::SUCCESS)
}

/// a host's latest deployment, as a tree under its name
fn host_tree(
    repo: &Repository,
    head: Oid,
    now: DateTime<FixedOffset>,
    host: &str,
    is_this: bool,
    record: &Record,
) -> anyhow::Result<Tree<String>> {
    let name = if is_this {
        format!("{} {}", paint(SCOPE, host), paint(NOTE, "(this host)"))
    } else {
        paint(SCOPE, host)
    };

    let done = match record.activation.as_str() {
        "none" => "already on",
        "boot" => "boot to",
        _ => "switch to",
    };
    let commit = repo.find_commit(record.commit).ok();
    let short = commit
        .as_ref()
        .and_then(|commit| commit.as_object().short_id().ok())
        .and_then(|short| short.as_str().ok().map(str::to_owned))
        .unwrap_or_else(|| record.commit.to_string()[..7].to_owned());
    let what = commit.as_ref().map_or_else(
        || paint(NOTE, "a commit not in this clone yet"),
        |commit| {
            commit
                .summary()
                .ok()
                .flatten()
                .unwrap_or_default()
                .to_owned()
        },
    );

    let mut leaves = vec![
        format!("{done} {short}: {what}"),
        format!(
            "{}, {}",
            record
                .rebuilt_at
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M"),
            ago(now - record.rebuilt_at)
        ),
    ];
    if commit.is_some() {
        leaves.push(against_head(repo, head, record.commit)?);
    }
    if is_this {
        leaves.push(running(record)?);
    }

    Ok(headed(name).with_leaves(leaves))
}

/// where the deployed commit stands against the one checked out
fn against_head(repo: &Repository, head: Oid, deployed: Oid) -> anyhow::Result<String> {
    let (behind, ahead) = repo.graph_ahead_behind(head, deployed)?;
    let commits = |n: usize| {
        if n == 1 {
            "1 commit".to_owned()
        } else {
            format!("{n} commits")
        }
    };

    Ok(match (behind, ahead) {
        (0, 0) => "at HEAD".to_owned(),
        (behind, 0) => format!("{} behind HEAD", commits(behind)),
        (0, ahead) => format!("{} ahead of HEAD", commits(ahead)),
        (behind, ahead) => format!(
            "apart from HEAD: {} behind it and {} ahead",
            commits(behind),
            commits(ahead)
        ),
    })
}

/// whether this host is running what its latest note says it was left on
fn running(record: &Record) -> anyhow::Result<String> {
    let running = Generation::running()?.to_string();
    let next_boot = Generation::next_boot()?.to_string();

    Ok(if running == record.generation {
        "running".to_owned()
    } else if next_boot == record.generation {
        "waiting for a reboot".to_owned()
    } else {
        "no longer running: the system has changed since, outside rbld".to_owned()
    })
}

/// how long ago something was, to the largest whole unit
fn ago(elapsed: TimeDelta) -> String {
    let plural = |n: i64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };

    match elapsed.num_seconds() {
        ..60 => "just now".to_owned(),
        60..3600 => plural(elapsed.num_minutes(), "minute"),
        3600..86_400 => plural(elapsed.num_hours(), "hour"),
        _ => plural(elapsed.num_days(), "day"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_takes_the_largest_whole_unit() {
        assert_eq!(ago(TimeDelta::seconds(5)), "just now");
        assert_eq!(ago(TimeDelta::minutes(1)), "1 minute ago");
        assert_eq!(ago(TimeDelta::minutes(59)), "59 minutes ago");
        assert_eq!(ago(TimeDelta::hours(25)), "1 day ago");
        assert_eq!(ago(TimeDelta::days(9)), "9 days ago");
    }
}
