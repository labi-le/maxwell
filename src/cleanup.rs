use crate::prelude::*;

pub(crate) fn cleanup_old_downloads(download_dir: &Path) {
    const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
    let entries = match fs::read_dir(download_dir) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "Unable to scan {} for startup cleanup: {error}",
                download_dir.display()
            );
            return;
        }
    };
    let now = SystemTime::now();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!(
                    "Unable to read an entry in {} for startup cleanup: {error}",
                    download_dir.display()
                );
                continue;
            }
        };
        if !entry.file_name().as_encoded_bytes().starts_with(b"post-") {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                eprintln!(
                    "Unable to inspect {} for startup cleanup: {error}",
                    entry.path().display()
                );
                continue;
            }
        };
        if !metadata.is_dir() {
            continue;
        }
        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(error) => {
                eprintln!(
                    "Unable to read modification time of {} for startup cleanup: {error}",
                    entry.path().display()
                );
                continue;
            }
        };
        if now.duration_since(modified).is_ok_and(|age| age > MAX_AGE) {
            if let Err(error) = fs::remove_dir_all(entry.path()) {
                eprintln!(
                    "Unable to remove {} during startup cleanup: {error}",
                    entry.path().display()
                );
            }
        }
    }
}
