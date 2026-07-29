pub mod boundaries;

#[derive(clap::Subcommand, Debug)]
pub(in crate::cli) enum CheckCommand {
    Boundaries,
}
