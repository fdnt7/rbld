//! what a switch would restart, and which of it has to stay up
//!
//! A switch restarts the system units whose files changed, and Home Manager
//! the user's own; a unit restarted under a session takes the session with
//! it. What would be restarted is read from the unit files of the two
//! generations rather than replayed: comparing files needs no root, which
//! `switch-to-configuration dry-activate` does, and reads system and user
//! units alike.
//!
//! What has to stay up is read from the running system: the units rbld runs
//! under, and a few no session survives the restart of. The units the
//! graphical session is made of are watched rather than kept up: a switch
//! that restarts one is said and recorded, and goes ahead, since what they
//! are on fw16 (a wallpaper, a tray icon, a clipboard keeper) comes back on
//! its own, and a boot for them would hold back everything else as well.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    process::Command,
};

use crate::util::generation::Generation;

/// where a generation keeps its system units
const SYSTEM_UNITS: &str = "etc/systemd/system";
/// where a Home Manager generation keeps its user units
const USER_UNITS: &str = "home-files/.config/systemd/user";
/// what a Home Manager generation is called in the store
const HOME_GENERATION: &str = "-home-manager-generation";

/// the target a graphical session's own units are wanted by
const GRAPHICAL_SESSION: &str = "graphical-session.target";

/// system units no session survives the restart of, wherever rbld runs
///
/// NixOS marks every one of these to be reloaded or left alone, so they only
/// count where that changes. systemd-udevd is not among them: NixOS restarts
/// it for any change to the udev rules, and sessions see nothing of it.
const SESSION_CRITICAL: [&str; 4] = [
    "dbus.service",
    "dbus-broker.service",
    "systemd-logind.service",
    "display-manager.service",
];

/// which service manager a unit belongs to
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Manager {
    System,
    User,
}

impl Manager {
    fn systemctl(self) -> Command {
        let mut command = Command::new("systemctl");
        if self == Self::User {
            command.arg("--user");
        }
        command
    }
}

/// what a switch does to a unit that changed
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Restart,
    /// gone from the new generation, and stopped for it
    Stop,
}

impl Change {
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Stop => "stop",
        }
    }

    /// what the change did, as a note's trailer names it
    pub const fn done(self) -> &'static str {
        match self {
            Self::Restart => "Restarted",
            Self::Stop => "Stopped",
        }
    }
}

/// a unit of the graphical session a switch would restart or stop
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watched {
    pub change: Change,
    pub unit: String,
}

/// what switching from one generation to another would do to the running
/// session
pub struct Restarts {
    /// one for each unit that has to stay up and would not; none where a
    /// switch would leave all of them alone
    pub reasons: Vec<String>,
    /// the units of the graphical session it would restart or stop
    pub watched: Vec<Watched>,
}

/// a unit of the running session, and why it matters
struct Protected {
    manager: Manager,
    name: String,
    why: &'static str,
    /// whether a restart of it decides a boot, or is only watched
    counts: bool,
}

/// what switching from `running` to `new` would restart or stop of the
/// running session
///
/// Home Manager's user units are compared for the user running rbld, whose
/// session it is; those NixOS installs under `/etc/systemd/user` are never
/// restarted by a switch, so they are not compared.
pub fn restarts(new: &Generation, running: &Generation) -> anyhow::Result<Restarts> {
    let system = changes(
        &running.path().join(SYSTEM_UNITS),
        &new.path().join(SYSTEM_UNITS),
    )?;

    let user = std::env::var("USER").ok();
    let home = |generation: &Generation| -> io::Result<Option<PathBuf>> {
        user.as_ref()
            .map_or(Ok(None), |user| home_generation(generation, user))
    };
    let user = match (home(running)?, home(new)?) {
        (Some(old), Some(new)) => changes(&old.join(USER_UNITS), &new.join(USER_UNITS))?,
        _ => BTreeMap::new(),
    };

    let mut restarts = Restarts {
        reasons: vec![],
        watched: vec![],
    };
    for unit in protected() {
        let changes = match unit.manager {
            Manager::System => &system,
            Manager::User => &user,
        };

        if let Some(&change) = changes.get(&template(&unit.name))
            && is_active(unit.manager, &unit.name)
        {
            if unit.counts {
                restarts.reasons.push(format!(
                    "a switch would {} {}, {}",
                    change.verb(),
                    unit.name,
                    unit.why
                ));
            } else {
                restarts.watched.push(Watched {
                    change,
                    unit: unit.name,
                });
            }
        }
    }

    Ok(restarts)
}

