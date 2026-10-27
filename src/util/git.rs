//! the repository, as the commands have occasion to read and write it

use std::{
    collections::{BTreeSet, HashSet},
    io::Write as _,
    path::Path,
    process::Command,
};

use {
    crate::util::{
        scope::{HOSTS, Scope},
        terminal::Interrupts,
    },
    git2::{Commit, Oid, Repository, ResetType, StatusOptions, build::CheckoutBuilder},
};

/// every path the repository has a change against, in the order git reports them
///
/// renames and copies move a file, so both endpoints belong to the change, and
/// a path arrived at both ways is listed once. Untracked files are no part of
/// it: nix never copies them into the store, so a flake builds the same with
/// them there as without, and a command that stops for a dirty tree has no
/// business stopping for those.
pub fn changed_paths(repo: &Repository) -> anyhow::Result<Vec<String>> {
    paths(repo, true)
}

/// every path the next commit would change, in the order git reports them
///
/// what [`changed_paths`] reports, less what has not been staged: a hook runs
/// on the commit being made, and a change still in the work tree is no part of
/// it.
pub fn staged_paths(repo: &Repository) -> anyhow::Result<Vec<String>> {
    paths(repo, false)
}

/// the names of the hosts, being the directories under `hosts/` in the index
///
/// the index rather than the work tree or head, so that a host the commit
/// being made adds is a host already, and one it removes is no longer one.
pub fn hosts(repo: &Repository) -> anyhow::Result<BTreeSet<String>> {
    let prefix = format!("{HOSTS}/");

    Ok(repo
        .index()?
        .iter()
        .filter_map(|entry| {
            let path = String::from_utf8(entry.path).ok()?;
            let (host, _) = path.strip_prefix(&prefix)?.split_once('/')?;

            Some(host.to_owned())
        })
        .collect())
}

fn paths(repo: &Repository, unstaged: bool) -> anyhow::Result<Vec<String>> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(false)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(unstaged);

    let mut seen = HashSet::new();
    let mut paths = vec![];

    for entry in repo.statuses(Some(&mut options))?.iter() {
        let deltas = [
            entry.head_to_index(),
            entry.index_to_workdir().filter(|_| unstaged),
        ];
        let files = deltas
            .iter()
            .flatten()
            .flat_map(|delta| [delta.new_file(), delta.old_file()]);

        for path in files.filter_map(|file| file.path().and_then(Path::to_str)) {
            if seen.insert(path.to_owned()) {
                paths.push(path.to_owned());
            }
        }
    }

    Ok(paths)
}

/// where the repository keeps its files, which is what the paths git reports
/// are relative to
///
/// `flake` stands in for a repository with no work tree to speak of, being
/// where the command was pointed and the best that can be said of it.
pub fn workdir<'a>(repo: &'a Repository, flake: &'a str) -> &'a str {
    repo.workdir().and_then(Path::to_str).unwrap_or(flake)
}

/// commits `paths` on `flake` with `message`, or what is staged where there
/// are none, saying whether git took it
///
/// Files are named as pathspecs rather than staged first, so what goes in is
/// what the caller means to commit and nothing that happens to be sitting in
/// the index beside it. The repository's hooks are skipped: they are written
/// for what a person edits and the message they write for it, and a file a
/// program regenerates is not the one, nor a placeholder the other. Changes a
/// person made have had the pre-commit hook run on them by [`hook`] instead.
///
/// git is given our terminal as it stands rather than a pty, having something
/// of its own to ask for on the way -- a passphrase for the key it signs with
/// -- and asking it wherever it finds a terminal to ask on. That makes the two
/// of us one foreground process group, and a ctrl-c answered into that prompt
/// arrives at both, which is what the interrupts are held against.
pub fn commit(flake: &str, message: &str, paths: &[&str]) -> anyhow::Result<bool> {
    let _interrupts = Interrupts::held()?;

    let status = Command::new("git")
        .arg("-C")
        .arg(flake)
        .args(["commit", "--no-verify", "--message", message, "--"])
        .args(paths)
        .status()?;

    Ok(status.success())
}

