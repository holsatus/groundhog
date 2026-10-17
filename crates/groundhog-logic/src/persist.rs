//! Files a frontend keeps for itself, such as its configuration, written
//! safely.
//!
//! What a frontend stores, and in which format, is its own business. What
//! every such file needs is the same, so it is written and tested once here:
//!
//! - [`write_atomic`] replaces a file so that a crash or a concurrent save
//!   never leaves it half-written;
//! - [`read_or_set_aside`] loads a file, and moves one that does not parse
//!   aside instead of overwriting it;
//! - [`SaveQueue`] saves off the caller's thread, one save at a time and the
//!   newest last.

use std::{
    ffi::OsString,
    fmt::Display,
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::sync::{mpsc, watch};

/// Uniquifies the sidecar of each write, so two concurrent writers never
/// write into the same intermediate file.
static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `path` with `suffix` appended to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// Write `contents` to `path` atomically: into a sibling temporary file
/// first, then renamed over the target. Creates missing parent directories.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    let temporary = with_suffix(
        path,
        &format!(".tmp-{}", WRITE_COUNTER.fetch_add(1, Ordering::Relaxed)),
    );
    std::fs::write(&temporary, contents)?;

    if let Err(error) = std::fs::rename(&temporary, path) {
        // A failed save must not leave its sidecar behind.
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

/// Read `path` and `parse` its contents.
///
/// `Ok(None)` when there is nothing usable to load: the file does not exist,
/// or it does not parse, in which case it is moved aside to
/// `<path>.corrupt` (so a fresh file can be written without losing it).
/// Any other read error is returned, and the file is left alone.
pub fn read_or_set_aside<T, E: Display>(
    path: &Path,
    parse: impl FnOnce(&str) -> Result<T, E>,
) -> io::Result<Option<T>> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    match parse(&contents) {
        Ok(value) => Ok(Some(value)),
        Err(error) => {
            let backup = with_suffix(path, ".corrupt");
            log::error!(
                "Could not parse {}: {error}; moving it to {}",
                path.display(),
                backup.display()
            );
            if let Err(error) = std::fs::rename(path, &backup) {
                log::error!("Could not move {} aside: {error}", path.display());
            }
            Ok(None)
        }
    }
}

/// Saves values one at a time, in order, on blocking threads.
///
/// Saves are coalesced: while one is being written, only the newest of the
/// values saved meanwhile is written next. Two saves can therefore never land
/// out of order and leave an older value on disk.
pub struct SaveQueue<T> {
    latest: watch::Sender<Option<T>>,
}

impl<T> Clone for SaveQueue<T> {
    fn clone(&self) -> Self {
        Self {
            latest: self.latest.clone(),
        }
    }
}

impl<T: Clone + Send + Sync + 'static> SaveQueue<T> {
    /// Start the queue on the current async runtime. `write` persists one
    /// value; the returned receiver yields the outcome of each write.
    pub fn spawn(
        write: impl Fn(T) -> Result<(), String> + Send + Sync + 'static,
    ) -> (Self, mpsc::UnboundedReceiver<Result<(), String>>) {
        let (latest, mut requests) = watch::channel(None::<T>);
        let (outcomes, receiver) = mpsc::unbounded_channel();
        let write = std::sync::Arc::new(write);

        tokio::spawn(async move {
            while requests.changed().await.is_ok() {
                let Some(value) = requests.borrow_and_update().clone() else {
                    continue;
                };

                let write = write.clone();
                let outcome = tokio::task::spawn_blocking(move || write(value))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()));

                let _ = outcomes.send(outcome);
            }
        });

        (Self { latest }, receiver)
    }

    /// Persist `value`, or a newer one if saved again meanwhile.
    pub fn save(&self, value: T) {
        self.latest.send_replace(Some(value));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    static DIRECTORY: AtomicU64 = AtomicU64::new(0);

    /// A file path in a fresh temporary directory, removed when dropped.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "groundhog-persist-{}-{}",
                std::process::id(),
                DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            Self(directory.join("file.txt"))
        }

        /// Every file in the directory, by name.
        fn siblings(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(self.0.parent().unwrap())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[test]
    fn atomic_writes_replace_the_file_and_leave_nothing_behind() {
        let file = TempFile::new();

        write_atomic(&file.0, b"one").unwrap();
        write_atomic(&file.0, b"two").unwrap();

        assert_eq!(std::fs::read_to_string(&file.0).unwrap(), "two");
        assert_eq!(file.siblings(), ["file.txt"]);
    }

    #[test]
    fn unparseable_files_are_set_aside() {
        let file = TempFile::new();
        let parse = |text: &str| text.parse::<u32>();

        assert_eq!(read_or_set_aside(&file.0, parse).unwrap(), None, "missing");

        write_atomic(&file.0, b"42").unwrap();
        assert_eq!(read_or_set_aside(&file.0, parse).unwrap(), Some(42));

        write_atomic(&file.0, b"not a number").unwrap();
        assert_eq!(read_or_set_aside(&file.0, parse).unwrap(), None);
        assert_eq!(file.siblings(), ["file.txt.corrupt"]);
        let backup = file.0.with_file_name("file.txt.corrupt");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "not a number");
    }

    #[tokio::test]
    async fn the_newest_save_lands_last() {
        let file = TempFile::new();
        let path = file.0.clone();
        let (queue, mut outcomes) = SaveQueue::spawn(move |value: u32| {
            // Slow enough that saves pile up behind the first.
            std::thread::sleep(Duration::from_millis(20));
            write_atomic(&path, value.to_string().as_bytes()).map_err(|error| error.to_string())
        });

        for value in 1..=20 {
            queue.save(value);
        }

        let mut writes = 0;
        loop {
            let outcome = tokio::time::timeout(Duration::from_secs(5), outcomes.recv())
                .await
                .expect("the saves finish")
                .expect("the queue is alive");
            assert_eq!(outcome, Ok(()));
            writes += 1;
            if std::fs::read_to_string(&file.0).unwrap() == "20" {
                break;
            }
        }
        assert!(writes < 20, "saves made meanwhile were coalesced");
    }
}
