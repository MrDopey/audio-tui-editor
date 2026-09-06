//! What a save actually did, and the summary shown for it (design §16) —
//! pulled out of `ffmpeg::mod` since reporting on a save is a distinct
//! concern from running the save pipeline itself.

use std::path::PathBuf;

use super::{MetadataReport, Processing};

#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub path: PathBuf,
    /// Nothing needed doing; the file was not rewritten (design §16).
    pub noop: bool,
    pub source_duration: f64,
    pub output_duration: f64,
    pub removed_beginning: f64,
    pub removed_ending: f64,
    pub processing: Processing,
    pub metadata: MetadataReport,
}

impl SaveOutcome {
    /// The save summary shown after every save (design §16).
    pub fn summary_lines(&self) -> Vec<String> {
        use crate::timespec::format_timestamp_millis as ts;

        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string());

        let mut lines = vec![format!("Saved: {name}"), String::new()];

        if self.noop {
            lines.push("No changes were required.".to_string());
            lines.push(String::new());
            lines.push("Duration:".to_string());
            lines.push(format!(
                "  {} → {}",
                ts(self.source_duration),
                ts(self.output_duration)
            ));
            lines.push(String::new());
            lines.push("Status:".to_string());
            lines.push("  NO-OP".to_string());
            return lines;
        }

        lines.push("Duration:".to_string());
        lines.push(format!(
            "  {} → {}",
            ts(self.source_duration),
            ts(self.output_duration)
        ));
        lines.push(String::new());
        lines.push("Removed:".to_string());
        lines.push(format!("  beginning: {:.3}s", self.removed_beginning));
        lines.push(format!("  ending:    {:.3}s", self.removed_ending));
        lines.push(String::new());
        lines.push("Processing:".to_string());
        lines.push(format!("  {}", self.processing));
        lines.push(String::new());
        lines.push("Metadata:".to_string());
        lines.push(format!("  {}", self.metadata.summary_line()));
        lines.push(String::new());
        lines.push("Status:".to_string());
        lines.push("  SUCCESS".to_string());
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(noop: bool) -> SaveOutcome {
        SaveOutcome {
            path: PathBuf::from("/rec/interview.opus"),
            noop,
            source_duration: 6151.2,
            output_duration: if noop { 6151.2 } else { 6128.2 },
            removed_beginning: if noop { 0.0 } else { 12.0 },
            removed_ending: if noop { 0.0 } else { 11.0 },
            processing: Processing::StreamCopy,
            metadata: MetadataReport::default(),
        }
    }

    #[test]
    fn success_summary_matches_the_documented_shape() {
        let lines = outcome(false).summary_lines();
        assert_eq!(lines[0], "Saved: interview.opus");
        assert!(lines.contains(&"  01:42:31.200 → 01:42:08.200".to_string()));
        assert!(lines.contains(&"  beginning: 12.000s".to_string()));
        assert!(lines.contains(&"  ending:    11.000s".to_string()));
        assert!(lines.contains(&"  stream copy".to_string()));
        assert!(lines.contains(&"  preserved".to_string()));
        assert!(lines.contains(&"  SUCCESS".to_string()));
    }

    #[test]
    fn noop_summary_is_reported_explicitly() {
        let lines = outcome(true).summary_lines();
        assert!(lines.contains(&"No changes were required.".to_string()));
        assert!(lines.contains(&"  NO-OP".to_string()));
        assert!(!lines.iter().any(|l| l.contains("Removed")));
        assert!(!lines.iter().any(|l| l.contains("SUCCESS")));
    }

    #[test]
    fn reencoding_is_named_in_the_summary() {
        let mut o = outcome(false);
        o.processing = Processing::Reencode;
        assert!(o.summary_lines().contains(&"  re-encoding".to_string()));
    }

    #[test]
    fn metadata_loss_is_visible_in_the_summary() {
        let mut o = outcome(false);
        o.metadata.lost.push("Comment".to_string());
        let lines = o.summary_lines();
        assert!(lines
            .iter()
            .any(|l| l.contains("partially preserved") && l.contains("Comment")));
    }
}
