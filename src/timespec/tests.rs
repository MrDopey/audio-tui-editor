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
    fn a_doubled_dash_means_the_same_as_a_single_one() {
        // The Cursor prompt gives `++`/`--` a distinct meaning (see
        // `cursor_pos_prefixes_pick_the_right_reference_point`), but a
        // Begin/End marker has no "current position" to be relative to, so
        // here they're just accepted as synonyms rather than misparsing
        // "--10" as the negative duration "-10".
        assert_eq!(parse_pos("--10s").unwrap(), PosSpec::FromEnd(10.0));
        assert_eq!(parse_pos("++10s").unwrap(), PosSpec::FromStart(10.0));
        assert_eq!(parse_pos("--10").unwrap(), PosSpec::FromEnd(10.0));
        assert_eq!(parse_pos("++10").unwrap(), PosSpec::FromStart(10.0));
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
    fn rejects_non_finite_values() {
        assert!(parse_pos("nan").is_err());
        assert!(parse_pos("inf").is_err());
        assert!(parse_pos("-inf").is_err());
        assert!(parse_pos("+nan").is_err());
        assert!(parse_pos("-nan").is_err());
        assert!(parse_pos("+inf").is_err());
        assert!(parse_pos("nan%").is_err());
        assert!(parse_pos("inf%").is_err());
        assert!(parse_duration("nan").is_err());
        assert!(parse_duration("infs").is_err());
        assert!(parse_duration("1:nan").is_err());
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
    fn cursor_pos_prefixes_pick_the_right_reference_point() {
        // current = 100s, duration = 600s.
        assert_eq!(parse_cursor_pos("+10s", 100.0, 600.0).unwrap(), 110.0);
        assert_eq!(parse_cursor_pos("-10s", 100.0, 600.0).unwrap(), 90.0);
        assert_eq!(parse_cursor_pos("++10s", 100.0, 600.0).unwrap(), 10.0);
        assert_eq!(parse_cursor_pos("--10s", 100.0, 600.0).unwrap(), 590.0);
        // `mm:ss` and `P%` are unambiguous positions, so they stay absolute.
        assert_eq!(parse_cursor_pos("1:23", 100.0, 600.0).unwrap(), 83.0);
        assert_eq!(parse_cursor_pos("50%", 100.0, 600.0).unwrap(), 300.0);
    }

    #[test]
    fn a_bare_number_is_relative_to_current_by_default() {
        // current = 100s, duration = 600s — bare `X` means `+X`, same as
        // typing the `+` explicitly, so `10` is 10s further, not 00:10.
        assert_eq!(parse_cursor_pos("10", 100.0, 600.0).unwrap(), 110.0);
        assert_eq!(parse_cursor_pos("10s", 100.0, 600.0).unwrap(), 110.0);
        assert_eq!(parse_cursor_pos("1m", 100.0, 600.0).unwrap(), 160.0);
        assert_eq!(
            parse_marker_pos("10", 100.0).unwrap(),
            PosSpec::Resolved(110.0)
        );
    }

    #[test]
    fn cursor_pos_clamps_to_the_file() {
        assert_eq!(parse_cursor_pos("-1000s", 100.0, 600.0).unwrap(), 0.0);
        assert_eq!(parse_cursor_pos("+1000s", 100.0, 600.0).unwrap(), 600.0);
        assert_eq!(parse_cursor_pos("--1000s", 100.0, 600.0).unwrap(), 0.0);
    }

    #[test]
    fn formats_timestamps() {
        assert_eq!(format_timestamp(0.0), "00:00");
        assert_eq!(format_timestamp(83.0), "01:23");
        assert_eq!(format_timestamp(6151.0), "01:42:31");
        assert_eq!(format_timestamp_millis(6151.2), "01:42:31.200");
        assert_eq!(format_timestamp_millis(0.9999), "00:00:01.000");
    }
