//! the repository, as the commands have occasion to read and write it

use std::{collections::HashSet, path::Path, process::Command};

use {
    crate::util::terminal::Interrupts,
    git2::{Commit, Repository, ResetType, StatusOptions, build::CheckoutBuilder},
};

/// every path the repository has a change against, in the order git reports them
///
/// renames and copies move a file, so both endpoints belong to the change, and
/// a path arrived at both ways is listed once. Untracked files are no part of
/// it: nix never copies them into the store, so a flake builds the same with
/// them there as without, and a command that stops for a dirty tree has no
/// business stopping for those.
pub fn changed_paths(repo: &Repository) -> anyhow::Result<Vec<String>> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(false)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let mut seen = HashSet::new();
    let mut paths = vec![];

    for entry in repo.statuses(Some(&mut options))?.iter() {
        let deltas = [entry.head_to_index(), entry.index_to_workdir()];
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

/// commits `path` on `flake` with `message`, saying whether git took it
///
/// The file is named as a pathspec rather than staged first, so what goes in
/// is the one thing the caller means to commit and nothing that happens to be
/// sitting in the index beside it. The repository's hooks are skipped: they
/// are written for what a person edits, and a file a program regenerates is
/// not that.
///
/// git is given our terminal as it stands rather than a pty, having something
/// of its own to ask for on the way -- a passphrase for the key it signs with
/// -- and asking it wherever it finds a terminal to ask on. That makes the two
/// of us one foreground process group, and a ctrl-c answered into that prompt
/// arrives at both, which is what the interrupts are held against.
pub fn commit(flake: &str, message: &str, path: &str) -> anyhow::Result<bool> {
    let _interrupts = Interrupts::held()?;

    let status = Command::new("git")
        .arg("-C")
        .arg(flake)
        .arg("commit")
        .arg("--message")
        .arg(message)
        .arg("--no-verify")
        .arg("--")
        .arg(path)
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
