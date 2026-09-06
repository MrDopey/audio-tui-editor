//! Parsing and formatting of user-facing time positions.
//!
//! Positions are a first-class user feature (design §10): the user writes
//! `+10s`, `-1m` or `50%` and never has to compute an absolute timestamp or
//! learn FFmpeg's timestamp syntax. The semantic expression is retained for
//! display and only resolved to an absolute offset when needed.

use std::fmt;

/// A position within a file, possibly expressed relative to its start or end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PosSpec {
    /// An absolute offset in seconds from the start of the file.
    Absolute(f64),
    /// `+X`: X seconds after the start of the file.
    FromStart(f64),
    /// `-X`: X seconds before the end of the file.
    FromEnd(f64),
    /// `P%`: a fraction of the total duration.
    Percent(f64),
    /// `+X`/`-X` relative to a marker's own current position ([`parse_marker_pos`]),
    /// already resolved to an absolute offset at parse time. Kept distinct
    /// from `Absolute` only so [`Marker::is_relative`] still shows the typed
    /// expression alongside the resolved timestamp.
    Resolved(f64),
}

impl PosSpec {
    /// Resolve to an absolute offset in seconds, clamped to `[0, duration]`.
    pub fn resolve(&self, duration: f64) -> f64 {
        let raw = match *self {
            PosSpec::Absolute(s) => s,
            PosSpec::FromStart(s) => s,
            PosSpec::FromEnd(s) => duration - s,
            PosSpec::Percent(p) => duration * p / 100.0,
            PosSpec::Resolved(s) => s,
        };
        raw.clamp(0.0, duration.max(0.0))
    }
}

/// A marker: the expression the user wrote plus its resolved absolute value.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    spec: PosSpec,
    text: String,
    seconds: f64,
}

impl Marker {
    /// Build a marker from a spec, resolving it against the file duration.
    pub fn from_spec(spec: PosSpec, text: String, duration: f64) -> Self {
        Marker {
            spec,
            text,
            seconds: spec.resolve(duration),
        }
    }

    /// Build an absolute marker, rendering its own canonical timestamp text.
    pub fn absolute(seconds: f64, duration: f64) -> Self {
        let seconds = seconds.clamp(0.0, duration.max(0.0));
        Marker {
            spec: PosSpec::Absolute(seconds),
            text: format_timestamp(seconds),
            seconds,
        }
    }

    /// Parse a user expression against a known duration.
    pub fn parse(input: &str, duration: f64) -> Result<Self, String> {
        let spec = parse_pos(input)?;
        Ok(Marker::from_spec(spec, input.trim().to_string(), duration))
    }

    pub fn seconds(&self) -> f64 {
        self.seconds
    }

    /// The expression as the user wrote it (e.g. `-10s`).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// True when the displayed text is a relative expression rather than a
    /// plain timestamp, so the UI can show both forms.
    pub fn is_relative(&self) -> bool {
        !matches!(self.spec, PosSpec::Absolute(_))
    }

    /// Shift the marker by `delta` seconds. The result is absolute: nudging a
    /// relative marker with the arrow keys makes it a concrete position.
    pub fn nudged(&self, delta: f64, duration: f64) -> Self {
        Marker::absolute(self.seconds + delta, duration)
    }
}

impl fmt::Display for Marker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_relative() {
            write!(f, "{} ({})", self.text, format_timestamp(self.seconds))
        } else {
            write!(f, "{}", format_timestamp(self.seconds))
        }
    }
}

/// Parse a position expression relative to the file itself: `+10s`, `-1m`,
/// `50%`, `1:23`, `90`, `1.5s` — `+`/`-` are always from the file's start/end.
/// Used where there's no "current position" to be relative to (e.g.
/// [`Marker::parse`]); see [`parse_marker_pos`] and [`parse_cursor_pos`] for
/// the marker- and cursor-relative counterparts, which give `+`/`-` a
/// different meaning from this function's.
pub fn parse_pos(input: &str) -> Result<PosSpec, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty position".to_string());
    }

    if let Some(rest) = s.strip_suffix('%') {
        let pct: f64 = rest
            .trim()
            .parse()
            .map_err(|_| format!("invalid percentage: {s}"))?;
        if !(0.0..=100.0).contains(&pct) {
            return Err(format!("percentage out of range: {s}"));
        }
        return Ok(PosSpec::Percent(pct));
    }

    // `++`/`--` mean the same thing here as `+`/`-` (there's no "current
    // position" for a Begin/End marker to be relative to, unlike
    // `parse_marker_pos`/`parse_cursor_pos`, where they're a distinct meaning
    // from single `+`/`-`) — accepted so the same doubled-dash habit those
    // teach doesn't silently misparse as a negative duration here.
    if let Some(rest) = s.strip_prefix("++") {
        return Ok(PosSpec::FromStart(parse_duration(rest)?));
    }
    if let Some(rest) = s.strip_prefix("--") {
        return Ok(PosSpec::FromEnd(parse_duration(rest)?));
    }
    if let Some(rest) = s.strip_prefix('+') {
        return Ok(PosSpec::FromStart(parse_duration(rest)?));
    }
    if let Some(rest) = s.strip_prefix('-') {
        return Ok(PosSpec::FromEnd(parse_duration(rest)?));
    }
    Ok(PosSpec::Absolute(parse_duration(s)?))
}