/// the units that have to stay up, each once, by the first reason found
fn protected() -> Vec<Protected> {
    let mut units = vec![];

    let ancestry = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    units.extend(
        ancestors(&ancestry)
            .into_iter()
            .map(|(manager, name)| Protected {
                manager,
                name,
                why: "which rbld runs under",
                counts: true,
            }),
    );

    units.extend(graphical_session().into_iter().map(|name| Protected {
        manager: Manager::User,
        name,
        why: "which the graphical session is made of",
        counts: false,
    }));

    units.extend(SESSION_CRITICAL.iter().map(|name| Protected {
        manager: Manager::System,
        name: (*name).to_owned(),
        why: "which no session survives the restart of",
        counts: true,
    }));

    let mut seen = BTreeSet::new();
    units.retain(|unit| seen.insert((unit.manager, unit.name.clone())));
    units
}

/// the services in a cgroup path, as `/proc/self/cgroup` gives it: those
/// above a user manager's are the system's, and those below it the user's
fn ancestors(cgroup: &str) -> Vec<(Manager, String)> {
    let Some(path) = cgroup.lines().find_map(|line| line.strip_prefix("0::")) else {
        return vec![];
    };

    let mut manager = Manager::System;
    let mut units = vec![];
    for name in path.split('/') {
        if name.ends_with(".service") {
            units.push((manager, name.to_owned()));
        }
        if name.starts_with("user@") && name.ends_with(".service") {
            manager = Manager::User;
        }
    }

    units
}

/// the user units the running graphical session wants or requires, or none
/// where there is no user manager to ask
fn graphical_session() -> Vec<String> {
    let Ok(output) = Manager::User
        .systemctl()
        .args(["show", "--property=Wants", "--property=Requires", "--value"])
        .arg(GRAPHICAL_SESSION)
        .output()
    else {
        return vec![];
    };

    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .filter(|name| name.ends_with(".service"))
        .map(str::to_owned)
        .collect()
}

fn is_active(manager: Manager, name: &str) -> bool {
    manager
        .systemctl()
        .args(["is-active", "--quiet", name])
        .status()
        .is_ok_and(|status| status.success())
}

/// the Home Manager generation `generation` activates for `user`, from the
/// service that activates it, or nothing where there is none
fn home_generation(generation: &Generation, user: &str) -> io::Result<Option<PathBuf>> {
    let service = generation
        .path()
        .join(SYSTEM_UNITS)
        .join(format!("home-manager-{user}.service"));
    let text = match std::fs::read_to_string(service) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };

    Ok(text.find(HOME_GENERATION).and_then(|end| {
        let start = text[..end].rfind("/nix/store/")?;
        Some(PathBuf::from(&text[start..end + HOME_GENERATION.len()]))
    }))
}

/// the name of the file a unit is read from: an instance's is its template's
fn template(name: &str) -> String {
    match (name.find('@'), name.rfind('.')) {
        (Some(at), Some(dot)) if at < dot => format!("{}{}", &name[..=at], &name[dot..]),
        _ => name.to_owned(),
    }
}

/// the services and sockets in `old` that a switch to `new` would restart or
/// stop, by name
///
/// A unit is restarted where its file or any of its drop-ins changed, unless
/// it is marked to be reloaded or left alone instead; it is stopped where it
/// is gone, unless it is marked to be kept. New units are only started, which
/// nothing running notices.
fn changes(old: &Path, new: &Path) -> io::Result<BTreeMap<String, Change>> {
    let mut changes = BTreeMap::new();

    for name in units(old)?.union(&units(new)?) {
        let (before, after) = (unit(old, name)?, unit(new, name)?);
        let change = match (before, after) {
            (Some(before), Some(after)) if before != after && is_restarted(&after) => {
                Change::Restart
            }
            (Some(before), None) if marked(&before, "X-StopOnRemoval") != Some(false) => {
                Change::Stop
            }
            _ => continue,
        };
        changes.insert(name.clone(), change);
    }

    Ok(changes)
}

/// the services and sockets a unit directory has files for, missing being
/// empty
fn units(dir: &Path) -> io::Result<BTreeSet<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(e),
    };

    let mut names = BTreeSet::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // a drop-in or wants directory carries the unit's name, and is read
        // with the unit rather than as one; a link left dangling is no unit
        let is_dir = std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir());
        if (name.ends_with(".service") || name.ends_with(".socket")) && !is_dir {
            names.insert(name);
        }
    }

    Ok(names)
}

