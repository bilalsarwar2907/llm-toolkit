/// registry.rs
///
/// Central download registry.
///
/// Responsibilities:
/// - Prevent duplicate downloads of the same model file (in this process).
/// - Provide cross-process exclusion via `fs2` filesystem locks.
/// - Support cancellation via a shared atomic flag.
/// - Track all currently active downloads.
///
/// Usage pattern:
///   1. Call `registry.try_acquire(filename)` before starting a download.
///   2. On `AcquireResult::Acquired(guard)`, proceed with the download.
///   3. Poll `guard.is_cancelled()` inside the download loop.
///   4. Drop the guard when the download finishes or fails — this releases
///      both the in-memory slot and the filesystem lock automatically.

use anyhow::{Context, Result};
use fs2::FileExt;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ─── Public types ─────────────────────────────────────────────────────────────

/// Outcome of a `try_acquire` call.
pub enum AcquireResult {
    /// Slot acquired. The caller owns the guard and must drop it when the
    /// download finishes or fails.
    Acquired(DownloadGuard),
    /// A download for this filename is already in progress (either in this
    /// process or in another process holding the filesystem lock).
    AlreadyActive,
}

/// RAII guard for an active download slot.
///
/// Dropping this guard:
/// - Removes the entry from the in-memory registry map.
/// - Releases the cross-process filesystem lock (by closing the lock file).
pub struct DownloadGuard {
    filename: String,
    cancel_flag: Arc<AtomicBool>,
    inner: Arc<Mutex<HashMap<String, DownloadEntry>>>,
    /// Held alive to keep the fs2 lock; released automatically on drop.
    _lock_file: File,
}

impl DownloadGuard {
    /// Signal that this download should be cancelled.
    ///
    /// The download loop is responsible for polling `is_cancelled()` and
    /// stopping cleanly. Cancellation does not abort in-flight I/O.
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    /// Returns `true` if cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }

    /// Returns a clone of the cancellation flag for use inside async tasks.
    ///
    /// The task should check `flag.load(Ordering::SeqCst)` periodically.
    pub fn cancellation_token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel_flag)
    }

    /// The filename this guard protects.
    pub fn filename(&self) -> &str {
        &self.filename
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        // Remove from the in-memory map. If the mutex is poisoned we have
        // bigger problems, but we do not panic in Drop.
        if let Ok(mut map) = self.inner.lock() {
            map.remove(&self.filename);
        }
        // _lock_file is dropped here → fs2 lock released.
    }
}

// ─── Internal entry ───────────────────────────────────────────────────────────

struct DownloadEntry {
    cancel_flag: Arc<AtomicBool>,
}

// ─── Registry ─────────────────────────────────────────────────────────────────

/// Central registry for active model downloads.
///
/// Intended to be created once and shared as `Arc<Registry>` across the
/// application.
pub struct Registry {
    inner: Arc<Mutex<HashMap<String, DownloadEntry>>>,
    lock_dir: PathBuf,
}

impl Registry {
    /// Create a new `Registry`.
    ///
    /// `lock_dir` is the directory where per-file `.lock` files are stored.
    /// It is created if it does not already exist.
    ///
    /// # Errors
    ///
    /// Returns an error if `lock_dir` cannot be created.
    pub fn new(lock_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&lock_dir).with_context(|| {
            format!("could not create lock directory: {}", lock_dir.display())
        })?;
        Ok(Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            lock_dir,
        })
    }

    /// Attempt to acquire an exclusive download slot for `filename`.
    ///
    /// The check is two-phase:
    /// 1. In-memory map lookup (fast; guards against duplicate calls within
    ///    the same process).
    /// 2. `fs2` exclusive lock on `<lock_dir>/<filename>.lock` (guards against
    ///    another process downloading the same file simultaneously).
    ///
    /// # Returns
    ///
    /// - `Ok(AcquireResult::Acquired(guard))` — proceed with the download.
    /// - `Ok(AcquireResult::AlreadyActive)` — download is already in progress.
    /// - `Err(...)` — I/O failure (e.g. cannot create the lock file).
    ///
    /// # Errors
    ///
    /// Propagates I/O errors from lock file creation.
    pub fn try_acquire(&self, filename: &str) -> Result<AcquireResult> {
        let mut map = self.inner.lock().expect("registry mutex poisoned");

        // Phase 1: in-memory duplicate check.
        if map.contains_key(filename) {
            return Ok(AcquireResult::AlreadyActive);
        }

        // Phase 2: cross-process filesystem lock.
        let lock_path = self.lock_path(filename);
        let lock_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("could not open lock file: {}", lock_path.display()))?;

        match lock_file.try_lock_exclusive() {
            Ok(()) => {
                let cancel_flag = Arc::new(AtomicBool::new(false));

                map.insert(
                    filename.to_string(),
                    DownloadEntry {
                        cancel_flag: Arc::clone(&cancel_flag),
                    },
                );

                Ok(AcquireResult::Acquired(DownloadGuard {
                    filename: filename.to_string(),
                    cancel_flag,
                    inner: Arc::clone(&self.inner),
                    _lock_file: lock_file,
                }))
            }
            Err(_) => Ok(AcquireResult::AlreadyActive),
        }
    }

    /// Cancel an in-progress download by filename.
    ///
    /// Sets the cancellation flag so the download loop can detect it.
    ///
    /// Returns `true` if a download was found and flagged.
    /// Returns `false` if no download is currently active for `filename`.
    pub fn cancel(&self, filename: &str) -> bool {
        let map = self.inner.lock().expect("registry mutex poisoned");
        if let Some(entry) = map.get(filename) {
            entry.cancel_flag.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Returns `true` if a download is currently active for `filename`.
    pub fn is_active(&self, filename: &str) -> bool {
        let map = self.inner.lock().expect("registry mutex poisoned");
        map.contains_key(filename)
    }

    /// Returns the filenames of all currently active downloads.
    pub fn active_downloads(&self) -> Vec<String> {
        let map = self.inner.lock().expect("registry mutex poisoned");
        map.keys().cloned().collect()
    }

    /// Build the filesystem lock file path for a given filename.
    ///
    /// Path separators and colons in the filename are replaced with `_` to
    /// produce a valid file name on all platforms.
    fn lock_path(&self, filename: &str) -> PathBuf {
        let safe = filename.replace(['/', '\\', ':'], "_");
        self.lock_dir.join(format!("{safe}.lock"))
    }
}