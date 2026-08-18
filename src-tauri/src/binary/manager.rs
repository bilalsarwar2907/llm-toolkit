/// binary/manager.rs
///
/// Manages the llama-server binary lifecycle.
///
/// Responsibilities:
/// - Detect the current platform and select the appropriate release asset.
/// - Download the pinned llama-server release archive.
/// - Verify archive integrity (SHA256 where provided by the release).
/// - Extract only the llama-server binary safely (no path traversal).
/// - Store the binary at `~/.llm-toolkit/bin/`.
/// - Maintain a version lock file at `~/.llm-toolkit/bin/version.lock`.
/// - Detect when the installed version differs from the pinned version.
/// - Never auto-update — version changes require explicit user approval.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

// ─── Version pin ─────────────────────────────────────────────────────────────

/// Pinned llama.cpp release build number.
///
/// To upgrade: update this constant, verify the new assets exist on GitHub
/// Releases, and obtain SHA256 checksums where available.
/// Changing this value without user approval violates the no-auto-update rule.
pub const SUPPORTED_LLAMA_VERSION: &str = "b5234";

// ─── Public types ─────────────────────────────────────────────────────────────

/// Describes the release asset for the current platform.
#[derive(Debug, Clone)]
pub struct PlatformRelease {
    /// Full HTTPS download URL.
    pub url: String,
    /// Expected SHA256 hex digest, if the release publishes one.
    ///
    /// `None` means the integrity check is skipped for this asset.
    /// llama.cpp releases currently do not publish machine-readable checksums.
    pub sha256: Option<String>,
    /// Filename of the `llama-server[.exe]` entry inside the zip archive.
    pub binary_entry: String,
}

/// Current status of the managed binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryStatus {
    /// Binary is absent or has no version lock.
    NotInstalled,
    /// Binary is present and matches the pinned version.
    Installed { version: String },
    /// Binary is present but its version differs from the pin.
    ///
    /// The user must explicitly approve the update.
    OutOfDate { installed: String, pinned: String },
}

// ─── Manager ─────────────────────────────────────────────────────────────────

/// Manages the `llama-server` binary on disk.
///
/// Intended to be created once and shared via `Arc<BinaryManager>`.
pub struct BinaryManager {
    /// Directory where the binary and version lock reside.
    bin_dir: PathBuf,
    /// HTTP client for archive downloads.
    client: reqwest::Client,
}

impl BinaryManager {
    const VERSION_LOCK: &'static str = "version.lock";

    /// Platform-specific binary filename.
    #[cfg(target_os = "windows")]
    pub const BINARY_FILENAME: &'static str = "llama-server.exe";
    #[cfg(not(target_os = "windows"))]
    pub const BINARY_FILENAME: &'static str = "llama-server";

    /// Create a new `BinaryManager`.
    ///
    /// Creates `bin_dir` if it does not already exist.
    ///
    /// # Errors
    ///
    /// Returns an error if `bin_dir` cannot be created.
    pub fn new(bin_dir: PathBuf, client: reqwest::Client) -> Result<Self> {
        fs::create_dir_all(&bin_dir)
            .with_context(|| format!("could not create bin dir: {}", bin_dir.display()))?;
        Ok(Self { bin_dir, client })
    }

    /// Return the current status of the managed binary.
    ///
    /// # Errors
    ///
    /// Propagates I/O errors from reading the version lock file.
    pub fn status(&self) -> Result<BinaryStatus> {
        if !self.binary_path().exists() {
            return Ok(BinaryStatus::NotInstalled);
        }
        match self.read_version_lock()? {
            None => Ok(BinaryStatus::NotInstalled),
            Some(v) if v == SUPPORTED_LLAMA_VERSION => {
                Ok(BinaryStatus::Installed { version: v })
            }
            Some(installed) => Ok(BinaryStatus::OutOfDate {
                installed,
                pinned: SUPPORTED_LLAMA_VERSION.to_string(),
            }),
        }
    }

    /// Download and install the pinned `llama-server` binary for this platform.
    ///
    /// Steps:
    /// 1. Resolve the platform-specific release descriptor.
    /// 2. Download the zip archive into `bin_dir`.
    /// 3. Verify SHA256 if `release.sha256` is `Some`.
    /// 4. Extract the binary, rejecting any path-traversal entries.
    /// 5. Set the executable bit (Unix only).
    /// 6. Delete the archive.
    /// 7. Write `version.lock`.
    ///
    /// # Errors
    ///
    /// Returns an error on network failure, SHA256 mismatch, extraction
    /// failure, or filesystem errors.
    pub async fn install(&self) -> Result<()> {
        let release = Self::platform_release()?;

        // Derive the archive filename from the URL.
        let archive_name = release
            .url
            .rsplit('/')
            .next()
            .unwrap_or("llama-archive.zip")
            .to_string();
        let archive_path = self.bin_dir.join(&archive_name);

        // Download.
        self.download_archive(&release.url, &archive_path).await?;

        // SHA256 verification (skipped when None).
        if let Some(expected) = &release.sha256 {
            let actual = crate::downloader::compute_sha256(&archive_path)?;
            if actual != *expected {
                // Remove the corrupted archive before returning.
                let _ = fs::remove_file(&archive_path);
                bail!("SHA256 mismatch: expected {expected}, got {actual}");
            }
        }

        // Extract the binary.
        let binary_path = self.binary_path();
        extract_from_zip(&archive_path, &release.binary_entry, &binary_path)?;

        // Make executable on Unix.
        #[cfg(unix)]
        make_executable(&binary_path)?;

        // Clean up the archive.
        let _ = fs::remove_file(&archive_path);

        // Commit the version lock.
        self.write_version_lock(SUPPORTED_LLAMA_VERSION)?;

        Ok(())
    }

