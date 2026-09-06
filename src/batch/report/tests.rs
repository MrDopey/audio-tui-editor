    use super::super::item::Trim;
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
        assert_eq!(
            report.items[0].line(),
            "01 a.opus   02:31 → 02:28  (-00:03)"
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
        for column in ["NAME", "NEW START", "NEW END", "TRIMMED", "STATUS"] {
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
