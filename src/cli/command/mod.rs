pub(in crate::cli) mod check;

#[derive(clap::Subcommand, Debug)]
pub(super) enum Command {
    Switch,
    Update,
    Check {
        #[command(subcommand)]
        command: check::CheckCommand,
    },
}
