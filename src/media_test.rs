use crate::prelude::*;
use crate::*;


    #[test]
    fn selects_only_confirmed_regular_media_without_symlinks_or_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("completed-paths.txt");
        fs::write(dir.path().join("1.jpg"), [1u8; 3]).unwrap();
        fs::write(dir.path().join("2.mp4"), [2u8; 4]).unwrap();
        fs::write(dir.path().join("3.mp3"), [3u8; 5]).unwrap();
        fs::write(dir.path().join("4.jpg.part"), [4u8; 8]).unwrap();
        fs::write(dir.path().join("5.temp.mp4"), [5u8; 3]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("1.jpg"), dir.path().join("6.jpg")).unwrap();
        fs::write(&manifest, format!("{}\n{}\n{}\n{}\n{}\n",
            dir.path().join("1.jpg").display(), dir.path().join("2.mp4").display(),
            dir.path().join("3.mp3").display(), dir.path().join("5.temp.mp4").display(),
            dir.path().join("6.jpg").display())).unwrap();
        let result = select_media(dir.path(), &manifest, 4).unwrap();
        assert_eq!(result.files.len(), 2);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.files[0].kind, MediaKind::Photo);
        assert_eq!(result.files[1].kind, MediaKind::Video);
    }

    #[test]
    fn enforces_file_count_and_size_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("completed-paths.txt");
        let mut paths = String::new();
        for index in 0..12 {
            let path = dir.path().join(format!("{index:02}.jpg"));
            fs::write(&path, [0u8; 2]).unwrap();
            paths.push_str(&format!("{}\n", path.display()));
        }
        fs::write(&manifest, paths).unwrap();
        let result = select_media(dir.path(), &manifest, 2).unwrap();
        assert_eq!(result.files.len(), MAX_FILES);
        assert_eq!(result.skipped, 2);
        let result = select_media(dir.path(), &manifest, 1).unwrap();
        assert!(result.files.is_empty());
        assert_eq!(result.skipped, 12);
    }
