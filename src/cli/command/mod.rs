pub(super) mod check;
pub(super) mod update;

#[derive(clap::Subcommand, Debug)]
pub(super) enum Command {
    Switch,
    Update,
    Check {
        #[command(subcommand)]
        command: check::CheckCommand,
    },
}
