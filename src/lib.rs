//! audioedit — a vim-like terminal audio editor.
//!
//! The crate is split so the media pipeline can be exercised directly by
//! integration tests: [`media`] shells out to ffmpeg/ffprobe, [`player`] owns
//! playback, and [`app`]/[`ui`] are the terminal front end.

pub mod app;
mod base64;
pub mod batch;
pub mod cli;
pub mod config;
pub mod debug;
pub mod media;
pub mod player;
pub mod term;
mod text;
pub mod timespec;
pub mod ui;
