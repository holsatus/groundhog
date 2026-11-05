//! Files a frontend keeps for itself, such as its configuration, written
//! safely.
//!
//! What a frontend stores, and in which format, is its own business. What
//! every such file needs is the same, so it is written and tested once here:
//!
//! - [`write_atomic`] replaces a file so that a crash or a concurrent save,
//!   from this process or another, never leaves it half-written, and keeps
//!   its permissions;
//! - [`read_or_set_aside`] loads a file, and moves one that is not usable
//!   (not text, or not parseable) aside instead of overwriting it. A file
//!   that cannot be *read* is left alone, and its caller must then not write
//!   it either;
//! - [`SaveQueue`] saves off the caller's thread, one save at a time and the
//!   newest last.

use std::{
    ffi::OsString,
    fmt::Display,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::sync::{mpsc, watch};

/// Uniquifies the sidecar of each write within this process; the process id
/// does so between processes.
static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `path` with `suffix` appended to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// The intermediate file a write of `path` goes through: unique to this
/// write, so no two writers, in one process or two, share one.
fn sidecar(path: &Path) -> PathBuf {
    let write = WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    with_suffix(path, &format!(".tmp-{}-{write}", std::process::id()))
}

/// Write `contents` to `path` atomically: into a sibling temporary file
/// first, flushed to disk, then renamed over the target, which keeps its
/// permissions. Creates missing parent directories.
///
/// # Errors
///
/// The underlying I/O failure: the directory cannot be created, the temporary
/// file cannot be written or synced, or the rename fails (a read-only or full
/// filesystem, a directory in the way). The target keeps its old contents
/// unless the rename succeeded, and a failed save removes its temporary file.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    let temporary = sidecar(path);
    if let Err(error) =
        write_durably(&temporary, contents, path).and_then(|()| std::fs::rename(&temporary, path))
    {
        // A failed save must not leave its sidecar behind.
        drop(std::fs::remove_file(&temporary));
        return Err(error);
    }

    sync_parent(path);
    Ok(())
}

/// Write `contents` to `temporary`, with the permissions of `target` if it
/// exists, and flush it to disk: the rename that follows must not reach the
/// disk before the data does.
fn write_durably(temporary: &Path, contents: &[u8], target: &Path) -> io::Result<()> {
    let mut file = File::create(temporary)?;
    file.write_all(contents)?;
    match std::fs::metadata(target) {
        Ok(metadata) => file.set_permissions(metadata.permissions())?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    file.sync_all()
}

/// Flush the directory holding `path`, so the rename itself is durable. Best
/// effort: not every platform can open a directory.
fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let Err(error) = File::open(parent).and_then(|directory| directory.sync_all()) {
            log::debug!("Could not flush {}: {error}", parent.display());
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Read `path` and `parse` its contents.
///
/// `Ok(None)` when there is nothing usable to load: the file does not exist,
/// or it is not text or does not parse, in which case it is moved aside to
/// `<path>.corrupt` (so a fresh file can be written without losing it).
/// Any other read error is returned, and the file is left alone: the caller
/// must then not write it either, or the file is lost after all.
///
/// # Errors
///
/// A read failure other than "not found": no permission, a directory where the
/// file should be, .... Text that does not parse is *not* an error (it is set
/// aside and `Ok(None)` returned), so an error here means the file could not
/// even be read and is still in place.
pub fn read_or_set_aside<T, E: Display>(
    path: &Path,
    parse: impl FnOnce(&str) -> Result<T, E>,
) -> io::Result<Option<T>> {
    let contents = match std::fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    let parsed = match String::from_utf8(contents) {
        Ok(text) => parse(&text).map_err(|error| error.to_string()),
        Err(error) => Err(format!("not UTF-8 text ({error})")),
    };

    match parsed {
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
    /// The newest value, numbered by the saves so far.
    latest: watch::Sender<(u64, Option<T>)>,
    /// The number of the last save written (or failed).
    written: watch::Receiver<u64>,
}

impl<T> Clone for SaveQueue<T> {
    fn clone(&self) -> Self {
        Self {
            latest: self.latest.clone(),
            written: self.written.clone(),
        }
    }
}

impl<T: Clone + Send + Sync + 'static> SaveQueue<T> {
    /// Start the queue on the current async runtime. `write` persists one
    /// value; the returned receiver yields the outcome of each write.
    pub fn spawn(
        write: impl Fn(T) -> Result<(), String> + Send + Sync + 'static,
    ) -> (Self, mpsc::UnboundedReceiver<Result<(), String>>) {
        let (latest, mut requests) = watch::channel((0, None::<T>));
        let (written_tx, written) = watch::channel(0);
        let (outcomes, receiver) = mpsc::unbounded_channel();
        let write = std::sync::Arc::new(write);

        tokio::spawn(async move {
            while requests.changed().await.is_ok() {
                let (number, value) = requests.borrow_and_update().clone();
                let Some(value) = value else {
                    continue;
                };

                let write = write.clone();
                let outcome = tokio::task::spawn_blocking(move || write(value))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()));

                written_tx.send_replace(number);
                drop(outcomes.send(outcome));
            }
        });

        (Self { latest, written }, receiver)
    }

    /// Persist `value`, or a newer one if saved again meanwhile.
    pub fn save(&self, value: T) {
        self.latest.send_modify(|(number, latest)| {
            *number += 1;
            *latest = Some(value);
        });
    }

    /// Resolves once the newest value saved so far has been written (or its
    /// write failed): what to wait for before the process exits.
    pub fn flushed(&self) -> impl Future<Output = ()> + Send + 'static {
        let saved = self.latest.borrow().0;
        let mut written = self.written.clone();
        async move {
            // A queue whose task is gone has nothing left to write.
            drop(written.wait_for(|written| *written >= saved).await);
        }
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
            drop(std::fs::remove_dir_all(self.0.parent().unwrap()));
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

    #[test]
    fn invalid_utf8_is_set_aside() {
        let file = TempFile::new();
        let bytes = [0xff, 0xfe, b'x'];
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(&file.0, bytes).unwrap();

        let loaded = read_or_set_aside(&file.0, |text| text.parse::<u32>());
        assert_eq!(loaded.unwrap(), None);
        assert_eq!(file.siblings(), ["file.txt.corrupt"]);
        let backup = file.0.with_file_name("file.txt.corrupt");
        assert_eq!(std::fs::read(backup).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_writes_keep_the_files_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let file = TempFile::new();
        write_atomic(&file.0, b"one").unwrap();
        std::fs::set_permissions(&file.0, std::fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&file.0, b"two").unwrap();
        let mode = std::fs::metadata(&file.0).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn sidecars_are_unique_between_processes() {
        let name = sidecar(Path::new("config.ron"));
        let pid = std::process::id().to_string();
        assert!(name.to_string_lossy().contains(&pid), "{}", name.display());
        assert_ne!(sidecar(Path::new("config.ron")), name);
    }

    #[tokio::test]
    async fn flushed_waits_for_the_newest_save() {
        let file = TempFile::new();
        let path = file.0.clone();
        let (queue, _outcomes) = SaveQueue::spawn(move |value: u32| {
            std::thread::sleep(Duration::from_millis(20));
            write_atomic(&path, value.to_string().as_bytes()).map_err(|error| error.to_string())
        });

        queue.save(1);
        queue.save(2);
        tokio::time::timeout(Duration::from_secs(5), queue.flushed())
            .await
            .expect("the save is written");

        assert_eq!(std::fs::read_to_string(&file.0).unwrap(), "2");
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
