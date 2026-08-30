//! Application configuration (design §12) plus CLI override plumbing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Fields are `pub` for convenient read access throughout the app (and so
/// tests can build one with a struct literal), but the only paths that
/// should ever *construct or mutate* a `Config` are [`Config::load`] and
/// [`Config::apply`] — both call [`Config::validate`] before returning, so a
/// value read from `app`/`ui`/`batch` is always known-valid. If you find
/// yourself writing `config.playback.foo = ...` outside this module, call
/// [`Config::validate`] afterwards.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub playback: Playback,
    pub editing: Editing,
    pub auto_trim: AutoTrim,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Playback {
    pub small_seek_seconds: f64,
    pub large_seek_seconds: f64,
    /// Volume step in percentage points.
    pub volume_step: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Editing {
    pub fine_step_seconds: f64,
    pub large_step_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AutoTrim {
    pub begin_threshold_db: f64,
    pub end_threshold_db: f64,
    pub begin_min_duration: f64,
    pub end_min_duration: f64,
}

impl Default for Playback {
    fn default() -> Self {
        Playback {
            small_seek_seconds: 10.0,
            large_seek_seconds: 60.0,
            volume_step: 5.0,
        }
    }
}

impl Default for Editing {
    fn default() -> Self {
        Editing {
            fine_step_seconds: 1.0,
            large_step_seconds: 10.0,
        }
    }
}

impl Default for AutoTrim {
    fn default() -> Self {
        AutoTrim {
            begin_threshold_db: -40.0,
            end_threshold_db: -40.0,
            begin_min_duration: 3.0,
            end_min_duration: 5.0,
        }
    }
}

/// Overrides supplied on the command line, applied on top of the file config.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub small_seek_seconds: Option<f64>,
    pub large_seek_seconds: Option<f64>,
    pub volume_step: Option<f64>,
    pub fine_step_seconds: Option<f64>,
    pub large_step_seconds: Option<f64>,
    pub begin_threshold_db: Option<f64>,
    pub end_threshold_db: Option<f64>,
    pub begin_min_duration: Option<f64>,
    pub end_min_duration: Option<f64>,
}

impl Config {
    /// Read a config file. Missing files yield defaults; malformed ones error.
    pub fn load(path: Option<&Path>) -> Result<(Config, Option<PathBuf>)> {
        let candidate = match path {
            Some(p) => Some(p.to_path_buf()),
            None => default_config_path(),
        };
        let Some(path) = candidate else {
            return Ok((Config::default(), None));
        };
        if !path.exists() {
            return Ok((Config::default(), None));
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;
        config.validate()?;
        Ok((config, Some(path)))
    }

    pub fn apply(&mut self, o: &Overrides) -> Result<()> {
        macro_rules! set {
            ($($field:ident => $target:expr),* $(,)?) => {
                $(if let Some(v) = o.$field { $target = v; })*
            };
        }
        set! {
            small_seek_seconds => self.playback.small_seek_seconds,
            large_seek_seconds => self.playback.large_seek_seconds,
            volume_step => self.playback.volume_step,
            fine_step_seconds => self.editing.fine_step_seconds,
            large_step_seconds => self.editing.large_step_seconds,
            begin_threshold_db => self.auto_trim.begin_threshold_db,
            end_threshold_db => self.auto_trim.end_threshold_db,
            begin_min_duration => self.auto_trim.begin_min_duration,
            end_min_duration => self.auto_trim.end_min_duration,
        }
        self.validate()
    }

    /// Re-check every field's constraints. Called automatically by
    /// [`Config::load`] and [`Config::apply`]; exposed so any other code path
    /// that mutates a `Config` directly can re-validate afterwards too.
    pub fn validate(&self) -> Result<()> {
        let positives = [
            (
                "playback.small_seek_seconds",
                self.playback.small_seek_seconds,
            ),
            (
                "playback.large_seek_seconds",
                self.playback.large_seek_seconds,
            ),
            ("playback.volume_step", self.playback.volume_step),
            ("editing.fine_step_seconds", self.editing.fine_step_seconds),
            (
                "editing.large_step_seconds",
                self.editing.large_step_seconds,
            ),
        ];
        for (name, value) in positives {
            anyhow::ensure!(
                value > 0.0,
                "{name} must be greater than zero (got {value})"
            );
        }
        let non_negatives = [
            (
                "auto_trim.begin_min_duration",
                self.auto_trim.begin_min_duration,
            ),
            (
                "auto_trim.end_min_duration",
                self.auto_trim.end_min_duration,
            ),
        ];
        for (name, value) in non_negatives {
            anyhow::ensure!(value >= 0.0, "{name} must not be negative (got {value})");
        }
        for (name, value) in [
            (
                "auto_trim.begin_threshold_db",
                self.auto_trim.begin_threshold_db,
            ),
            (
                "auto_trim.end_threshold_db",
                self.auto_trim.end_threshold_db,
            ),
        ] {
            anyhow::ensure!(
                value <= 0.0,
                "{name} is measured in dBFS and must not be positive (got {value})"
            );
        }
        Ok(())
    }
}

/// `$XDG_CONFIG_HOME/audioedit/config.toml` (or the platform equivalent).
pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("audioedit").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_values() {
        let c = Config::default();
        assert_eq!(c.playback.small_seek_seconds, 10.0);
        assert_eq!(c.playback.large_seek_seconds, 60.0);
        assert_eq!(c.playback.volume_step, 5.0);
        assert_eq!(c.editing.fine_step_seconds, 1.0);
        assert_eq!(c.editing.large_step_seconds, 10.0);
        assert_eq!(c.auto_trim.begin_threshold_db, -40.0);
        assert_eq!(c.auto_trim.end_threshold_db, -40.0);
        assert_eq!(c.auto_trim.begin_min_duration, 3.0);
        assert_eq!(c.auto_trim.end_min_duration, 5.0);
    }

    #[test]
    fn parses_the_documented_config_file() {
        let text = r#"
[playback]
small_seek_seconds = 10
large_seek_seconds = 60
volume_step = 5

[editing]
fine_step_seconds = 1
large_step_seconds = 10

[auto_trim]
begin_threshold_db = -40
end_threshold_db = -40
begin_min_duration = 3
end_min_duration = 5
"#;
        let parsed: Config = toml::from_str(text).unwrap();
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn partial_config_keeps_defaults() {
        let parsed: Config = toml::from_str("[auto_trim]\nbegin_threshold_db = -30\n").unwrap();
        assert_eq!(parsed.auto_trim.begin_threshold_db, -30.0);
        assert_eq!(parsed.auto_trim.end_threshold_db, -40.0);
        assert_eq!(parsed.playback.small_seek_seconds, 10.0);
    }

    #[test]
    fn overrides_apply_independently() {
        let mut c = Config::default();
        c.apply(&Overrides {
            begin_threshold_db: Some(-30.0),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(c.auto_trim.begin_threshold_db, -30.0);
        assert_eq!(c.auto_trim.end_threshold_db, -40.0);
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut c = Config::default();
        assert!(c
            .apply(&Overrides {
                small_seek_seconds: Some(0.0),
                ..Default::default()
            })
            .is_err());
        let mut c = Config::default();
        assert!(c
            .apply(&Overrides {
                begin_threshold_db: Some(12.0),
                ..Default::default()
            })
            .is_err());
    }
}
