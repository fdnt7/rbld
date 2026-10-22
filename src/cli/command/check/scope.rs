use std::process::ExitCode;

use {
    super::super::refuse_split,
    crate::util::{git, scope},
    git2::Repository,
};

/// refuses a commit whose staged changes span more than one scope
///
/// a commit is meant to be about one part of the configuration, so one
/// that changes two is two commits run together, and the way out is to
/// split it: the listing groups what was staged by scope, which is the
/// split to make. The message is no part of it: the scope written there is
/// a person's own word for the change, which is free to be narrower than
/// the part of the configuration it sits in.
pub(in crate::cli) fn check_scope(flake: &str) -> anyhow::Result<ExitCode> {
    let repo = Repository::open(flake)?;
    let scopes = scope::group(git::staged_paths(&repo)?, &git::hosts(&repo)?);

    if scopes.len() <= 1 {
        return Ok(ExitCode::SUCCESS);
    }

    refuse_split(
        &format!(
            "this commit spans {} scopes; split it into one commit for each",
            scopes.len()
        ),
        git::workdir(&repo, flake),
        &scopes,
    );

    Ok(ExitCode::FAILURE)
}
