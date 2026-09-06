    use super::*;

    #[test]
    fn temp_file_sits_beside_the_source_and_keeps_the_extension() {
        let temp = TempFile::beside(Path::new("/music/interview 1.opus")).unwrap();
        assert_eq!(temp.path.parent().unwrap(), Path::new("/music"));
        assert_eq!(temp.path.extension().unwrap(), "opus");
        assert!(temp
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with('.'));
    }

    #[test]
    fn temp_file_is_removed_unless_committed() {
        let dir = std::env::temp_dir().join(format!("audioedit-tmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("x.wav");
        let temp = TempFile::beside(&source).unwrap();
        std::fs::write(&temp.path, b"partial").unwrap();
        let path = temp.path.clone();
        drop(temp);
        assert!(
            !path.exists(),
            "an abandoned temporary file must be cleaned up"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn temp_file_survives_drop_once_kept() {
        let dir = std::env::temp_dir().join(format!("audioedit-tmp-keep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("x.wav");
        let temp = TempFile::beside(&source).unwrap();
        std::fs::write(&temp.path, b"partial").unwrap();
        let path = temp.into_kept_path();
        assert!(path.exists(), "a kept temporary file must survive drop");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn processing_labels_match_the_spec() {
        assert_eq!(Processing::StreamCopy.to_string(), "stream copy");
        assert_eq!(Processing::Reencode.to_string(), "re-encoding");
    }

    #[test]
    fn a_lossy_result_is_retried_while_a_better_attempt_remains() {
        let lossy = MetadataReport {
            cover_art: CoverArt::Lost,
            ..MetadataReport::default()
        };
        assert!(
            should_retry_for_cleaner_metadata(&lossy, false),
            "a stream-copy-audio-only result must not win over an untried \
             reencode-all-streams attempt just because it came first"
        );
    }

    #[test]
    fn a_lossy_result_is_accepted_once_nothing_else_remains() {
        let lossy = MetadataReport {
            cover_art: CoverArt::Lost,
            ..MetadataReport::default()
        };
        assert!(!should_retry_for_cleaner_metadata(&lossy, true));
    }

    #[test]
    fn a_clean_result_is_never_retried() {
        assert!(!should_retry_for_cleaner_metadata(
            &MetadataReport::default(),
            false
        ));
    }
