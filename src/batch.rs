//! Applying the automatic trim policy to a whole folder (design §17).
//!
//! Each file is processed independently: a failure on one is recorded and the
//! run continues. Every result stays inspectable in the final report.
//!
//! A run can be a dry run, in which detection happens exactly as it would
//! otherwise but no file is written, so the user can see what the policy would
//! do before committing to it.

use std::sync::mpsc::Sender;

use crate::config::{AutoTrim, Config};
use crate::media::probe::{MediaInfo, SkippedFile};
use crate::media::{autotrim, ffmpeg, first_line};
use crate::timespec::format_timestamp;

/// The confirmation text shown before a folder-wide run (design §17), shared
/// by the CLI's interactive prompt and the TUI's confirmation overlay so the
/// two front ends can never describe the same run differently. Each caller
/// appends its own footer (a keyboard hint, or an interactive prompt).
pub fn confirmation_lines(count: usize, skipped_count: usize, auto: &AutoTrim) -> Vec<String> {
    let mut lines = vec![
        format!("Apply automatic trim to {count} files?"),
        String::new(),
        "Threshold:".to_string(),
        format!("  begin {} dB", auto.begin_threshold_db),
        format!("  end   {} dB", auto.end_threshold_db),
        String::new(),
        "Minimum duration:".to_string(),
        format!("  begin {}s", auto.begin_min_duration),
        format!("  end   {}s", auto.end_min_duration),
        String::new(),
        "Folder contents are rewritten in place.".to_string(),
        "A dry run reports what would change without writing anything.".to_string(),
    ];
    if skipped_count > 0 {
        lines.push(String::new());
        lines.push(format!(
            "{skipped_count} file(s) could not be read and will be reported as skipped."
        ));
    }
    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Files are rewritten in place.
    Apply,
    /// Nothing is written; results describe what would happen.
    DryRun,
}

impl RunMode {
    pub fn is_dry_run(self) -> bool {
        self == RunMode::DryRun
    }