/// stages every change to a tracked file, as `git add --update` does,
/// handing back the index as it stood before, for [`restore`]
///
/// untracked files are left out, as [`changed_paths`] leaves them out.
pub fn stage_tracked(repo: &Repository) -> anyhow::Result<Oid> {
    let mut index = repo.index()?;
    let before = index.write_tree()?;

    index.update_all(["*"], None)?;
    index.write()?;

    Ok(before)
}

/// runs the repository's `name` hook on `flake`, as git would on its way to a
/// commit, saying whether it passed; a repository without one passes
///
/// On the terminal, for it to say what it found as it would have.
pub fn hook(flake: &str, name: &str) -> anyhow::Result<bool> {
    let _interrupts = Interrupts::held()?;

    let status = Command::new("git")
        .arg("-C")
        .arg(flake)
        .args(["hook", "run", "--ignore-missing", name])
        .status()?;

    Ok(status.success())
}

/// puts head back to `target` and the index back to `index`, as
/// [`stage_tracked`] found it, and leaves the work tree alone
///
/// the work tree is what a person changed, which [`stage_tracked`] and a
/// commit of the index only ever read; anything a hook rewrote in it is that
/// hook's doing, and left to be seen.
pub fn restore(repo: &Repository, target: &Commit<'_>, index: Oid) -> anyhow::Result<()> {
    repo.reset(target.as_object(), ResetType::Soft, None)?;

    let mut staged = repo.index()?;
    staged.read_tree(&repo.find_tree(index)?)?;
    staged.write()?;

    Ok(())
}

/// opens the editor git would open on `text`, handing back what is left in
/// it, or nothing where the editor fails, as quitting one without saving
/// does in some
///
/// The file is named as git names a commit message's, so an editor that knows
/// one knows the other and shows it as such, scissors line and all.
pub fn edit(flake: &str, text: &str) -> anyhow::Result<Option<String>> {
    let editor = Command::new("git")
        .arg("-C")
        .arg(flake)
        .args(["var", "GIT_EDITOR"])
        .output()?;
    if !editor.status.success() {
        anyhow::bail!(
            "git var GIT_EDITOR: {}",
            String::from_utf8_lossy(&editor.stderr).trim()
        );
    }
    let editor = String::from_utf8_lossy(&editor.stdout).trim().to_owned();

    let dir = tempfile::Builder::new().prefix("rbld-").tempdir()?;
    let path = dir.path().join("COMMIT_EDITMSG");
    std::fs::write(&path, text)?;

    // run through the shell, as git runs it, the editor being a command line
    // of its own and not only a program's name
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$@\""))
        .arg(&editor)
        .arg(&path)
        .current_dir(flake)
        .status()?;

    if !status.success() {
        return Ok(None);
    }

    Ok(Some(std::fs::read_to_string(&path)?))
}

/// runs the repository's `commit-msg` hook on `message`, as git would on its
/// way to a commit, handing back the message as the hook leaves it where it
/// passes, which a hook may have added to, and nothing where it refuses; a
/// repository without one passes
///
/// On the terminal, for what it says to be seen as it would on a refused
/// commit, colours and all, before anything is asked about it.
pub fn check_message(flake: &str, message: &str) -> anyhow::Result<Option<String>> {
    let mut file = tempfile::Builder::new()
        .prefix("rbld-")
        .suffix(".txt")
        .tempfile()?;
    file.write_all(message.as_bytes())?;

    let status = Command::new("git")
        .arg("-C")
        .arg(flake)
        .args(["hook", "run", "--ignore-missing", "commit-msg", "--"])
        .arg(file.path())
        .status()?;

    if !status.success() {
        return Ok(None);
    }

    Ok(Some(std::fs::read_to_string(file.path())?))
}

/// whether `commit` is on the upstream of the branch checked out, which is
/// to say pushed; a branch without one has nothing pushed to say so of
pub fn published(repo: &Repository, commit: Oid) -> anyhow::Result<bool> {
    let head = repo.head()?;
    if !head.is_branch() {
        return Ok(false);
    }
    let Ok(upstream) = git2::Branch::wrap(head).upstream() else {
        return Ok(false);
    };
    let Some(pushed) = upstream.get().target() else {
        return Ok(false);
    };

    Ok(pushed == commit || repo.graph_descendant_of(pushed, commit)?)
}

