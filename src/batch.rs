//! Applying the automatic trim policy to a whole folder (design §17).
//!
//! Each file is processed independently: a failure on one is recorded and the
//! run continues. Every result stays inspectable in the final report.
//!
//! A run can be a dry run, in which detection happens exactly as it would
//! otherwise but no file is written, so the user can see what the policy would
//! do before committing to it.

use std::sync::mpsc::Sender;

use clap::ValueEnum;
use serde_json::json;

use crate::config::{AutoTrim, Config};
use crate::media::probe::{MediaInfo, SkippedFile};
use crate::media::{autotrim, ffmpeg, first_line};
use crate::timespec::format_timestamp;

/// Longest a name gets to be in `--format table` before it is trimmed with an
/// ellipsis; `table-full` never trims.
const TABLE_MAX_NAME_WIDTH: usize = 40;

/// The header row for `--format csv`.
pub const CSV_HEADER: &str = "number,name,status,new_start_seconds,new_end_seconds,\
old_duration_seconds,new_duration_seconds,trimmed_seconds,reason";

/// Quote a CSV field per RFC 4180 if it contains a comma, quote, or newline.
fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// How a headless run (`--dry-run` / `--apply-defaults`) is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Columns aligned; long names are trimmed to fit (the default).
    Table,
    /// Columns aligned; names are never trimmed.
    TableFull,
    /// The whole run as a single minified JSON document, one line.
    Json,
    /// Same document as `json`, pretty-printed for a human to read.
    JsonFull,
    /// One CSV row per file, streamed as it completes.
    Csv,
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Table
    }
}

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

/// The before/after shape of a trim, broken out per side so a report can
/// say exactly what moved rather than just the overall duration change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trim {
    pub old_duration: f64,
    pub new_duration: f64,
    /// The new start time, or `None` when the beginning was not trimmed.
    pub new_start: Option<f64>,
    /// The new end time, or `None` when the end was not trimmed.
    pub new_end: Option<f64>,
}

impl Trim {
    /// How much runtime the trim removes. Always positive: a `Trim` only
    /// exists when at least one side was actually detected.
    pub fn trimmed(&self) -> f64 {
        self.old_duration - self.new_duration
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemStatus {
    /// The file was rewritten.
    Changed(Trim),
    /// The file would be rewritten, but this was a dry run.
    WouldChange(Trim),
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

/// The fields a report can show about one file, pulled out of [`ItemStatus`]
/// once so `line`, `csv_row`, `to_json` and the table renderer can never
/// disagree about what a status means. A field that does not apply to this
/// row (durations on a failure, a side of the trim that wasn't touched) is
/// `None`; every renderer turns that into its own "not applicable" spelling.
struct Fields<'a> {
    status: &'static str,
    new_start: Option<f64>,
    new_end: Option<f64>,
    old_duration: Option<f64>,
    new_duration: Option<f64>,
    trimmed: Option<f64>,
    reason: Option<&'a str>,
}

impl BatchItem {
    fn fields(&self) -> Fields<'_> {
        let of_trim = |status, trim: &Trim| Fields {
            status,
            new_start: trim.new_start,
            new_end: trim.new_end,
            old_duration: Some(trim.old_duration),
            new_duration: Some(trim.new_duration),
            trimmed: Some(trim.trimmed()),
            reason: None,
        };
        match &self.status {
            ItemStatus::Changed(trim) => of_trim("changed", trim),
            ItemStatus::WouldChange(trim) => of_trim("would_change", trim),
            ItemStatus::NoOp => Fields {
                status: "no_op",
                new_start: None,
                new_end: None,
                old_duration: None,
                new_duration: None,
                trimmed: None,
                reason: None,
            },
            ItemStatus::Failed(err) => Fields {
                status: "failed",
                new_start: None,
                new_end: None,
                old_duration: None,
                new_duration: None,
                trimmed: None,
                reason: Some(err),
            },
            ItemStatus::Skipped(why) => Fields {
                status: "skipped",
                new_start: None,
                new_end: None,
                old_duration: None,
                new_duration: None,
                trimmed: None,
                reason: Some(why),
            },
        }
    }

    /// The per-file line shown in the report (design §17).
    pub fn line(&self) -> String {
        format!("{:02} {}   {}", self.number, self.name, self.detail())
    }

    /// Everything after the name: what happened, or would happen, to a file.
    fn detail(&self) -> String {
        let f = self.fields();
        match &self.status {
            ItemStatus::Changed(_) | ItemStatus::WouldChange(_) => format!(
                "{} → {}  (-{})",
                format_timestamp(f.old_duration.unwrap_or(0.0)),
                format_timestamp(f.new_duration.unwrap_or(0.0)),
                format_timestamp(f.trimmed.unwrap_or(0.0))
            ),
            ItemStatus::NoOp => "NO-OP".to_string(),
            ItemStatus::Failed(err) => format!("FAILED: {}", first_line(err)),
            ItemStatus::Skipped(why) => format!("SKIPPED: {}", first_line(why)),
        }
    }

    /// `name`, shortened to `width` with a trailing ellipsis if it doesn't
    /// fit. `width: None` (`table-full`) never shortens.
    fn table_name(&self, width: Option<usize>) -> String {
        match width {
            Some(width) if self.name.chars().count() > width => {
                let keep = width.saturating_sub(1);
                format!("{}…", self.name.chars().take(keep).collect::<String>())
            }
            _ => self.name.clone(),
        }
    }

    /// One row for `--format table`/`table-full`: number, name, then every
    /// [`Fields`] value as its own column (`-` where it doesn't apply), so a
    /// reader can see exactly which side of a file moved.
    fn table_row(&self, max_name_width: Option<usize>, number_width: usize) -> [String; 9] {
        let f = self.fields();
        let ts = |v: Option<f64>| v.map(format_timestamp).unwrap_or_else(|| "-".to_string());
        [
            format!("{:0number_width$}", self.number),
            self.table_name(max_name_width),
            ts(f.new_start),
            ts(f.new_end),
            ts(f.old_duration),
            ts(f.new_duration),
            ts(f.trimmed),
            f.status.to_string(),
            f.reason.map(first_line).unwrap_or("-").to_string(),
        ]
    }

    /// One row matching [`CSV_HEADER`] (RFC 4180 quoting for the name and
    /// the failure/skip reason, the only fields that can contain a comma). A
    /// field that does not apply to a row reads as `-` rather than being
    /// left blank, so every row has the same shape at a glance.
    pub fn csv_row(&self) -> String {
        let f = self.fields();
        let secs = |v: Option<f64>| v.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string());
        format!(
            "{},{},{},{},{},{},{},{},{}",
            self.number,
            csv_field(&self.name),
            f.status,
            secs(f.new_start),
            secs(f.new_end),
            secs(f.old_duration),
            secs(f.new_duration),
            secs(f.trimmed),
            csv_field(f.reason.unwrap_or("-")),
        )
    }

