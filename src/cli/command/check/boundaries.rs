use std::{collections::BTreeMap, process::ExitCode};

use {
    crate::util::{
        git,
        render::{HEADLINE, NOTE, file_path, paint, rooted},
        scope::Scope,
        terminal::refuse,
    },
    anstyle::Style,
    git2::Repository,
    termtree::Tree,
};

/// a scope's name, bold so it heads the files under it
const SCOPE: Style = Style::new().bold();

impl crate::cli::Cli {
    /// refuses a commit whose staged changes span more than one scope
    ///
    /// a commit is meant to be about one part of the configuration, so one
    /// that changes two is two commits run together, and the way out is to
    /// split it: the listing groups what was staged by scope, which is the
    /// split to make. The message is no part of it: the scope written there is
    /// a person's own word for the change, which is free to be narrower than
    /// the part of the configuration it sits in.
    pub(in crate::cli) fn check_boundaries(self) -> anyhow::Result<ExitCode> {
        let repo = Repository::open(&self.flake)?;
        let hosts = git::hosts(&repo)?;

        let mut scopes: BTreeMap<Scope, Vec<String>> = BTreeMap::new();
        for path in git::staged_paths(&repo)? {
            scopes
                .entry(Scope::of(&path, &hosts))
                .or_default()
                .push(path);
        }

        if scopes.len() <= 1 {
            return Ok(ExitCode::SUCCESS);
        }

        let headline = paint(
            HEADLINE,
            &format!(
                "this commit spans {} scopes; split it into one commit for each",
                scopes.len()
            ),
        );

        let mut tree = rooted(git::workdir(&repo, &self.flake));
        for (scope, paths) in &scopes {
            // the kind is said in words as well as set apart, since prek
            // captures this hook and strips the styling when it does
            let heading = format!(
                "{} {}",
                paint(SCOPE, &scope.to_string()),
                paint(NOTE, &format!("({})", scope.kind()))
            );
            let leaves = paths
                .iter()
                .map(|path| Tree::new(file_path(Style::new(), path)));

            tree.push(Tree::new(heading).with_leaves(leaves));
        }

        refuse(headline, tree);

        Ok(ExitCode::FAILURE)
    }
}
