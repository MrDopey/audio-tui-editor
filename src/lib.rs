//! audioedit — a vim-like terminal audio editor.
//!
//! The crate is split so the media pipeline can be exercised directly by
//! integration tests: [`media`] shells out to ffmpeg/ffprobe, [`player`] owns
//! playback, and [`app`]/[`ui`] are the terminal front end.

pub mod app;
pub mod batch;
pub mod cli;
pub mod config;
pub mod media;
pub mod player;
pub mod timespec;
pub mod ui;