/// what head changes, file by file, as `git show --stat` has it
pub fn stat(flake: &str) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(flake)
        .args(["show", "--stat", "--format=", "HEAD"])
        .output()?;

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// gives head `message` and nothing else, its tree left as it is whatever is
/// staged, saying whether git took it
///
/// The message has been through the message hook by [`check_message`], and
/// the tree is the one head already had, so neither hook runs again.
pub fn reword(flake: &str, message: &str) -> anyhow::Result<bool> {
    let _interrupts = Interrupts::held()?;

    let mut file = tempfile::Builder::new()
        .prefix("rbld-")
        .suffix(".txt")
        .tempfile()?;
    file.write_all(message.as_bytes())?;

    let status = Command::new("git")
        .arg("-C")
        .arg(flake)
        .args([
            "commit",
            "--amend",
            "--only",
            "--no-verify",
            "--cleanup=verbatim",
            "--file",
        ])
        .arg(file.path())
        .status()?;

    Ok(status.success())
}

/// gives head `message`, as it stands, saying whether git took it
///
/// The hooks are skipped: the changes went through the pre-commit hook before
/// head was committed, and `message` through the message hook by
/// [`check_message`]. What is left to refuse it is signing it.
pub fn amend(flake: &str, message: &str) -> anyhow::Result<bool> {
    let _interrupts = Interrupts::held()?;

    let mut file = tempfile::Builder::new()
        .prefix("rbld-")
        .suffix(".txt")
        .tempfile()?;
    file.write_all(message.as_bytes())?;

    let status = Command::new("git")
        .arg("-C")
        .arg(flake)
        .args([
            "commit",
            "--amend",
            "--no-verify",
            "--cleanup=verbatim",
            "--file",
        ])
        .arg(file.path())
        .status()?;

    Ok(status.success())
}

/// puts head back to `target` and `path` back to what `target` has of it, and
/// puts nothing else back
///
/// A command that writes one file and commits it leaves behind one file and at
/// most one commit, which is all there is to take back: the reset moves head
/// off whatever commit is standing there, and the checkout fetches the file
/// back out of where head now points. Whether a commit was made is the reset's
/// business rather than the caller's, so a caller names the commit it means to
/// end up on and lets a ref write that lands where it started be a ref write.
///
/// The reset is soft because moving head is all it is wanted for. A hard one
/// would put the file back too, and then go on putting back whatever else it
/// found: the programs these commands run take as long as they take, and what
/// a person changed elsewhere in the tree while one of them was running is no
/// part of what there is to undo.
pub fn undo(repo: &Repository, target: &Commit<'_>, path: &str) -> anyhow::Result<()> {
    repo.reset(target.as_object(), ResetType::Soft, None)?;

    let mut checkout = CheckoutBuilder::new();
    checkout.force().path(path);
    repo.checkout_head(Some(&mut checkout))?;

    Ok(())
}

/// where head stands against the branch it follows, once that is fetched
pub enum CaughtUp {
    /// nothing came in: head follows no branch, or has everything the one it
    /// follows has, whether or not it has more besides
    Current,
    /// head was behind, and has been fast forwarded from `from` to `to`
    Forwarded {
        upstream: String,
        from: Oid,
        to: Oid,
    },
    /// the remote could not be fetched from, as it cannot with no network
    Unfetched { remote: String, why: String },
    /// head and its upstream each have commits the other lacks
    Diverged {
        upstream: String,
        ahead: usize,
        behind: usize,
    },
    /// head is behind, but fast forwarding it would overwrite changes here
    Blocked { upstream: String, why: String },
}

