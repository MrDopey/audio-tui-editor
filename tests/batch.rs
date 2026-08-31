//! Folder-wide dry-run and apply runs (design §17).

mod common;

use audioedit::batch::{self, ItemStatus, RunMode};
use audioedit::config::Config;
use audioedit::media::probe;

use common::{temp_files, Workspace};

#[test]
fn a_dry_run_reports_changes_without_touching_any_file() {
    let ws = Workspace::new("dryrun");
    ws.make("a.flac", &["-c:a", "flac"]);
    ws.make("b.opus", &["-c:a", "libopus", "-b:a", "64k"]);
    ws.make_continuous("c.opus");

    let files = probe::scan_folder(ws.path()).expect("scanning");
    let before: Vec<Vec<u8>> = files
        .iter()
        .map(|f| std::fs::read(&f.path).unwrap())
        .collect();

    let report = batch::run(&files, &[], &Config::default(), RunMode::DryRun, |_| {});

    assert_eq!(report.processed(), 3);
    assert_eq!(report.changed(), 2, "two files have silence to trim");
    assert_eq!(report.noop(), 1);
    assert_eq!(report.failed(), 0);
    assert!(report
        .items
        .iter()
        .any(|i| matches!(i.status, ItemStatus::WouldChange { .. })));
    assert!(report
        .summary_lines()
        .iter()
        .any(|l| l.contains("no files were modified")));

    for (file, original) in files.iter().zip(&before) {
        assert_eq!(
            &std::fs::read(&file.path).unwrap(),
            original,
            "{} changed",
            file.file_name()
        );
    }
    assert!(temp_files(ws.path()).is_empty());
}

#[test]
fn an_applied_run_trims_each_file_independently_and_reports_noops() {
    let ws = Workspace::new("apply");
    ws.make("a.flac", &["-c:a", "flac"]);
    ws.make("b.opus", &["-c:a", "libopus", "-b:a", "64k"]);
    ws.make_continuous("c.opus");

    let files = probe::scan_folder(ws.path()).expect("scanning");
    let mut seen = Vec::new();
    let report = batch::run(
        &files,
        &[],
        &Config::default(),
        RunMode::Apply,
        |progress| {
            if let batch::Progress::Item(item) = progress {
                seen.push(item.number);
            }
        },
    );

    assert_eq!(seen, vec![1, 2, 3], "every file is processed in order");
    assert_eq!(report.changed(), 2);
    assert_eq!(report.noop(), 1);
    assert_eq!(report.failed(), 0);

    for file in probe::scan_folder(ws.path()).expect("rescanning") {
        let expected = if file.file_name() == "c.opus" {
            5.0
        } else {
            6.0
        };
        assert!(
            (file.duration - expected).abs() < 0.4,
            "{} is {}s, expected about {expected}s",
            file.file_name(),
            file.duration
        );
    }
    assert!(temp_files(ws.path()).is_empty());
}

#[test]
fn a_broken_file_fails_alone_without_stopping_the_run() {
    let ws = Workspace::new("resilient");
    let good = ws.make("a.flac", &["-c:a", "flac"]);
    let good_before = std::fs::read(&good).unwrap();

    let mut files = probe::scan_folder(ws.path()).expect("scanning");
    // A file that probes fine but whose declared duration is a lie.
    let mut broken = files[0].clone();
    broken.path = ws.path().join("missing.flac");
    broken.duration = 60.0;
    files.push(broken);

    let report = batch::run(&files, &[], &Config::default(), RunMode::Apply, |_| {});

    assert_eq!(report.processed(), 2);
    assert_eq!(report.failed(), 1, "the missing file must fail");
    assert_eq!(report.changed(), 1, "the good file must still be trimmed");
    assert_ne!(std::fs::read(&good).unwrap(), good_before);
    assert!(temp_files(ws.path()).is_empty());
}