    /// Returns the full path to the `llama-server[.exe]` binary.
    pub fn binary_path(&self) -> PathBuf {
        self.bin_dir.join(Self::BINARY_FILENAME)
    }

    /// Read the installed version string from `version.lock`.
    ///
    /// Returns `None` if the lock file does not exist.
    ///
    /// # Errors
    ///
    /// Propagates I/O errors from reading the file.
    pub fn read_version_lock(&self) -> Result<Option<String>> {
        let path = self.lock_path();
        if !path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("cannot read version lock: {}", path.display()))?;
        Ok(Some(contents.trim().to_string()))
    }

    /// Write `version` to `version.lock`, overwriting any previous content.
    ///
    /// # Errors
    ///
    /// Propagates I/O errors from writing the file.
    pub fn write_version_lock(&self, version: &str) -> Result<()> {
        let path = self.lock_path();
        fs::write(&path, version)
            .with_context(|| format!("cannot write version lock: {}", path.display()))?;
        Ok(())
    }

    /// Return the release descriptor for the currently running platform.
    ///
    /// Supported combinations: Windows x86_64, macOS aarch64, macOS x86_64,
    /// Linux x86_64.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported OS/arch combinations.
    pub fn platform_release() -> Result<PlatformRelease> {
        let v = SUPPORTED_LLAMA_VERSION;
        let base =
            format!("https://github.com/ggerganov/llama.cpp/releases/download/{v}");

        let (asset, binary_entry) =
            match (std::env::consts::OS, std::env::consts::ARCH) {
                ("windows", "x86_64") => (
                    format!("llama-{v}-bin-win-avx2-x64.zip"),
                    "llama-server.exe".to_string(),
                ),
                ("macos", "aarch64") => (
                    format!("llama-{v}-bin-macos-arm64.zip"),
                    "llama-server".to_string(),
                ),
                ("macos", "x86_64") => (
                    format!("llama-{v}-bin-macos-x64.zip"),
                    "llama-server".to_string(),
                ),
                ("linux", "x86_64") => (
                    format!("llama-{v}-bin-ubuntu-x64.zip"),
                    "llama-server".to_string(),
                ),
                (os, arch) => bail!("unsupported platform: os={os} arch={arch}"),
            };

        Ok(PlatformRelease {
            url: format!("{base}/{asset}"),
            // llama.cpp releases do not publish machine-readable SHA256 sums.
            sha256: None,
            binary_entry,
        })
    }

    // ─── Private helpers ─────────────────────────────────────────────────────

    fn lock_path(&self) -> PathBuf {
        self.bin_dir.join(Self::VERSION_LOCK)
    }

    /// Stream-download `url` into `dest`.
    ///
    /// Uses `response.chunk()` to avoid loading the full archive into memory.
    async fn download_archive(&self, url: &str, dest: &Path) -> Result<()> {
        use std::io::Write as _;

        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("HTTP error for {url}"))?;

        let mut file = fs::File::create(dest)
            .with_context(|| format!("cannot create archive: {}", dest.display()))?;

        while let Some(chunk) = response.chunk().await.context("stream read error")? {
            file.write_all(&chunk)
                .with_context(|| format!("write error: {}", dest.display()))?;
        }

        Ok(())
    }
}

// ─── Archive extraction ───────────────────────────────────────────────────────

/// Extract the entry whose basename matches `binary_entry` from a zip archive.
///
/// Path-traversal entries (containing `..`, or starting with `/` or `\`) are
/// silently skipped. Returns an error if the target entry is not found.
fn extract_from_zip(archive: &Path, binary_entry: &str, dest: &Path) -> Result<()> {
    use std::io;

    let file = fs::File::open(archive)
        .with_context(|| format!("cannot open archive: {}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .with_context(|| format!("cannot read zip: {}", archive.display()))?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).context("zip entry error")?;
        let raw_name = entry.name().to_string();

        // Reject path-traversal and absolute paths.
        if raw_name.contains("..") || raw_name.starts_with('/') || raw_name.starts_with('\\') {
            continue;
        }

        // Match by basename only — ignore directory prefix inside the zip.
        let basename = raw_name.rsplit(['/', '\\']).next().unwrap_or(&raw_name);
        if basename != binary_entry {
            continue;
        }

        let mut out = fs::File::create(dest)
            .with_context(|| format!("cannot create binary: {}", dest.display()))?;
        io::copy(&mut entry, &mut out).context("extraction failed")?;
        return Ok(());
    }

    bail!("'{binary_entry}' not found in {}", archive.display())
}

/// Set the executable bit on a file (Unix only).
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .with_context(|| format!("cannot stat: {}", path.display()))?
        .permissions();
    perms.set_mode(perms.mode() | 0o755);
    fs::set_permissions(path, perms)
        .with_context(|| format!("cannot chmod: {}", path.display()))?;
    Ok(())
}