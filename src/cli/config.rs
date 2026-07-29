use clap::CommandFactory;

use crate::cli::Cli;

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
        toml::from_str(&raw).map_err(|e| e.to_string().trim_end().to_owned())
    }
}

impl Cli {
    pub(crate) fn parse_with_config() -> Self {
        let file_config = Self::command()
            .ignore_errors(true)
            .get_matches()
            .get_one::<FileConfig>("config")
            .cloned()
            .unwrap_or_default();

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
