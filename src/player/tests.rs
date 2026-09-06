    use super::*;
    use std::process::Command as Proc;

    /// A short stereo file to decode. Returns its path inside a fresh folder.
    fn fixture(name: &str, seconds: u32) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("audioedit-player-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creating the fixture folder");
        let path = dir.join("tone.wav");
        let status = Proc::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency=440:sample_rate=48000:duration={seconds}"
            ))
            .args(["-ac", "2"])
            .arg(&path)
            .status()
            .expect("running ffmpeg");
        assert!(status.success());
        path
    }

    fn cleanup(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn seeking_moves_and_clamps_position_and_detects_at_end() {
        let path = fixture("silent-seek", 2);
        let output = AudioOutput::silent();
        let mut player = AudioPlayer::new(&output, &path, 600.0, 100.0);

        assert_eq!(player.position(), 0.0);
        assert!(!player.is_playing());

        player.seek_to(120.0);
        assert_eq!(player.position(), 120.0);
        player.seek_by(-30.0);
        assert_eq!(player.position(), 90.0);

        // Seeking is clamped to the file.
        player.seek_by(-1000.0);
        assert_eq!(player.position(), 0.0);
        player.seek_to(10_000.0);
        assert_eq!(player.position(), 600.0);
        assert!(player.at_end());
        cleanup(&path);
    }

    #[test]
    fn volume_is_set_and_clamped_between_0_and_150() {
        let path = fixture("silent-volume", 2);
        let output = AudioOutput::silent();
        let mut player = AudioPlayer::new(&output, &path, 600.0, 100.0);

        player.set_volume(50.0);
        assert_eq!(player.volume(), 50.0);
        player.adjust_volume(-100.0);
        assert_eq!(player.volume(), 0.0, "volume is clamped at zero");
        player.adjust_volume(1000.0);
        assert_eq!(player.volume(), 150.0, "volume is capped");
        cleanup(&path);
    }

    #[test]
    fn a_silent_player_reports_no_decode_error() {
        let path = fixture("no-error", 1);
        let output = AudioOutput::silent();
        let player = AudioPlayer::new(&output, &path, 60.0, 100.0);
        assert!(player.error().is_none());
        cleanup(&path);
    }

    #[test]
    fn playing_from_the_end_restarts_from_the_beginning() {
        let path = fixture("restart", 2);
        let output = AudioOutput::silent();
        let mut player = AudioPlayer::new(&output, &path, 60.0, 100.0);
        player.seek_to(60.0);
        assert!(player.at_end());
        player.play();
        assert!(player.is_playing());
        assert!(
            player.position() < 1.0,
            "playback should restart from the start"
        );
        player.pause();
        assert!(!player.is_playing());
        cleanup(&path);
    }