/// a unit's file followed by its drop-ins, in the order systemd reads them,
/// or nothing where the directory has no file for it
fn unit(dir: &Path, name: &str) -> io::Result<Option<String>> {
    let mut text = match std::fs::read_to_string(dir.join(name)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        // a masked unit is a link to /dev/null, which reads as empty
        Err(e) if e.kind() == io::ErrorKind::InvalidData => String::new(),
        Err(e) => return Err(e),
    };

    let drop_ins = dir.join(format!("{name}.d"));
    if let Ok(entries) = std::fs::read_dir(&drop_ins) {
        let mut confs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "conf"))
            .collect();
        confs.sort();
        for conf in confs {
            text.push('\n');
            text.push_str(&std::fs::read_to_string(conf)?);
        }
    }

    Ok(Some(text))
}

/// whether a changed unit is restarted for it, rather than reloaded or left
/// as it is, as NixOS and Home Manager mark it
fn is_restarted(unit: &str) -> bool {
    marked(unit, "X-RestartIfChanged") != Some(false)
        && marked(unit, "X-ReloadIfChanged") != Some(true)
        && !matches!(
            value(unit, "X-SwitchMethod").as_deref(),
            Some("reload" | "keep-old")
        )
}

/// the boolean a unit sets `key` to, its last setting winning, as systemd
/// reads it
fn marked(unit: &str, key: &str) -> Option<bool> {
    match value(unit, key)?.as_str() {
        "1" | "yes" | "true" | "on" => Some(true),
        "0" | "no" | "false" | "off" => Some(false),
        _ => None,
    }
}

/// the value a unit sets `key` to, its last setting winning
fn value(unit: &str, key: &str) -> Option<String> {
    unit.lines()
        .filter_map(|line| line.trim().split_once('='))
        .filter(|(name, _)| name.trim() == key)
        .map(|(_, value)| value.trim().to_ascii_lowercase())
        .next_back()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ancestry_splits_at_the_user_manager() {
        let cgroup = "1:net_cls:/\n0::/user.slice/user-1000.slice/user@1000.service/app.slice/foot.service\n";

        assert_eq!(
            ancestors(cgroup),
            vec![
                (Manager::System, "user@1000.service".to_owned()),
                (Manager::User, "foot.service".to_owned()),
            ]
        );
        assert_eq!(
            ancestors("0::/user.slice/user-1000.slice/session-3.scope\n"),
            vec![]
        );
    }

    #[test]
    fn an_instance_is_read_from_its_template() {
        assert_eq!(template("user@1000.service"), "user@.service");
        assert_eq!(template("foot.service"), "foot.service");
    }

    #[test]
    fn marks_decide_what_a_change_does() {
        assert!(is_restarted("[Service]\nExecStart=/bin/true\n"));
        assert!(!is_restarted("[Unit]\nX-RestartIfChanged=false\n"));
        assert!(!is_restarted("[Unit]\nX-ReloadIfChanged=true\n"));
        assert!(!is_restarted("[Unit]\nX-SwitchMethod=keep-old\n"));
        // the last setting wins, as a drop-in overrides the unit
        assert!(is_restarted(
            "[Unit]\nX-RestartIfChanged=false\n\n[Unit]\nX-RestartIfChanged=true\n"
        ));
    }

    #[test]
    fn changes_are_read_from_files_and_drop_ins() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (old, new) = (tmp.path().join("old"), tmp.path().join("new"));
        for dir in [&old, &new] {
            std::fs::create_dir_all(dir.join("drop.service.d")).expect("dirs");
            std::fs::write(dir.join("same.service"), "a").expect("same");
            std::fs::write(dir.join("drop.service"), "a").expect("drop");
            std::fs::write(dir.join("kept.service"), "a\nX-RestartIfChanged=false").expect("kept");
        }
        std::fs::write(old.join("drop.service.d/x.conf"), "a").expect("conf");
        std::fs::write(new.join("drop.service.d/x.conf"), "b").expect("conf");
        std::fs::write(new.join("kept.service"), "b\nX-RestartIfChanged=false").expect("kept");
        std::fs::write(old.join("gone.socket"), "a").expect("gone");
        std::fs::write(new.join("added.service"), "a").expect("added");
        std::fs::create_dir(new.join("default.target.wants")).expect("wants");

        let changes = changes(&old, &new).expect("changes");
        assert_eq!(
            changes.into_iter().collect::<Vec<_>>(),
            vec![
                ("drop.service".to_owned(), Change::Restart),
                ("gone.socket".to_owned(), Change::Stop),
            ]
        );
    }
}
