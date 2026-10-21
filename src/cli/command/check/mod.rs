pub mod scope;

#[derive(clap::Subcommand, Debug)]
pub(in crate::cli) enum CheckCommand {
    /// Refuse a commit whose staged changes span more than one scope
    // `boundaries` is what this was called before it knew about scopes, kept
    // so that a hook still naming it runs until it is renamed
    #[command(alias = "boundaries")]
    Scope,
}