/// fetches the remote the branch checked out follows, and fast forwards head
/// to it where head is only behind
///
/// Never more than a fast forward: where the two have diverged, a rebase
/// would make the commits here over, signed again and dated anew, and a merge
/// would be a commit nobody wrote, so that is left to be done by hand. A
/// fetch that fails is said rather than stopped on, since a configuration
/// that broke the network has to be deployable without one.
pub fn catch_up(repo: &Repository, workdir: &str) -> anyhow::Result<CaughtUp> {
    let head = repo.head()?;
    let Some(branch) = head.name().ok().filter(|_| head.is_branch()) else {
        return Ok(CaughtUp::Current);
    };
    let Some(remote) = repo
        .branch_upstream_remote(branch)
        .ok()
        .and_then(|remote| remote.as_str().ok().map(str::to_owned))
    else {
        return Ok(CaughtUp::Current);
    };

    let fetched = {
        let _interrupts = Interrupts::held()?;
        Command::new("git")
            .arg("-C")
            .arg(workdir)
            .args(["fetch", "--quiet", &remote])
            .output()?
    };
    if !fetched.status.success() {
        return Ok(CaughtUp::Unfetched {
            remote,
            why: String::from_utf8_lossy(&fetched.stderr).trim().to_owned(),
        });
    }

    // read again: the fetch has moved the upstream under the one read above
    let Ok(upstream) = git2::Branch::wrap(repo.head()?).upstream() else {
        return Ok(CaughtUp::Current);
    };
    let name = upstream.name()?.unwrap_or("upstream").to_owned();
    let (Some(ours), Some(theirs)) = (repo.head()?.target(), upstream.get().target()) else {
        return Ok(CaughtUp::Current);
    };
    let (ahead, behind) = repo.graph_ahead_behind(ours, theirs)?;

    if behind == 0 {
        return Ok(CaughtUp::Current);
    }
    if ahead > 0 {
        return Ok(CaughtUp::Diverged {
            upstream: name,
            ahead,
            behind,
        });
    }

    let merged = {
        let _interrupts = Interrupts::held()?;
        Command::new("git")
            .arg("-C")
            .arg(workdir)
            .args(["merge", "--ff-only", "--quiet"])
            .arg(theirs.to_string())
            .output()?
    };
    if !merged.status.success() {
        return Ok(CaughtUp::Blocked {
            upstream: name,
            why: String::from_utf8_lossy(&merged.stderr).trim().to_owned(),
        });
    }

    Ok(CaughtUp::Forwarded {
        upstream: name,
        from: ours,
        to: theirs,
    })
}

/// a commit a catch up brought in
pub struct Incoming {
    /// abbreviated, as git would
    pub id: String,
    pub summary: String,
    /// the other host it touches, where it touches nothing but that host and
    /// so cannot change this one
    pub elsewhere: Option<String>,
}

/// the commits after `from` up to `to`, oldest first, each said of whether it
/// can change `host`
///
/// Only a commit to another host's part alone is said not to: a user's part
/// may be taken up by a host the user has no account on, as k takes up f's
/// features, so no other part can be ruled out by its path.
pub fn incoming(
    repo: &Repository,
    from: Oid,
    to: Oid,
    host: &str,
) -> anyhow::Result<Vec<Incoming>> {
    let hosts = hosts(repo)?;
    let mut walk = repo.revwalk()?;
    walk.push(to)?;
    walk.hide(from)?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::REVERSE)?;

    let mut commits = vec![];
    for oid in walk {
        let commit = repo.find_commit(oid?)?;
        let parent = commit
            .parents()
            .next()
            .map(|parent| parent.tree())
            .transpose()?;
        let diff = repo.diff_tree_to_tree(parent.as_ref(), Some(&commit.tree()?), None)?;

        let mut scopes = BTreeSet::new();
        for delta in diff.deltas() {
            for file in [delta.old_file(), delta.new_file()] {
                if let Some(path) = file.path().and_then(Path::to_str) {
                    scopes.insert(Scope::of(path, &hosts));
                }
            }
        }
        let elsewhere = match scopes.iter().collect::<Vec<_>>()[..] {
            [Scope::Host(other)] if other != host => Some(other.clone()),
            _ => None,
        };

        commits.push(Incoming {
            id: commit
                .as_object()
                .short_id()?
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            summary: commit
                .summary()
                .ok()
                .flatten()
                .unwrap_or_default()
                .to_owned(),
            elsewhere,
        });
    }

    Ok(commits)
}
