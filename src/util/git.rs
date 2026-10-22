//! the repository, as the commands have occasion to read and write it

use std::{
    collections::{BTreeSet, HashSet},
    io::Write as _,
    path::Path,
    process::Command,
};

use {
    crate::util::{scope::HOSTS, terminal::Interrupts},
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

/// opens the editor on head's message, starting from `message`, and amends
/// head with what comes back, saying whether git took it
///
/// Everything from a scissors line in `message` down is shown in the editor
/// and never kept. The hooks run as on any commit, the message hook being the
/// one with anything left to check, and git stops where it is refused, or
/// where the editor leaves it no message, keeping head as it was.
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
            "--edit",
            "--cleanup=scissors",
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
