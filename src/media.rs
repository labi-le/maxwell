use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MediaKind {
    Photo,
    Video,
    Audio,
    Document,
}

pub(crate) fn media_kind(path: &Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_str()?;
    if ["jpg", "jpeg", "png"]
        .iter()
        .any(|item| ext.eq_ignore_ascii_case(item))
    {
        Some(MediaKind::Photo)
    } else if ext.eq_ignore_ascii_case("mp4") {
        Some(MediaKind::Video)
    } else if ["mp3", "m4a"]
        .iter()
        .any(|item| ext.eq_ignore_ascii_case(item))
    {
        Some(MediaKind::Audio)
    } else if [
        "webp", "gif", "heic", "heif", "mov", "mkv", "webm", "avi", "ogg", "opus", "wav", "flac",
        "aac", "aiff", "m4v", "3gp",
    ]
    .iter()
    .any(|item| ext.eq_ignore_ascii_case(item))
    {
        Some(MediaKind::Document)
    } else {
        None
    }
}

pub(crate) struct MediaFile {
    pub(crate) path: PathBuf,
    pub(crate) kind: MediaKind,
}

pub(crate) struct Selection {
    pub(crate) files: Vec<MediaFile>,
    pub(crate) skipped: usize,
    pub(crate) size_skipped: usize,
}

pub(crate) fn select_media(
    dir: &Path,
    manifest: &Path,
    max_bytes: u64,
) -> std::io::Result<Selection> {
    let mut selected = Selection {
        files: Vec::new(),
        skipped: 0,
        size_skipped: 0,
    };
    let confirmed = match fs::File::open(manifest) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(selected),
        Err(error) => return Err(error),
    };
    let mut canonical_dir = None;
    for line in BufReader::new(confirmed).lines() {
        let line = line?;
        let path = PathBuf::from(line.trim_end_matches('\r'));
        // yt-dlp's output template places finished files directly in this work directory.
        if path.parent() != Some(dir)
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".temp.mp4"))
            || selected.files.iter().any(|file| file.path == path)
        {
            continue;
        }
        let kind = media_kind(&path);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            continue;
        }
        if canonical_dir.is_none() {
            canonical_dir = Some(fs::canonicalize(dir)?);
        }
        if fs::canonicalize(&path)?.parent() != canonical_dir.as_deref() {
            continue;
        }
        let Some(kind) = kind else {
            selected.skipped += 1;
            continue;
        };
        if metadata.len() == 0 || metadata.len() > max_bytes || selected.files.len() == MAX_FILES {
            selected.skipped += 1;
            if metadata.len() > max_bytes {
                selected.size_skipped += 1;
            }
            continue;
        }
        selected.files.push(MediaFile { path, kind });
    }
    Ok(selected)
}

pub(crate) fn disk_usage(dir: &Path) -> std::io::Result<u64> {
    let mut size = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_file() {
            size = size.saturating_add(metadata.len());
        }
    }
    Ok(size)
}

async fn spawn_blocking_scan<T: Send + 'static>(
    task: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> std::io::Result<T> {
    tokio::task::spawn_blocking(task)
        .await
        .unwrap_or_else(|_| Err(std::io::Error::other("media scan task failed")))
}

pub(crate) async fn select_media_async(
    dir: &Path,
    manifest: &Path,
    max_bytes: u64,
) -> std::io::Result<Selection> {
    let dir = dir.to_path_buf();
    let manifest = manifest.to_path_buf();
    spawn_blocking_scan(move || select_media(&dir, &manifest, max_bytes)).await
}

pub(crate) async fn disk_usage_async(dir: &Path) -> std::io::Result<u64> {
    let dir = dir.to_path_buf();
    spawn_blocking_scan(move || disk_usage(&dir)).await
}
