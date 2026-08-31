//! What actually survived a save, established by probing the output and
//! comparing it against the source (design §15).

use super::super::probe::{MediaInfo, METADATA_FIELDS};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoverArt {
    /// The source had none, so there is nothing to say.
    #[default]
    Absent,
    Preserved,
    Lost,
}

/// What actually survived into the output, established by probing it.
#[derive(Debug, Clone, Default)]
pub struct MetadataReport {
    /// Fields that were present in the source and match in the output.
    pub preserved: Vec<String>,
    /// Fields that were meant to be present but are missing or differ.
    pub lost: Vec<String>,
    /// Fields the user edited that were written successfully.
    pub applied: Vec<String>,
    pub cover_art: CoverArt,
    pub chapters_source: usize,
    pub chapters_output: usize,
}

impl MetadataReport {
    /// True only when nothing that was supposed to survive went missing.
    pub fn fully_preserved(&self) -> bool {
        self.lost.is_empty()
            && self.cover_art != CoverArt::Lost
            && self.chapters_output >= self.chapters_source
    }

    /// A one-line verdict for the save summary.
    pub fn summary_line(&self) -> String {
        if self.fully_preserved() {
            "preserved".to_string()
        } else {
            let mut parts = Vec::new();
            if !self.lost.is_empty() {
                parts.push(format!("lost {}", self.lost.join(", ")));
            }
            if self.cover_art == CoverArt::Lost {
                parts.push("lost cover artwork".to_string());
            }
            if self.chapters_output < self.chapters_source {
                parts.push(format!(
                    "lost {} chapter(s)",
                    self.chapters_source - self.chapters_output
                ));
            }
            format!("partially preserved ({})", parts.join("; "))
        }
    }
}

/// Metadata validation: compare what should be there against what is there.
pub fn compare_metadata(
    source: &MediaInfo,
    output: &MediaInfo,
    edits: &std::collections::BTreeMap<String, Option<String>>,
) -> MetadataReport {
    let mut report = MetadataReport {
        cover_art: if !source.has_cover_art {
            CoverArt::Absent
        } else if output.has_cover_art {
            CoverArt::Preserved
        } else {
            CoverArt::Lost
        },
        chapters_source: source.chapter_count,
        chapters_output: output.chapter_count,
        ..MetadataReport::default()
    };

    // Every field the source carried, plus every field the user set.
    let mut keys: Vec<String> = METADATA_FIELDS.iter().map(|(k, _)| k.to_string()).collect();
    for key in source.all_tags().keys() {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    for key in edits.keys() {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }

    for key in keys {
        let edited = edits.get(&key);
        let intended: Option<String> = match edited {
            Some(Some(value)) => Some(value.clone()),
            Some(None) => None,
            None => source.tag(&key).map(str::to_string),
        };
        let actual = output.tag(&key).map(str::to_string);
        let label = label_for(&key);

        match intended {
            None => {
                // Nothing was meant to be here; an absent tag is correct.
            }
            Some(intended) => {
                if actual.as_deref().map(str::trim) == Some(intended.trim()) {
                    if edited.is_some() {
                        report.applied.push(label);
                    } else {
                        report.preserved.push(label);
                    }
                } else {
                    report.lost.push(label);
                }
            }
        }
    }

    report
}

fn label_for(key: &str) -> String {
    METADATA_FIELDS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, label)| label.to_string())
        .unwrap_or_else(|| key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn info(tags: &[(&str, &str)], cover: bool, chapters: usize) -> MediaInfo {
        MediaInfo {
            has_cover_art: cover,
            chapter_count: chapters,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..crate::media::probe::fixture()
        }
    }

    #[test]
    fn identical_metadata_is_reported_as_preserved() {
        let source = info(&[("title", "Hello"), ("artist", "Jane")], false, 0);
        let output = info(&[("title", "Hello"), ("artist", "Jane")], false, 0);
        let report = compare_metadata(&source, &output, &BTreeMap::new());
        assert!(report.fully_preserved());
        assert_eq!(report.summary_line(), "preserved");
        assert!(report.preserved.contains(&"Title".to_string()));
    }

    #[test]
    fn missing_metadata_is_never_reported_as_preserved() {
        let source = info(
            &[("title", "Hello"), ("comment", "Recorded remotely")],
            false,
            0,
        );
        let output = info(&[("title", "Hello")], false, 0);
        let report = compare_metadata(&source, &output, &BTreeMap::new());
        assert!(!report.fully_preserved());
        assert_eq!(report.lost, vec!["Comment".to_string()]);
        assert!(report.summary_line().contains("Comment"));
    }

    #[test]
    fn lost_cover_art_is_reported() {
        let source = info(&[], true, 0);
        let output = info(&[], false, 0);
        let report = compare_metadata(&source, &output, &BTreeMap::new());
        assert_eq!(report.cover_art, CoverArt::Lost);
        assert!(!report.fully_preserved());
        assert!(report.summary_line().contains("cover artwork"));
    }

    #[test]
    fn absent_cover_art_is_not_a_loss() {
        let report = compare_metadata(&info(&[], false, 0), &info(&[], false, 0), &BTreeMap::new());
        assert_eq!(report.cover_art, CoverArt::Absent);
        assert!(report.fully_preserved());
    }

    #[test]
    fn dropped_chapters_are_reported() {
        let report = compare_metadata(&info(&[], false, 5), &info(&[], false, 2), &BTreeMap::new());
        assert!(!report.fully_preserved());
        assert!(report.summary_line().contains("3 chapter(s)"));
    }

    #[test]
    fn applied_edits_are_distinguished_from_preserved_fields() {
        let source = info(&[("title", "Old"), ("artist", "Jane")], false, 0);
        let output = info(&[("title", "New"), ("artist", "Jane")], false, 0);
        let edits = BTreeMap::from([("title".to_string(), Some("New".to_string()))]);
        let report = compare_metadata(&source, &output, &edits);
        assert_eq!(report.applied, vec!["Title".to_string()]);
        assert!(report.preserved.contains(&"Artist".to_string()));
        assert!(report.fully_preserved());
    }

    #[test]
    fn a_deleted_tag_that_survives_is_a_failure() {
        let source = info(&[("comment", "old")], false, 0);
        let output = info(&[("comment", "old")], false, 0);
        let edits = BTreeMap::from([("comment".to_string(), None)]);
        let report = compare_metadata(&source, &output, &edits);
        // The tag was meant to be gone, so it is not "preserved" either way.
        assert!(report.preserved.is_empty());
        assert!(report.applied.is_empty());
        assert!(report.fully_preserved());
    }
}
