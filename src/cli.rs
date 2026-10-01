//! Command line parsing.

use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};

use crate::core::text;

/// Icon set used for tree rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum IconStyle {
    /// Plain glyphs that render in any terminal font.
    Text,
    /// Nerd Font glyphs.
    Nerd,
}

/// Which content the diff is compared against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BaselineKind {
    /// `GitHead` inside a repository, `Snapshot` outside of one.
    Auto,
    /// The `HEAD` commit of the repository.
    Head,
    /// The contents at startup.
    Snapshot,
}

#[derive(Debug, Parser)]
#[command(
    name = "yagura",
    version,
    about = "Watch what a coding agent is changing, from the terminal",
    long_about = None,
)]
pub struct Cli {
    /// Directory to watch. Defaults to the current directory.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Icons shown in front of tree rows.
    #[arg(long, value_enum, default_value_t = IconStyle::Text)]
    pub icons: IconStyle,

    /// Which content the diff is compared against.
    #[arg(long, value_enum, default_value_t = BaselineKind::Auto)]
    pub baseline: BaselineKind,

    /// Show files that `.gitignore` excludes.
    #[arg(long)]
    pub no_ignore: bool,

    /// Write tracing logs to this file. `YAGURA_LOG` is used when omitted.
    #[arg(long, value_name = "PATH")]
    pub log: Option<PathBuf>,

    /// Panic on purpose after the terminal is set up. Used to check that the
    /// terminal is restored on the panic path.
    #[arg(long, hide = true)]
    pub selftest_panic: bool,
}

/// Everything the application needs to start, validated and canonicalized.
#[derive(Debug, Clone)]
pub struct Config {
    /// Absolute, canonical root that is watched.
    pub root: PathBuf,
    pub icons: IconStyle,
    pub baseline: BaselineKind,
    /// False when `--no-ignore` was given.
    pub ignore_enabled: bool,
    /// True when `NO_COLOR` is set.
    pub no_color: bool,
    /// File the tracing logs are written to. `None` disables logging.
    pub log: Option<PathBuf>,
    pub selftest_panic: bool,
}

impl Config {
    /// Validates the parsed arguments.
    pub fn from_args(cli: Cli) -> anyhow::Result<Self> {
        let requested = match cli.path {
            Some(path) => path,
            None => std::env::current_dir().map_err(|err| {
                anyhow::anyhow!(
                    "cannot read the working directory: {}",
                    text::os_error(&err)
                )
            })?,
        };
        // The reason is rendered by hand, because the operating system text is
        // not in English everywhere and a startup error is the first thing a
        // user reads.
        let root = std::fs::canonicalize(&requested).map_err(|err| {
            anyhow::anyhow!(
                "cannot open {}: {}",
                requested.display(),
                text::os_error(&err)
            )
        })?;
        let root = strip_verbatim_prefix(&root);
        anyhow::ensure!(root.is_dir(), "{} is not a directory", requested.display());
        Ok(Self {
            root,
            icons: cli.icons,
            baseline: cli.baseline,
            ignore_enabled: !cli.no_ignore,
            no_color: std::env::var_os("NO_COLOR").is_some(),
            log: cli.log,
            selftest_panic: cli.selftest_panic,
        })
    }
}

/// Turns the `\\?\` prefix that `canonicalize` adds on Windows back into a
/// path that is pleasant to display.
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy().into_owned();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
        path.to_path_buf()
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<Config> {
        Config::from_args(Cli::try_parse_from(args).expect("parse"))
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_to_text_icons_and_auto_baseline() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = parse(&["yagura", dir.path().to_str().expect("utf8")]).expect("config");
        assert_eq!(config.icons, IconStyle::Text);
        assert_eq!(config.baseline, BaselineKind::Auto);
        assert!(config.ignore_enabled);
        assert!(!config.no_color);
    }

    #[test]
    fn no_ignore_disables_the_ignore_rules() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = parse(&[
            "yagura",
            "--no-ignore",
            "--icons",
            "nerd",
            "--baseline",
            "snapshot",
            dir.path().to_str().expect("utf8"),
        ])
        .expect("config");
        assert!(!config.ignore_enabled);
        assert_eq!(config.icons, IconStyle::Nerd);
        assert_eq!(config.baseline, BaselineKind::Snapshot);
    }

    #[test]
    fn rejects_an_unknown_baseline() {
        assert!(Cli::try_parse_from(["yagura", "--baseline", "nope"]).is_err());
        assert!(Cli::try_parse_from(["yagura", "--icons", "emoji"]).is_err());
    }

    #[test]
    fn rejects_a_file_as_root() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").expect("write");
        assert!(parse(&["yagura", file.to_str().expect("utf8")]).is_err());
    }

    #[test]
    fn resolves_a_relative_root_to_an_absolute_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).expect("mkdir");
        let config =
            parse(&["yagura", sub.join("..").join("sub").to_str().expect("utf8")]).expect("config");
        assert!(config.root.is_absolute());
        assert!(config.root.ends_with("sub"));
    }

    #[test]
    fn a_missing_root_is_reported_in_english() {
        let dir = tempfile::tempdir().expect("temp dir");
        let missing = dir.path().join("nope");
        let err = parse(&["yagura", missing.to_str().expect("utf8")])
            .expect_err("a missing root is an error")
            .to_string();
        assert!(err.contains("cannot open"), "{err}");
        assert!(err.contains("not found"), "{err}");
    }
}
