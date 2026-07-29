use std::path::PathBuf;

use clap::CommandFactory;

use crate::cli::Cli;

/// where the config sits under the config home, so a declarative install has
/// somewhere to write one without every invocation naming it
const DEFAULT_PATH: &str = "rbld/config.toml";

#[derive(Clone, Default, serde::Deserialize)]
pub(super) struct FileConfig {
    flake: Option<String>,
}

impl FileConfig {
    pub(super) fn load(path: &str) -> Result<Self, String> {
        static CACHE: std::sync::OnceLock<(String, Result<FileConfig, String>)> =
            std::sync::OnceLock::new();

        if let Some((cached, result)) = CACHE.get()
            && cached == path
        {
            return result.clone();
        }

        let result = Self::read(path);
        let _ = CACHE.set((path.to_owned(), result.clone()));
        result
    }

    fn read(path: &str) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::parse(&raw)
    }

    fn parse(raw: &str) -> Result<Self, String> {
        toml::from_str(raw).map_err(|e| e.to_string().trim_end().to_owned())
    }

    /// the config nobody asked for by path: having none is the ordinary case, so
    /// only a config that is there and malformed is worth an error
    ///
    /// the path is spelled out in what it reports, since nothing on the command
    /// line points at the file it read
    fn load_default() -> Result<Self, String> {
        let Some(path) = Self::default_path() else {
            return Ok(Self::default());
        };

        match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
            Ok(raw) => Self::parse(&raw).map_err(|e| format!("{}: {e}", path.display())),
        }
    }

    /// XDG disregards a config home that is not absolute, and leaves the tool to
    /// fall back to `~/.config` as if it had never been set
    fn default_path() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|config_home| config_home.join(DEFAULT_PATH))
    }
}

impl Cli {
    pub(crate) fn parse_with_config() -> Self {
        // this parse only reaches the config, so whatever else is wrong with the
        // arguments is left to the parse below to report
        //
        // help is not an error `ignore_errors` covers: clap prints it and exits
        // from whichever parse handles it, and this one predates the config, so
        // the flake would read as required however the config supplies it.
        // disabled here, help falls through as an unknown argument and the parse
        // below prints what actually applies
        let file_config = Self::command()
            .ignore_errors(true)
            .disable_help_flag(true)
            .disable_help_subcommand(true)
            .get_matches()
            .get_one::<FileConfig>("config")
            .cloned()
            .unwrap_or_else(|| match FileConfig::load_default() {
                Ok(config) => config,
                Err(e) => Self::command()
                    .error(clap::error::ErrorKind::InvalidValue, e)
                    .exit(),
            });

        let mut cmd = Self::command();
        if let Some(flake) = file_config.flake {
            cmd = cmd.mut_arg("flake", |arg| arg.default_value(flake).required(false));
        }

        match <Self as clap::FromArgMatches>::from_arg_matches(&cmd.get_matches()) {
            Ok(cli) => cli,
            Err(e) => e.exit(),
        }
    }
}
