//! The result of a folder-wide run: what happened to each file, and the
//! renderers (`table`/`table-full`/`json`/`json-full`/`csv`) that describe it
//! (design §17).

use serde_json::json;

use super::item::{BatchItem, ItemStatus};
use super::RunMode;

/// Longest a name gets to be in `--format table` before it is trimmed with an
/// ellipsis; `table-full` never trims.
const TABLE_MAX_NAME_WIDTH: usize = 40;

/// The header row for `--format csv`.
pub const CSV_HEADER: &str = "number,name,status,new_start_seconds,new_end_seconds,\
old_duration_seconds,new_duration_seconds,trimmed_seconds,reason";

/// How a headless run (`--dry-run` / `--apply-defaults`) is printed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Columns aligned; long names are trimmed to fit (the default).
    #[default]
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

#[cfg(test)]
mod tests;
