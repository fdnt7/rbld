use std::process::ExitCode;

use {
    super::refuse_dirty,
    crate::util::{
        deploy::{self, Outcome},
        generation::Activation,
        git,
        terminal::{Interrupts, has_terminal, stopped},
    },
    git2::Repository,
};

/// deploys the commit checked out, as it was committed
///
/// a tree with changes in it is refused for now: what is deployed is what a
/// note can be written on, and that is a commit.
pub(in crate::cli) fn switch(flake: &str, asked: Option<Activation>) -> anyhow::Result<ExitCode> {
    // the deployment stops to be confirmed, so there is no starting one
    // without a terminal to confirm it on
    if !has_terminal() {
        return Ok(ExitCode::FAILURE);
    }

    let repo = Repository::open(flake)?;
    let workdir = git::workdir(&repo, flake);
    let paths = git::changed_paths(&repo)?;

    if !paths.is_empty() {
        refuse_dirty(
            "cannot switch on a dirty working directory",
            workdir,
            &paths,
        );
        return Ok(ExitCode::FAILURE);
    }

    let host = hostname::get()?.to_string_lossy().into_owned();
    let commit = repo.head()?.peel_to_commit()?;

    // a ctrl-c is for whichever program has the terminal, and we stay to say
    // what became of it
    let _interrupts = Interrupts::held()?;

    match deploy::deploy(&repo, workdir, &host, asked)? {
        Outcome::Deployed(deployment) => {
            deployment.record(&repo, workdir, commit.id());
            Ok(ExitCode::SUCCESS)
        }
        Outcome::NotDeployed(headline) => {
            stopped(headline, "the system is as it was");
            Ok(ExitCode::FAILURE)
        }
    }
}
