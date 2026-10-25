//! which flake a command works on
//!
//! rbld deploys through nh, so it takes the flake the way nh does: as the last
//! argument, or else from the variables nh itself reads, in the order nh reads
//! them. Whatever names the flake for `nh os switch` names it here too.

/// what nh reads for its `os` commands before anything else
const NH_OS_FLAKE: &str = "NH_OS_FLAKE";
/// what nh reads for any of its commands
const NH_FLAKE: &str = "NH_FLAKE";

#[derive(clap::Args, Debug)]
pub(super) struct Flake {
    // spelled out rather than a doc comment, which would want the variables
    // in backticks that help would then print
    #[arg(
        help = "The flake's repository [default: $NH_OS_FLAKE, else $NH_FLAKE]",
        value_hint = clap::ValueHint::DirPath
    )]
    flake: Option<String>,
}

impl Flake {
    /// the flake given, else the first of nh's variables that is set
    ///
    /// one set to nothing counts as unset, as nh counts it
    pub(super) fn resolve(self) -> Result<String, String> {
        self.flake
            .or_else(|| {
                [NH_OS_FLAKE, NH_FLAKE]
                    .into_iter()
                    .filter_map(std::env::var_os)
                    .find(|value| !value.is_empty())
                    .and_then(|value| value.into_string().ok())
            })
            .ok_or_else(|| {
                format!("no flake given, and neither {NH_OS_FLAKE} nor {NH_FLAKE} is set")
            })
    }
}
