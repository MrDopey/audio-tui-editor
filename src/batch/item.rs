//! One file's outcome in a folder-wide run, and the fields a report can show
//! about it — pulled out of [`report`](super::report) since a per-row shape
//! is a distinct concern from the whole-report rendering there (design §17).

use serde_json::json;

use crate::media::first_line;
use crate::timespec::format_timestamp;

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

/// Quote a CSV field per RFC 4180 if it contains a comma, quote, or newline.
fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
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
            Some(width) => crate::text::truncate_with_ellipsis(&self.name, width),
            None => self.name.clone(),
        }
    }

    /// One row for `--format table`/`table-full`: number, name, then every
    /// [`Fields`] value as its own column (`-` where it doesn't apply), so a
    /// reader can see exactly which side of a file moved.
    pub fn table_row(&self, max_name_width: Option<usize>, number_width: usize) -> [String; 9] {
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

    /// One row matching [`super::report::CSV_HEADER`] (RFC 4180 quoting for
    /// the name and the failure/skip reason, the only fields that can
    /// contain a comma). A field that does not apply to a row reads as `-`
    /// rather than being left blank, so every row has the same shape at a
    /// glance.
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

    pub(super) fn to_json(&self) -> serde_json::Value {
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
