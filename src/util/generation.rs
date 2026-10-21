//! NixOS system generations: the one running, the one the machine boots into
//! next, and one just built, and how a built one is made either of the others

use std::{
    fmt, io,
    path::{Path, PathBuf},
};

/// the generation the machine is running
const RUNNING: &str = "/run/current-system";
/// the generation the machine boots into next
const NEXT_BOOT: &str = "/nix/var/nix/profiles/system";

/// a system generation, by the store path of its toplevel
///
/// links are followed on the way in, so two generations compare equal exactly
/// when they are the same build, whatever led to them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation(PathBuf);

impl Generation {
    /// the generation `link` leads to
    pub fn at(link: &Path) -> io::Result<Self> {
        std::fs::canonicalize(link).map(Self)
    }

    pub fn running() -> io::Result<Self> {
        Self::at(Path::new(RUNNING))
    }

    pub fn next_boot() -> io::Result<Self> {
        Self::at(Path::new(NEXT_BOOT))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(f)
    }
}

/// how a generation is made the system's own
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    /// now, in place of the one running, and at every boot after
    Switch,
    /// at the next boot, leaving the one running alone
    Boot,
}

impl Activation {
    /// the generation this activation puts another in place of
    pub fn replaces(self) -> io::Result<Generation> {
        match self {
            Self::Switch => Generation::running(),
            Self::Boot => Generation::next_boot(),
        }
    }

    /// whether `generation` is where this activation leaves the machine,
    /// which is how an activation that failed part way is told apart from one
    /// that never happened
    pub fn reached(self, generation: &Generation) -> io::Result<bool> {
        Ok(&self.replaces()? == generation)
    }

    /// the nh subcommand that carries it out, which is also its name
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Switch => "switch",
            Self::Boot => "boot",
        }
    }
}

impl fmt::Display for Activation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.verb())
    }
}

/// how a generation is to be activated, and every reason it is to be
/// activated that way rather than the ordinary one
pub struct Decision {
    pub activation: Activation,
    pub reasons: Vec<String>,
}

impl Decision {
    /// what was asked for on the command line, else a switch
    pub fn new(asked: Option<Activation>) -> Self {
        asked.map_or(
            Self {
                activation: Activation::Switch,
                reasons: vec![],
            },
            |activation| Self {
                activation,
                reasons: vec![format!("asked for with --{activation}")],
            },
        )
    }
}