    pub fn label(self) -> &'static str {
        match self {
            RunMode::Apply => "apply",
            RunMode::DryRun => "dry run",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemStatus {
    /// The file was rewritten.
    Changed {
        from: f64,
        to: f64,
    },
    /// The file would be rewritten, but this was a dry run.
    WouldChange {
        from: f64,
        to: f64,
    },
    NoOp,
    Failed(String),
    Skipped(String),
}

#[derive(Debug, Clone)]
pub struct BatchItem {
    /// 1-based position in the run, matching the report listing.
    pub number: usize,
    pub name: String,
    pub status: ItemStatus,
}

impl BatchItem {
    /// The per-file line shown in the report (design §17).
    pub fn line(&self) -> String {
        match &self.status {
            ItemStatus::Changed { from, to } => format!(
                "{:02} {}   {} → {}",
                self.number,
                self.name,
                format_timestamp(*from),
                format_timestamp(*to)
            ),
            ItemStatus::WouldChange { from, to } => format!(
                "{:02} {}   {} → {}  (would change)",
                self.number,
                self.name,
                format_timestamp(*from),
                format_timestamp(*to)
            ),
            ItemStatus::NoOp => format!("{:02} {}   NO-OP", self.number, self.name),
            ItemStatus::Failed(err) => {
                format!(
                    "{:02} {}   FAILED: {}",
                    self.number,
                    self.name,
                    first_line(err)
                )
            }
            ItemStatus::Skipped(why) => {
                format!(
                    "{:02} {}   SKIPPED: {}",
                    self.number,
                    self.name,
                    first_line(why)
                )
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct BatchReport {
    pub items: Vec<BatchItem>,
    pub mode: RunMode,
}

impl BatchReport {
    pub fn new(mode: RunMode) -> Self {
        BatchReport {
            items: Vec::new(),
            mode,
        }
    }

    pub fn processed(&self) -> usize {
        self.items.len()
    }

    /// Files rewritten, or that would be rewritten in a dry run.
    pub fn changed(&self) -> usize {
        self.count(|s| {
            matches!(
                s,
                ItemStatus::Changed { .. } | ItemStatus::WouldChange { .. }
            )
        })
    }

    pub fn noop(&self) -> usize {
        self.count(|s| matches!(s, ItemStatus::NoOp))
    }

    pub fn failed(&self) -> usize {
        self.count(|s| matches!(s, ItemStatus::Failed(_)))
    }

    pub fn skipped(&self) -> usize {
        self.count(|s| matches!(s, ItemStatus::Skipped(_)))
    }

    fn count(&self, predicate: impl Fn(&ItemStatus) -> bool) -> usize {
        self.items.iter().filter(|i| predicate(&i.status)).count()
    }

    /// The summary block from design §17.
    pub fn summary_lines(&self) -> Vec<String> {
        let changed_label = if self.mode.is_dry_run() {
            "Would change:"
        } else {
            "Changed:  "
        };
        let mut lines = vec![
            format!("Processed: {}", self.processed()),
            format!("{changed_label} {}", self.changed()),
            format!("No-op:     {}", self.noop()),
            format!("Failed:    {}", self.failed()),
            format!("Skipped:   {}", self.skipped()),
        ];
        if self.mode.is_dry_run() {
            lines.push(String::new());
            lines.push("DRY RUN — no files were modified.".to_string());
        }
        lines
    }
}

/// Progress messages emitted while a run is in flight.
#[derive(Debug, Clone)]
pub enum Progress {
    Started { total: usize, mode: RunMode },
    Item(BatchItem),
    Finished(BatchReport),
}

/// Process every file, reporting progress through `emit`.
///
/// `skipped` are candidates that were found while scanning the folder but
/// could not be probed at all (design §17: every file must be accounted for
/// in the report, not just the ones that were successfully opened).
pub fn run(
    files: &[MediaInfo],
    skipped: &[SkippedFile],
    config: &Config,
    mode: RunMode,
    mut emit: impl FnMut(Progress),
) -> BatchReport {
    emit(Progress::Started {
        total: files.len() + skipped.len(),
        mode,
    });
    let mut report = BatchReport::new(mode);
    let mut number = 0;

    for file in skipped {
        number += 1;
        let item = BatchItem {
            number,
            name: file.name.clone(),
            status: ItemStatus::Skipped(file.reason.clone()),
        };
        report.items.push(item.clone());
        emit(Progress::Item(item));
    }

    for info in files {
        number += 1;
        let item = process_one(number, info, config, mode);
        report.items.push(item.clone());
        emit(Progress::Item(item));
    }

    emit(Progress::Finished(report.clone()));
    report
}

fn process_one(number: usize, info: &MediaInfo, config: &Config, mode: RunMode) -> BatchItem {
    let name = info.file_name();

    if info.duration <= 0.0 {
        return BatchItem {
            number,
            name,
            status: ItemStatus::Skipped("no measurable duration".to_string()),
        };
    }

    let suggestion = match autotrim::detect(&info.path, info.duration, &config.auto_trim) {
        Ok(s) => s,
        Err(err) => {
            return BatchItem {
                number,
                name,
                status: ItemStatus::Failed(err.to_string()),
            }
        }
    };

    if !suggestion.begin_detected && !suggestion.end_detected {
        return BatchItem {
            number,
            name,
            status: ItemStatus::NoOp,
        };
    }

    if mode.is_dry_run() {
        // Detection has run for real; only the write is withheld.
        return BatchItem {
            number,
            name,
            status: ItemStatus::WouldChange {
                from: info.duration,
                to: suggestion.end - suggestion.begin,
            },
        };
    }

    match ffmpeg::save(
        info,
        &ffmpeg::SaveRequest::trim(suggestion.begin, suggestion.end),
    ) {
        Ok(outcome) if outcome.noop => BatchItem {
            number,
            name,
            status: ItemStatus::NoOp,
        },
        Ok(outcome) => BatchItem {
            number,
            name,
            status: ItemStatus::Changed {
                from: outcome.source_duration,
                to: outcome.output_duration,
            },
        },
        Err(err) => BatchItem {
            number,
            name,
            status: ItemStatus::Failed(format!("{err:#}")),
        },
    }
}

/// Run a batch on a worker thread, streaming progress to `tx`.
pub fn spawn(
    files: Vec<MediaInfo>,
    skipped: Vec<SkippedFile>,
    config: Config,
    mode: RunMode,
    tx: Sender<Progress>,
) {
    std::thread::spawn(move || {
        run(&files, &skipped, &config, mode, |progress| {
            // A closed channel means the UI moved on; stop reporting.
            let _ = tx.send(progress);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(number: usize, name: &str, status: ItemStatus) -> BatchItem {
        BatchItem {
            number,
            name: name.to_string(),
            status,
        }
    }

    #[test]
    fn counts_every_outcome_category() {
        let report = BatchReport {
            mode: RunMode::Apply,
            items: vec![
                item(
                    1,
                    "a.opus",
                    ItemStatus::Changed {
                        from: 151.0,
                        to: 148.0,
                    },
                ),
                item(2, "b.opus", ItemStatus::NoOp),
                item(3, "c.opus", ItemStatus::Failed("ffmpeg exploded".into())),
                item(4, "d.opus", ItemStatus::Skipped("empty".into())),
                item(
                    5,
                    "e.opus",
                    ItemStatus::Changed {
                        from: 10.0,
                        to: 9.0,
                    },
                ),
            ],
        };
        assert_eq!(report.processed(), 5);
        assert_eq!(report.changed(), 2);
        assert_eq!(report.noop(), 1);
        assert_eq!(report.failed(), 1);
        assert_eq!(report.skipped(), 1);
        assert_eq!(report.summary_lines()[0], "Processed: 5");
    }

    #[test]
    fn per_file_lines_match_the_documented_shape() {
        let changed = item(
            1,
            "interview-001.opus",
            ItemStatus::Changed {
                from: 151.0,
                to: 148.0,
            },
        );
        assert_eq!(changed.line(), "01 interview-001.opus   02:31 → 02:28");
        let noop = item(2, "interview-002.opus", ItemStatus::NoOp);
        assert_eq!(noop.line(), "02 interview-002.opus   NO-OP");
    }

    #[test]
    fn failure_lines_stay_on_one_line() {
        let failed = item(3, "x.opus", ItemStatus::Failed("boom\nwith detail".into()));
        assert_eq!(failed.line(), "03 x.opus   FAILED: boom");
    }

    #[test]
    fn an_empty_run_reports_zeroes() {
        let report = BatchReport::new(RunMode::Apply);
        assert_eq!(report.processed(), 0);
        assert_eq!(report.summary_lines().len(), 5);
    }

    #[test]
    fn dry_run_items_are_marked_and_counted() {
        let report = BatchReport {
            mode: RunMode::DryRun,
            items: vec![
                item(
                    1,
                    "a.opus",
                    ItemStatus::WouldChange {
                        from: 151.0,
                        to: 148.0,
                    },
                ),
                item(2, "b.opus", ItemStatus::NoOp),
            ],
        };
        assert_eq!(report.changed(), 1);
        assert_eq!(
            report.items[0].line(),
            "01 a.opus   02:31 → 02:28  (would change)"
        );
        let summary = report.summary_lines();
        assert!(summary[1].starts_with("Would change:"));
        assert!(summary.iter().any(|l| l.contains("no files were modified")));
    }

    #[test]
    fn apply_mode_summary_does_not_mention_dry_run() {
        let report = BatchReport::new(RunMode::Apply);
        assert!(!report.summary_lines().iter().any(|l| l.contains("DRY RUN")));
        assert!(report.summary_lines()[1].starts_with("Changed:"));
    }

    #[test]
    fn mode_labels_are_stable() {
        assert_eq!(RunMode::Apply.label(), "apply");
        assert_eq!(RunMode::DryRun.label(), "dry run");
        assert!(RunMode::DryRun.is_dry_run());
        assert!(!RunMode::Apply.is_dry_run());
    }

    #[test]
    fn confirmation_lines_report_thresholds_and_skipped_files() {
        let lines = confirmation_lines(43, 2, &Config::default().auto_trim);
        assert_eq!(lines[0], "Apply automatic trim to 43 files?");
        assert!(lines.iter().any(|l| l.contains("begin -40 dB")));
        assert!(lines.iter().any(|l| l.contains("rewritten in place")));
        assert!(lines
            .iter()
            .any(|l| l.contains("2 file(s) could not be read")));
    }

    #[test]
    fn confirmation_lines_omit_the_skipped_note_when_there_is_nothing_to_skip() {
        let lines = confirmation_lines(1, 0, &Config::default().auto_trim);
        assert!(!lines.iter().any(|l| l.contains("could not be read")));
    }

    #[test]
    fn unprobable_files_are_counted_as_skipped_not_dropped() {
        let skipped = vec![SkippedFile {
            name: "corrupt.mp3".to_string(),
            reason: "no audio stream".to_string(),
        }];
        let report = run(&[], &skipped, &Config::default(), RunMode::Apply, |_| {});
        assert_eq!(report.processed(), 1, "the unprobable file must still count");
        assert_eq!(report.skipped(), 1);
        assert_eq!(report.items[0].line(), "01 corrupt.mp3   SKIPPED: no audio stream");
    }
}
