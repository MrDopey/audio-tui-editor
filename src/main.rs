//! audioedit — a vim-like terminal audio editor.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use audioedit::{batch, media, ui};
use clap::Parser;
use ratatui::crossterm::event::{self, Event};

use audioedit::app::App;
use audioedit::batch::RunMode;
use audioedit::cli::Cli;
use audioedit::config::Config;
use audioedit::media::probe;
use audioedit::player::AudioOutput;

/// How often the screen is refreshed so the playback cursor moves smoothly.
const FRAME: Duration = Duration::from_millis(50);

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Startup (design §3): folder, files, configuration, CLI overrides.
    let folder = match &cli.folder {
        Some(folder) => folder.clone(),
        None => std::env::current_dir().context("determining the current directory")?,
    };
    let folder = folder
        .canonicalize()
        .with_context(|| format!("resolving folder {}", folder.display()))?;
    anyhow::ensure!(folder.is_dir(), "{} is not a folder", folder.display());

    media::ensure_backend_available()?;

    let (mut config, config_path) = Config::load(cli.config.as_deref())?;
    config.apply(&cli.overrides())?;

    let scan = probe::scan_folder_detailed(&folder)?;
    let files = scan.files;

    if cli.is_batch_run() {
        return run_batch(&cli, &folder, &files, &scan.skipped, &config);
    }

    if let Some(path) = config_path {
        // Only worth mentioning before the TUI takes the screen.
        eprintln!("audioedit: using configuration {}", path.display());
    }

    let output = if cli.no_audio {
        AudioOutput::silent()
    } else {
        AudioOutput::open()
    };
    run_tui(App::new(folder, files, scan.skipped, config, output))
}

/// The interactive application.
fn run_tui(mut app: App) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let mut last_draw = Instant::now() - FRAME;
    loop {
        if last_draw.elapsed() >= FRAME {
            terminal.draw(|frame| ui::render(frame, app))?;
            last_draw = Instant::now();
        }

        if event::poll(FRAME)? {
            match event::read()? {
                Event::Key(key) => {
                    app.on_key(key);
                    last_draw = Instant::now() - FRAME;
                }
                Event::Resize(_, _) => last_draw = Instant::now() - FRAME,
                _ => {}
            }
        }

        if app.tick() {
            last_draw = Instant::now() - FRAME;
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

/// `--apply-defaults` / `--dry-run` without the TUI (design §17).
fn run_batch(
    cli: &Cli,
    folder: &Path,
    files: &[probe::MediaInfo],
    skipped: &[probe::SkippedFile],
    config: &Config,
) -> Result<()> {
    let mode = if cli.dry_run {
        RunMode::DryRun
    } else {
        RunMode::Apply
    };

    if files.is_empty() && skipped.is_empty() {
        println!(
            "No supported audio files were found in {}.",
            folder.display()
        );
        return Ok(());
    }

    if mode == RunMode::Apply && !confirm(cli, files.len(), skipped.len(), config)? {
        println!("Cancelled. No files were modified.");
        return Ok(());
    }

    println!();
    let report = batch::run(files, skipped, config, mode, |progress| {
        if let batch::Progress::Item(item) = progress {
            println!("{}", item.line());
            let _ = std::io::stdout().flush();
        }
    });

    println!();
    for line in report.summary_lines() {
        println!("{line}");
    }

    // A failure in a folder-wide run is worth a non-zero exit status.
    if report.failed() > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Confirmation before a destructive folder-wide run (design §17).
fn confirm(cli: &Cli, count: usize, skipped_count: usize, config: &Config) -> Result<bool> {
    for line in batch::confirmation_lines(count, skipped_count, &config.auto_trim) {
        println!("{line}");
    }
    println!();

    if cli.yes {
        println!("Proceeding (--yes).");
        return Ok(true);
    }

    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "refusing to rewrite {count} files without confirmation. \
             Re-run with --yes to proceed, or --dry-run to preview."
        );
    }

    print!("[Enter] continue, anything else to cancel: ");
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().is_empty())
}