    fn to_json(&self) -> serde_json::Value {
        let f = self.fields();
        json!({
            "number": self.number,
            "name": self.name,
            "status": f.status,
            "new_start_seconds": f.new_start,
            "new_end_seconds": f.new_end,
            "old_duration_seconds": f.old_duration,
            "new_duration_seconds": f.new_duration,
            "trimmed_seconds": f.trimmed,
            "reason": f.reason,
        })
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

    /// Render the whole report (rows and summary) for `--format`. `Csv` is
    /// only used here for the empty-folder and no-files-found cases; the
    /// normal csv run streams [`BatchItem::csv_row`] as it goes instead, so
    /// this must stay behaviourally identical to that streamed output.
    pub fn render(&self, format: OutputFormat) -> String {
        match format {
            OutputFormat::Table => self.render_table(Some(TABLE_MAX_NAME_WIDTH)),
            OutputFormat::TableFull => self.render_table(None),
            OutputFormat::Json => self.render_json(false),
            OutputFormat::JsonFull => self.render_json(true),
            OutputFormat::Csv => self.render_csv(),
        }
    }

    fn render_csv(&self) -> String {
        let mut lines = vec![CSV_HEADER.to_string()];
        lines.extend(self.items.iter().map(BatchItem::csv_row));
        lines.push(String::new());
        lines.extend(self.summary_lines());
        lines.join("\n")
    }

    /// Rows spaced out into aligned columns, so a page of results reads as a
    /// table rather than a ragged list of file names. Each of a trim's
    /// values gets its own column instead of being folded into one string,
    /// so a header row is printed first to label them.
    fn render_table(&self, max_name_width: Option<usize>) -> String {
        const HEADERS: [&str; 9] = [
            "#",
            "NAME",
            "NEW START",
            "NEW END",
            "OLD DURATION",
            "NEW DURATION",
            "TRIMMED",
            "STATUS",
            "NOTE",
        ];
        let number_width = self.items.len().to_string().len().max(2);
        let rows: Vec<[String; 9]> = self
            .items
            .iter()
            .map(|item| item.table_row(max_name_width, number_width))
            .collect();

        let mut widths: [usize; 9] = HEADERS.map(str::len);
        for row in &rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.chars().count());
            }
        }
        let format_row = |cells: &[String; 9]| -> String {
            cells
                .iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_string()
        };

        let mut lines = vec![format_row(&HEADERS.map(String::from))];
        lines.extend(rows.iter().map(format_row));
        lines.push(String::new());
        lines.extend(self.summary_lines());
        lines.join("\n")
    }

    fn render_json(&self, pretty: bool) -> String {
        let items: Vec<serde_json::Value> = self.items.iter().map(BatchItem::to_json).collect();
        let value = json!({
            "mode": self.mode.json_label(),
            "processed": self.processed(),
            "changed": self.changed(),
            "noop": self.noop(),
            "failed": self.failed(),
            "skipped": self.skipped(),
            "dry_run": self.mode.is_dry_run(),
            "items": items,
        });
        if pretty {
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
        } else {
            serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
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

    /// A trim with only the end moved (start left at 0, i.e. undetected) —
    /// the shape most existing tests don't care about beyond the durations.
    fn trim(old_duration: f64, new_duration: f64) -> Trim {
        Trim {
            old_duration,
            new_duration,
            new_start: None,
            new_end: Some(new_duration),
        }
    }

    #[test]
    fn counts_every_outcome_category() {
        let report = BatchReport {
            mode: RunMode::Apply,
            items: vec![
                item(1, "a.opus", ItemStatus::Changed(trim(151.0, 148.0))),
                item(2, "b.opus", ItemStatus::NoOp),
                item(3, "c.opus", ItemStatus::Failed("ffmpeg exploded".into())),
                item(4, "d.opus", ItemStatus::Skipped("empty".into())),
                item(5, "e.opus", ItemStatus::Changed(trim(10.0, 9.0))),
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
            ItemStatus::Changed(trim(151.0, 148.0)),
        );
        assert_eq!(
            changed.line(),
            "01 interview-001.opus   02:31 → 02:28  (-00:03)"
        );
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
                item(1, "a.opus", ItemStatus::WouldChange(trim(151.0, 148.0))),
                item(2, "b.opus", ItemStatus::NoOp),
            ],
        };
        assert_eq!(report.changed(), 1);
        assert_eq!(report.items[0].line(), "01 a.opus   02:31 → 02:28  (-00:03)");
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

    #[test]
    fn a_side_that_was_not_trimmed_reads_as_a_dash_everywhere() {
        // Only the beginning moved; the end was left exactly where it was.
        let trim = Trim {
            old_duration: 100.0,
            new_duration: 97.0,
            new_start: Some(3.0),
            new_end: None,
        };
        let report = BatchReport {
            mode: RunMode::DryRun,
            items: vec![item(1, "a.opus", ItemStatus::WouldChange(trim))],
        };

        // Columns: [number, name, new_start, new_end, old_duration,
        // new_duration, trimmed, status, note].
        let columns = report.items[0].table_row(None, 2);
        assert_eq!(columns[2], "00:03", "new start: {columns:?}");
        assert_eq!(columns[3], "-", "new end should be a dash: {columns:?}");

        let csv = report.render(OutputFormat::Csv);
        let fields: Vec<&str> = csv.lines().nth(1).unwrap().split(',').collect();
        assert_eq!(fields[3], "3"); // new_start_seconds
        assert_eq!(fields[4], "-"); // new_end_seconds

        let json = report.render(OutputFormat::Json);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let item = &value["items"][0];
        assert_eq!(item["new_start_seconds"], 3.0);
        assert!(item["new_end_seconds"].is_null());
    }

    #[test]
    fn a_no_op_reads_as_a_dash_in_every_format() {
        let report = BatchReport {
            mode: RunMode::DryRun,
            items: vec![item(1, "a.opus", ItemStatus::NoOp)],
        };

        let csv_row = report.render(OutputFormat::Csv);
        let fields: Vec<&str> = csv_row.lines().nth(1).unwrap().split(',').collect();
        assert_eq!(fields[2], "no_op");
        assert!(fields[3..].iter().all(|c| *c == "-"), "{fields:?}");

        let columns = report.items[0].table_row(None, 2);
        assert!(columns[2..7].iter().all(|c| c == "-"), "{columns:?}");
        assert_eq!(columns[7], "no_op");
    }

    #[test]
    fn the_csv_header_matches_every_row_field() {
        let header_fields = CSV_HEADER.split(',').count();
        let report = BatchReport {
            mode: RunMode::Apply,
            items: vec![item(1, "a.opus", ItemStatus::Changed(trim(10.0, 8.0)))],
        };
        let row = report.items[0].csv_row();
        assert_eq!(row.split(',').count(), header_fields);
    }

    #[test]
    fn table_rows_are_introduced_by_a_header() {
        let report = BatchReport {
            mode: RunMode::Apply,
            items: vec![item(1, "a.opus", ItemStatus::Changed(trim(10.0, 8.0)))],
        };
        let table = report.render(OutputFormat::Table);
        let header = table.lines().next().unwrap();
        for column in [
            "NAME",
            "NEW START",
            "NEW END",
            "TRIMMED",
            "STATUS",
        ] {
            assert!(header.contains(column), "missing {column} in {header}");
        }
    }

    #[test]
    fn json_is_compact_and_json_full_is_pretty_but_carry_the_same_data() {
        let report = BatchReport {
            mode: RunMode::DryRun,
            items: vec![item(1, "a.opus", ItemStatus::WouldChange(trim(10.0, 8.0)))],
        };
        let compact = report.render(OutputFormat::Json);
        let pretty = report.render(OutputFormat::JsonFull);

        assert_eq!(compact.lines().count(), 1, "json should be one line");
        assert!(pretty.lines().count() > 1, "json-full should be indented");

        let compact_value: serde_json::Value = serde_json::from_str(&compact).unwrap();
        let pretty_value: serde_json::Value = serde_json::from_str(&pretty).unwrap();
        assert_eq!(compact_value, pretty_value);
    }
}
