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
}

impl PosSpec {
    /// Resolve to an absolute offset in seconds, clamped to `[0, duration]`.
    pub fn resolve(&self, duration: f64) -> f64 {
        let raw = match *self {
            PosSpec::Absolute(s) => s,
            PosSpec::FromStart(s) => s,
            PosSpec::FromEnd(s) => duration - s,
            PosSpec::Percent(p) => duration * p / 100.0,
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

/// Parse a position expression: `+10s`, `-1m`, `50%`, `1:23`, `90`, `1.5s`.
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

    if let Some(rest) = s.strip_prefix('+') {
        return Ok(PosSpec::FromStart(parse_duration(rest)?));
    }
    if let Some(rest) = s.strip_prefix('-') {
        return Ok(PosSpec::FromEnd(parse_duration(rest)?));
    }
    Ok(PosSpec::Absolute(parse_duration(s)?))
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
mod tests {
    use super::*;

    #[test]
    fn parses_relative_expressions() {
        assert_eq!(parse_pos("+10s").unwrap(), PosSpec::FromStart(10.0));
        assert_eq!(parse_pos("-10s").unwrap(), PosSpec::FromEnd(10.0));
        assert_eq!(parse_pos("+1m").unwrap(), PosSpec::FromStart(60.0));
        assert_eq!(parse_pos("-1m").unwrap(), PosSpec::FromEnd(60.0));
        assert_eq!(parse_pos("50%").unwrap(), PosSpec::Percent(50.0));
    }

    #[test]
    fn resolves_against_a_ten_minute_file() {
        // The worked example from design §10.
        let d = 600.0;
        assert_eq!(parse_pos("+10s").unwrap().resolve(d), 10.0);
        assert_eq!(parse_pos("-10s").unwrap().resolve(d), 590.0);
        assert_eq!(
            format_timestamp(parse_pos("+10s").unwrap().resolve(d)),
            "00:10"
        );
        assert_eq!(
            format_timestamp(parse_pos("-10s").unwrap().resolve(d)),
            "09:50"
        );
        assert_eq!(parse_pos("50%").unwrap().resolve(d), 300.0);
    }

    #[test]
    fn parses_clock_and_bare_forms() {
        assert_eq!(parse_duration("1:23").unwrap(), 83.0);
        assert_eq!(parse_duration("1:02:03").unwrap(), 3723.0);
        assert_eq!(parse_duration("90").unwrap(), 90.0);
        assert_eq!(parse_duration("1.5s").unwrap(), 1.5);
        assert_eq!(parse_duration("500ms").unwrap(), 0.5);
        assert_eq!(parse_duration("2h").unwrap(), 7200.0);
    }

    #[test]
    fn rejects_nonsense() {
        assert!(parse_pos("").is_err());
        assert!(parse_pos("abc").is_err());
        assert!(parse_pos("120%").is_err());
        assert!(parse_duration("-5s").is_err());
    }

    #[test]
    fn resolution_is_clamped_to_the_file() {
        assert_eq!(parse_pos("-100s").unwrap().resolve(10.0), 0.0);
        assert_eq!(parse_pos("+100s").unwrap().resolve(10.0), 10.0);
    }

    #[test]
    fn markers_keep_their_expression_until_nudged() {
        let m = Marker::parse("-10s", 600.0).unwrap();
        assert!(m.is_relative());
        assert_eq!(m.text(), "-10s");
        assert_eq!(m.seconds(), 590.0);
        assert_eq!(m.to_string(), "-10s (09:50)");

        let nudged = m.nudged(-1.0, 600.0);
        assert!(!nudged.is_relative());
        assert_eq!(nudged.seconds(), 589.0);
        assert_eq!(nudged.to_string(), "09:49");
    }

    #[test]
    fn formats_timestamps() {
        assert_eq!(format_timestamp(0.0), "00:00");
        assert_eq!(format_timestamp(83.0), "01:23");
        assert_eq!(format_timestamp(6151.0), "01:42:31");
        assert_eq!(format_timestamp_millis(6151.2), "01:42:31.200");
        assert_eq!(format_timestamp_millis(0.9999), "00:00:01.000");
    }
}
