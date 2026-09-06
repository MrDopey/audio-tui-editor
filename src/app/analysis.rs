//! A background analysis whose result arrives on a channel — the shape
//! shared by waveform decoding and auto-marker detection (`Session::waveform`
//! / `Session::auto`).

use std::sync::mpsc::Receiver;

use super::try_recv_result;

pub enum Analysis<T> {
    Idle,
    Running(Receiver<anyhow::Result<T>>),
    Ready(T),
    Failed(String),
}

impl<T> Analysis<T> {
    /// Move to `Ready`/`Failed` if the worker has answered. Returns true on a
    /// state change, so the caller knows a redraw is warranted.
    pub(super) fn poll(&mut self) -> bool {
        let Analysis::Running(rx) = self else {
            return false;
        };
        let Some(result) = try_recv_result(rx, "analysis worker") else {
            return false;
        };
        match result {
            Ok(value) => *self = Analysis::Ready(value),
            Err(err) => {
                // The channel carries the full `anyhow::Error` so its chain
                // survives the thread boundary; it is only rendered to text
                // once it settles here, at the point of actually being shown.
                *self = Analysis::Failed(format!("{err:#}"));
            }
        }
        true
    }

    pub fn ready(&self) -> Option<&T> {
        match self {
            Analysis::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Analysis::Running(_))
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Analysis::Failed(err) => Some(err),
            _ => None,
        }
    }
}
