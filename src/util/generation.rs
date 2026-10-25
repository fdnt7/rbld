//! NixOS system generations: the one running, the one the machine boots into
//! next, and one just built, and how a built one is made either of the others

use std::{
    fmt, io,
    path::{Path, PathBuf},
};

use {
    crate::util::units,
    serde_json::{Map, Value},
};

/// the generation the machine is running
const RUNNING: &str = "/run/current-system";
/// the generation the machine boots into next
const NEXT_BOOT: &str = "/nix/var/nix/profiles/system";

/// what a generation holds that is only ever taken up at boot: the kernel
/// and what is loaded with it, each a link to a build of its own
const BOOT_LINKS: [&str; 4] = ["kernel", "initrd", "kernel-modules", "firmware"];
/// the kernel's command line, as text
const KERNEL_PARAMS: &str = "kernel-params";
/// what NixOS refuses to switch across, as JSON of a name for each
const SWITCH_INHIBITORS: &str = "switch-inhibitors";

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

    /// what `self` differs from `running` in that only a reboot takes up,
    /// one reason for each, and none where a switch would do
    ///
    /// A switch leaves the kernel running as it is, so a new one, new modules
    /// for it, a new initrd or firmware, or a new command line would wait for
    /// a reboot regardless; and NixOS refuses to switch across a change to
    /// its inhibitors at all. Anything a generation lacks is as if it were
    /// empty, so one without a kernel, as a container's is, compares only
    /// with another without.
    pub fn reboot_reasons(&self, running: &Self) -> io::Result<Vec<String>> {
        let mut reasons = vec![];

        for name in BOOT_LINKS {
            if self.link(name)? != running.link(name)? {
                reasons.push(format!("{name} changed"));
            }
        }

        if self.text(KERNEL_PARAMS)?.trim() != running.text(KERNEL_PARAMS)?.trim() {
            reasons.push(format!("{KERNEL_PARAMS} changed"));
        }

        let (new, old) = (
            self.text(SWITCH_INHIBITORS)?,
            running.text(SWITCH_INHIBITORS)?,
        );
        if let Some(changed) = inhibitors_changed(&new, &old) {
            reasons.push(format!("{SWITCH_INHIBITORS} changed{changed}"));
        }

        Ok(reasons)
    }

    /// where `name` in this generation leads, or nothing where there is none
    fn link(&self, name: &str) -> io::Result<Option<PathBuf>> {
        absent_as_none(std::fs::canonicalize(self.0.join(name)))
    }

    /// what `name` in this generation says, or nothing where there is none
    fn text(&self, name: &str) -> io::Result<String> {
        absent_as_none(std::fs::read_to_string(self.0.join(name))).map(Option::unwrap_or_default)
    }
}

fn absent_as_none<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// whether two sets of inhibitors differ, and if so, which of them did, as a
/// suffix to the reason: by name where both are JSON as NixOS writes them,
/// and as text otherwise
fn inhibitors_changed(new: &str, old: &str) -> Option<String> {
    let parse = |text: &str| -> Option<Map<String, Value>> {
        if text.trim().is_empty() {
            Some(Map::new())
        } else {
            serde_json::from_str(text).ok()
        }
    };

    match (parse(new), parse(old)) {
        (Some(new), Some(old)) => {
            let mut names: Vec<&str> = new
                .keys()
                .chain(old.keys())
                .map(String::as_str)
                .filter(|&name| new.get(name) != old.get(name))
                .collect();
            names.sort_unstable();
            names.dedup();

            (!names.is_empty()).then(|| format!(": {}", names.join(", ")))
        }
        _ => (new.trim() != old.trim()).then(String::new),
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
    /// the units of the graphical session a switch restarts or stops, which
    /// decide nothing but are said and recorded; none for a boot
    pub watched: Vec<units::Watched>,
}

impl Decision {
    /// what was asked for on the command line, else a boot where `new` needs
    /// a reboot to be taken up, or where switching to it would restart
    /// something that has to stay up, and a switch otherwise
    pub fn new(asked: Option<Activation>, new: &Generation) -> anyhow::Result<Self> {
        let running = Generation::running()?;
        let restarts = units::restarts(new, &running)?;

        let (activation, reasons) = if let Some(activation) = asked {
            (activation, vec![format!("asked for with --{activation}")])
        } else {
            let mut reasons = new.reboot_reasons(&running)?;
            reasons.extend(restarts.reasons);
            let activation = if reasons.is_empty() {
                Activation::Switch
            } else {
                Activation::Boot
            };
            (activation, reasons)
        };

        Ok(Self {
            activation,
            reasons,
            watched: if activation == Activation::Switch {
                restarts.watched
            } else {
                vec![]
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    /// a toplevel in `dir`, its kernel a link to `kernel` and its other
    /// files as given
    fn toplevel(dir: &Path, kernel: &str, files: &[(&str, &str)]) -> Generation {
        std::fs::create_dir_all(dir).expect("toplevel");
        let target = dir.with_extension(kernel);
        std::fs::write(&target, kernel).expect("kernel");
        symlink(&target, dir.join("kernel")).expect("link");
        for (name, text) in files {
            std::fs::write(dir.join(name), text).expect("file");
        }

        Generation(dir.to_owned())
    }

    #[test]
    fn the_same_boot_files_need_no_reboot() {
        let tmp = tempfile::tempdir().expect("tmp");
        let files = [
            (KERNEL_PARAMS, "quiet loglevel=4"),
            (SWITCH_INHIBITORS, r#"{"dbus-implementation":"broker"}"#),
        ];
        let old = toplevel(&tmp.path().join("old"), "a", &files);
        let new = toplevel(&tmp.path().join("new"), "a", &files);

        // the kernels are different files with the same contents, which is
        // not the same build
        assert_eq!(
            new.reboot_reasons(&old).ok(),
            Some(vec!["kernel changed".to_owned()])
        );

        let same = Generation(old.path().to_owned());
        assert_eq!(same.reboot_reasons(&old).ok(), Some(vec![]));
    }

    #[test]
    fn a_new_command_line_needs_a_reboot() {
        let tmp = tempfile::tempdir().expect("tmp");
        let old = toplevel(&tmp.path().join("old"), "k", &[(KERNEL_PARAMS, "quiet")]);
        let new = Generation(tmp.path().join("new"));
        std::fs::create_dir(new.path()).expect("new");
        symlink(old.path().join("kernel"), new.path().join("kernel")).expect("link");
        std::fs::write(new.path().join(KERNEL_PARAMS), "quiet loglevel=4").expect("params");

        assert_eq!(
            new.reboot_reasons(&old).ok(),
            Some(vec!["kernel-params changed".to_owned()])
        );
    }

    #[test]
    fn inhibitors_are_named_as_they_change() {
        assert_eq!(
            inhibitors_changed(r#"{"a": "1", "b": "2"}"#, r#"{"b":"2","a":"1"}"#),
            None
        );
        assert_eq!(
            inhibitors_changed(r#"{"a": "1", "c": "3"}"#, r#"{"a": "2", "b": "2"}"#),
            Some(": a, b, c".to_owned())
        );
        assert_eq!(inhibitors_changed("", "{}"), None);
        assert_eq!(inhibitors_changed("odd", "odder"), Some(String::new()));
    }
}