/// Parse a Begin/End marker expression relative to that marker's own
/// current position — the same `+`/`-` meaning the Cursor prompt's
/// [`parse_cursor_pos`] gives them, unified here so `:b`/`:e` on the command
/// line and the `b`/`e` prompt agree: `+X`/`-X` are `X` seconds after/before
/// `current` (that marker's own position, not the file's start/end);
/// `++X`/`--X`, `mm:ss` and `P%` are unchanged from [`parse_pos`]. A bare `X`
/// with no sign is *also* relative to `current` (the same as `+X`) — typing
/// `10` means 10 seconds further from here, not the absolute timestamp
/// `00:10` — since `mm:ss` (containing `:`) already covers the case where an
/// absolute clock reading is what's wanted.
pub fn parse_marker_pos(input: &str, current: f64) -> Result<PosSpec, String> {
    let s = input.trim();
    if let Some(rest) = s.strip_prefix("++") {
        return Ok(PosSpec::FromStart(parse_duration(rest)?));
    }
    if let Some(rest) = s.strip_prefix("--") {
        return Ok(PosSpec::FromEnd(parse_duration(rest)?));
    }
    if let Some(rest) = s.strip_prefix('+') {
        return Ok(PosSpec::Resolved(current + parse_duration(rest)?));
    }
    if let Some(rest) = s.strip_prefix('-') {
        return Ok(PosSpec::Resolved(current - parse_duration(rest)?));
    }
    if s.ends_with('%') || s.contains(':') {
        return parse_pos(s);
    }
    Ok(PosSpec::Resolved(current + parse_duration(s)?))
}

/// Parse a cursor-jump expression relative to a `current` position (design
/// §11: the Cursor prompt). Same `+`/`-`/`++`/`--` meaning as
/// [`parse_marker_pos`], just resolved to a plain offset instead of a
/// [`PosSpec`] since the cursor has no typed-expression display to preserve.
///
/// `+X`/`-X`: `X` seconds after/before `current`. `++X`/`--X`: `X` seconds
/// after the start / before the end of the file (the same meaning `+`/`-`
/// have in [`parse_pos`]). `mm:ss` and `P%` are absolute/percent, same as
/// [`parse_pos`]; a bare `X` with no sign is relative to `current` instead
/// (the same as `+X`), same reasoning as [`parse_marker_pos`].
pub fn parse_cursor_pos(input: &str, current: f64, duration: f64) -> Result<f64, String> {
    let s = input.trim();
    let clamp = |v: f64| v.clamp(0.0, duration.max(0.0));

    if let Some(rest) = s.strip_prefix("++") {
        return Ok(clamp(parse_duration(rest)?));
    }
    if let Some(rest) = s.strip_prefix("--") {
        return Ok(clamp(duration - parse_duration(rest)?));
    }
    if let Some(rest) = s.strip_prefix('+') {
        return Ok(clamp(current + parse_duration(rest)?));
    }
    if let Some(rest) = s.strip_prefix('-') {
        return Ok(clamp(current - parse_duration(rest)?));
    }
    if s.ends_with('%') || s.contains(':') {
        return Ok(clamp(parse_pos(s)?.resolve(duration)));
    }
    Ok(clamp(current + parse_duration(s)?))
}

/// Parse a duration: `10s`, `1m`, `2h`, `500ms`, `1:23`, `1:02:03`, `90`.
pub fn parse_duration(input: &str) -> Result<f64, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty duration".to_string());
    }

    if s.contains(':') {
        let mut total = 0.0;
        for part in s.split(':') {
            let v: f64 = part
                .trim()
                .parse()
                .map_err(|_| format!("invalid time: {s}"))?;
            if !v.is_finite() {
                return Err(format!("invalid time: {s}"));
            }
            if v < 0.0 {
                return Err(format!("negative component in time: {s}"));
            }
            total = total * 60.0 + v;
        }
        return Ok(total);
    }

    // Longest suffixes first so `ms` is not read as `m`.
    let units: [(&str, f64); 5] = [
        ("ms", 0.001),
        ("s", 1.0),
        ("m", 60.0),
        ("h", 3600.0),
        ("", 1.0),
    ];
    for (suffix, scale) in units {
        let body = if suffix.is_empty() {
            Some(s)
        } else {
            s.strip_suffix(suffix)
        };
        if let Some(body) = body {
            let body = body.trim();
            if body.is_empty() {
                continue;
            }
            if let Ok(v) = body.parse::<f64>() {
                if !v.is_finite() {
                    return Err(format!("invalid duration: {s}"));
                }
                if v < 0.0 {
                    return Err(format!("negative duration: {s}"));
                }
                return Ok(v * scale);
            }
        }
    }
    Err(format!("invalid duration: {s}"))
}

/// `HH:MM:SS` (hours omitted under an hour), used for compact UI columns.
pub fn format_timestamp(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let total = seconds.floor() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// `HH:MM:SS.mmm`, used where the summary must be exact (design §16).
pub fn format_timestamp_millis(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let total = seconds.floor() as u64;
    let millis = ((seconds - total as f64) * 1000.0).round() as u64;
    // Rounding can carry into the next second.
    let (total, millis) = if millis >= 1000 {
        (total + 1, 0)
    } else {
        (total, millis)
    };
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    format!("{h:02}:{m:02}:{s:02}.{millis:03}")
}

#[cfg(test)]
mod tests;
