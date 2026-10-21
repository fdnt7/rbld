//! how a command that deploys is told to activate, over what it would decide

use crate::util::generation::Activation;

#[derive(clap::Args, Debug)]
#[group(multiple = false)]
pub(super) struct Asked {
    /// Activate now, by switching to the new generation
    #[arg(long)]
    switch: bool,
    /// Activate at the next boot, leaving the running system alone
    #[arg(long)]
    boot: bool,
}

impl Asked {
    pub(super) const fn activation(&self) -> Option<Activation> {
        match (self.switch, self.boot) {
            (true, _) => Some(Activation::Switch),
            (_, true) => Some(Activation::Boot),
            _ => None,
        }
    }
}
