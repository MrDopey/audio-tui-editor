//! Command line interface (design §2).

use std::path::PathBuf;

use clap::Parser;

use crate::config::Overrides;

#[derive(Debug, Parser)]
#[command(
    name = "audioedit",
    version,
    about = "Vim-like terminal audio editor: browse, play, trim and edit metadata",
    long_about = "A keyboard-driven terminal editor for audio files.\n\n\
                  Saving is an in-place operation: a temporary output is produced and \
                  verified before the original file is replaced."
)]
pub struct Cli {
    /// Folder to work in (defaults to the current directory).
    #[arg(long, value_name = "PATH")]
    pub folder: Option<PathBuf>,

    /// Configuration file to read instead of the default location.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Apply the automatic trim policy to every supported file in the folder.
    #[arg(long)]
    pub apply_defaults: bool,

    /// Report what --apply-defaults would do without modifying any file.
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Skip the confirmation prompt before a folder-wide run.
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Do not open an audio device (browsing and editing still work).
    #[arg(long)]
    pub no_audio: bool,

    /// Seek amount for the arrow keys and h/l.
    #[arg(long, value_name = "SECONDS", help_heading = "Playback")]
    pub small_seek_seconds: Option<f64>,

    /// Seek amount for Ctrl-arrow and Ctrl-h/Ctrl-l.
    #[arg(long, value_name = "SECONDS", help_heading = "Playback")]
    pub large_seek_seconds: Option<f64>,

    /// Percentage points added or removed by each volume key.
    #[arg(long, value_name = "PERCENT", help_heading = "Playback")]
    pub volume_step: Option<f64>,

    /// Marker movement for the arrow keys and h/l in EDIT mode.
    #[arg(long, value_name = "SECONDS", help_heading = "Editing")]
    pub fine_step_seconds: Option<f64>,

    /// Marker movement for Ctrl-arrow and Ctrl-h/Ctrl-l in EDIT mode.
    #[arg(long, value_name = "SECONDS", help_heading = "Editing")]
    pub large_step_seconds: Option<f64>,

    /// Level below which leading audio counts as silence, in dBFS.
    #[arg(
        long,
        value_name = "DB",
        allow_negative_numbers = true,
        help_heading = "Automatic trim"
    )]
    pub begin_threshold_db: Option<f64>,

    /// Level below which trailing audio counts as silence, in dBFS.
    #[arg(
        long,
        value_name = "DB",
        allow_negative_numbers = true,
        help_heading = "Automatic trim"
    )]
    pub end_threshold_db: Option<f64>,

    /// How long the leading silence must last to be trimmed, in seconds.
    #[arg(long, value_name = "SECONDS", help_heading = "Automatic trim")]
    pub begin_min_duration: Option<f64>,

    /// How long the trailing silence must last to be trimmed, in seconds.
    #[arg(long, value_name = "SECONDS", help_heading = "Automatic trim")]
    pub end_min_duration: Option<f64>,
}

impl Cli {
    pub fn overrides(&self) -> Overrides {
        Overrides {
            small_seek_seconds: self.small_seek_seconds,
            large_seek_seconds: self.large_seek_seconds,
            volume_step: self.volume_step,
            fine_step_seconds: self.fine_step_seconds,
            large_step_seconds: self.large_step_seconds,
            begin_threshold_db: self.begin_threshold_db,
            end_threshold_db: self.end_threshold_db,
            begin_min_duration: self.begin_min_duration,
            end_min_duration: self.end_min_duration,
        }
    }

    /// True when the run should process the whole folder without the TUI.
    /// `--dry-run` on its own implies a folder-wide dry run.
    pub fn is_batch_run(&self) -> bool {
        self.apply_defaults || self.dry_run
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("audioedit").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_to_the_current_directory_and_the_tui() {
        let cli = parse(&[]);
        assert!(cli.folder.is_none());
        assert!(!cli.is_batch_run());
    }

    #[test]
    fn parses_the_documented_override_example() {
        let cli = parse(&[
            "--folder",
            "/recordings",
            "--begin-threshold-db",
            "-40",
            "--end-threshold-db",
            "-40",
            "--begin-min-duration",
            "1",
            "--end-min-duration",
            "1",
        ]);
        assert_eq!(cli.folder.unwrap(), PathBuf::from("/recordings"));
        let overrides = Cli::overrides(&parse(&["--begin-threshold-db", "-40"]));
        assert_eq!(overrides.begin_threshold_db, Some(-40.0));
    }

    #[test]
    fn apply_defaults_is_a_batch_run() {
        assert!(parse(&["--apply-defaults"]).is_batch_run());
    }

    #[test]
    fn dry_run_implies_a_batch_run() {
        let cli = parse(&["--dry-run"]);
        assert!(cli.is_batch_run());
        assert!(cli.dry_run);
    }

    #[test]
    fn dry_run_has_a_short_flag() {
        assert!(parse(&["--apply-defaults", "-n"]).dry_run);
    }

    #[test]
    fn overrides_are_independent() {
        let overrides = parse(&["--begin-min-duration", "2"]).overrides();
        assert_eq!(overrides.begin_min_duration, Some(2.0));
        assert_eq!(overrides.end_min_duration, None);
    }
}
