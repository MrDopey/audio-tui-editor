//! Applying the automatic trim policy to a whole folder (design §17).
//!
//! Each file is processed independently: a failure on one is recorded and the
//! run continues. Every result stays inspectable in the final report.
//!
//! A run can be a dry run, in which detection happens exactly as it would
//! otherwise but no file is written, so the user can see what the policy would
//! do before committing to it.

mod report;

use std::sync::mpsc::Sender;

use rayon::prelude::*;

use crate::config::{AutoTrim, Config};
use crate::media::probe::{MediaInfo, SkippedFile};
use crate::media::{autotrim, ffmpeg};

pub use report::{BatchItem, BatchReport, ItemStatus, OutputFormat, Trim, CSV_HEADER};

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

    /// Same as [`label`](Self::label), but a valid JSON identifier.
    fn json_label(self) -> &'static str {
        match self {
            RunMode::Apply => "apply",
            RunMode::DryRun => "dry_run",
        }
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
    jobs: usize,
    mut emit: impl FnMut(Progress),
) -> BatchReport {
    // rayon treats 0 as "use its own default"; this run is only ever meant
    // to use as many threads as the caller asked for, one at minimum.
    let jobs = jobs.max(1);
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

    // Detection/save per file can run across up to `jobs` threads, but
    // `collect()` on an indexed parallel iterator preserves input order, so
    // results still land — and get emitted to `emit` one at a time, in
    // order — exactly as if the run were sequential (design §17: front ends
    // see one file finish at a time). `jobs == 1` skips the pool entirely,
    // so the default run is the plain sequential loop it always was.
    let base_number = number;
    let items: Vec<BatchItem> = if jobs == 1 {
        files
            .iter()
            .enumerate()
            .map(|(i, info)| process_one(base_number + i + 1, info, config, mode))
            .collect()
    } else {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build()
            .expect("building the batch thread pool");
        pool.install(|| {
            files
                .par_iter()
                .enumerate()
                .map(|(i, info)| process_one(base_number + i + 1, info, config, mode))
                .collect()
        })
    };

    for item in items {
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

    // The suggestion's begin/end are the planned cut points; only a side
    // that was actually detected gets one, per §17's "or - if unchanged".
    let new_start = suggestion.begin_detected.then_some(suggestion.begin);
    let new_end = suggestion.end_detected.then_some(suggestion.end);

    if mode.is_dry_run() {
        // Detection has run for real; only the write is withheld.
        return BatchItem {
            number,
            name,
            status: ItemStatus::WouldChange(Trim {
                old_duration: info.duration,
                new_duration: suggestion.end - suggestion.begin,
                new_start,
                new_end,
            }),
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
            status: ItemStatus::Changed(Trim {
                old_duration: outcome.source_duration,
                new_duration: outcome.output_duration,
                new_start,
                new_end,
            }),
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
    jobs: usize,
    tx: Sender<Progress>,
) {
    std::thread::spawn(move || {
        run(&files, &skipped, &config, mode, jobs, |progress| {
            // A closed channel means the UI moved on; stop reporting.
            let _ = tx.send(progress);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let report = run(&[], &skipped, &Config::default(), RunMode::Apply, 1, |_| {});
        assert_eq!(
            report.processed(),
            1,
            "the unprobable file must still count"
        );
        assert_eq!(report.skipped(), 1);
        assert_eq!(
            report.items[0].line(),
            "01 corrupt.mp3   SKIPPED: no audio stream"
        );
    }
}
