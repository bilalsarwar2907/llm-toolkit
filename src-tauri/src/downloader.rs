/// downloader.rs
///
/// Hardened model downloader.
///
/// Responsibilities:
/// - Preflight disk-space check before downloading (D-05 guard).
/// - Resume support via `Range` headers and `.part` files.
/// - Recovery metadata persisted in `pending_resume.json`.
/// - Progress events emitted on a tokio mpsc channel.
/// - Post-download validation: SHA256 → ETag → file size → GGUF magic.
/// - Startup recovery scan for interrupted downloads.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use sysinfo::Disks;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::registry::DownloadGuard;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Partial files not updated within this window are considered stale.
pub const PART_RETENTION_SECS: u64 = 14 * 24 * 60 * 60;
/// Fraction of content-length added as overhead for required disk space.
const DISK_OVERHEAD_FACTOR: f64 = 1.10;
/// Fixed buffer added on top of overhead.
const DISK_BUFFER_BYTES: u64 = 500 * 1024 * 1024; // 500 MB
/// GGUF file format magic bytes (Tier 4 validation).
const GGUF_MAGIC: &[u8; 4] = b"GGUF";
/// Filename of the resume manifest in the model directory.
const RESUME_MANIFEST: &str = "pending_resume.json";
/// How often to persist the manifest during an active download.
const MANIFEST_UPDATE_INTERVAL: u64 = 5 * 1024 * 1024; // every 5 MB

// ─── Public types ─────────────────────────────────────────────────────────────

/// Parameters for a single model download.
pub struct DownloadRequest {
    /// Direct download URL.
    pub url: String,
    /// Destination filename (no directory component; placed inside `model_dir`).
    pub filename: String,
    /// Expected SHA256 hex digest (lowercase). `None` → Tier 1 check skipped.
    pub expected_sha256: Option<String>,
    /// Expected ETag from a prior HEAD response. `None` → Tier 2 check skipped.
    pub expected_etag: Option<String>,
    /// Directory where the model file will be saved.
    pub model_dir: PathBuf,
}

/// Progress snapshot emitted on the channel during a download.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub filename: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    /// Instantaneous speed in bytes/second (averaged over the last ~1 second).
    pub speed_bps: f64,
}

/// Events emitted on the progress channel.
#[derive(Debug)]
pub enum DownloadEvent {
    Progress(DownloadProgress),
    Complete { validation: ValidationOutcome },
    Cancelled,
    Failed(String),
}

/// Result of the post-download validation hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationOutcome {
    /// Tier 1: SHA256 matched. Highest confidence.
    Sha256Verified,
    /// Tiers 3 & 4 passed; no SHA256 available for Tier 1.
    StructuralPass,
    /// Tier 2: ETag differed from expected (warning — file retained).
    ETagMismatch { expected: String, actual: String },
}

/// One entry in `pending_resume.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeEntry {
    pub filename: String,
    pub url: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    /// Unix timestamp (seconds) when the download was first started.
    pub started_at_secs: u64,
    /// Unix timestamp (seconds) of the last manifest write.
    pub last_updated_secs: u64,
    pub expected_sha256: Option<String>,
    pub expected_etag: Option<String>,
}

/// The full `pending_resume.json` manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResumeManifest {
    pub entries: Vec<ResumeEntry>,
}

/// Information about a `.part` file found during a startup recovery scan.
#[derive(Debug, Clone)]
pub struct PartialDownloadInfo {
    pub filename: String,
    pub bytes_on_disk: u64,
    pub total_bytes: Option<u64>,
    pub last_updated_secs: u64,
    /// True if the file has not been updated in more than 14 days.
    pub is_stale: bool,
}

// ─── Private types ────────────────────────────────────────────────────────────

struct PreflightResult {
    content_length: Option<u64>,
    etag: Option<String>,
    supports_range: bool,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Download a model file with preflight disk check, resume support,
/// cancellation, progress events, and post-download validation.
///
/// The caller must hold the `DownloadGuard` for `request.filename` before
/// calling this function. The guard is used only to check cancellation;
/// releasing it is the caller's responsibility.
///
/// Progress events are sent on `progress_tx`. Dropping the receiver silently
/// discards events but does not abort the download.
///
/// # Errors
///
/// - Insufficient disk space (preflight).
/// - HTTP errors.
/// - Validation failures.
/// - I/O errors.
pub async fn download(
    client: &reqwest::Client,
    request: DownloadRequest,
    guard: &DownloadGuard,
    progress_tx: mpsc::Sender<DownloadEvent>,
) -> Result<ValidationOutcome> {
    // Ensure model directory exists.
    tokio::fs::create_dir_all(&request.model_dir)
        .await
        .with_context(|| {
            format!(
                "could not create model directory: {}",
                request.model_dir.display()
            )
        })?;

    let part_path = request.model_dir.join(format!("{}.part", request.filename));
    let final_path = request.model_dir.join(&request.filename);

    // ── Preflight ─────────────────────────────────────────────────────────────
    let preflight = preflight_head(client, &request.url).await?;

    if let Some(content_length) = preflight.content_length {
        check_disk_space(&request.model_dir, content_length)?;
    }

    // ── Resume ────────────────────────────────────────────────────────────────
    let mut manifest = load_resume_manifest(&request.model_dir).unwrap_or_default();

    let resume_from = if part_path.exists() && preflight.supports_range {
        std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    let now = unix_now_secs();
    let started_at = manifest
        .entries
        .iter()
        .find(|e| e.filename == request.filename)
        .map_or(now, |e| e.started_at_secs);

    manifest.upsert(ResumeEntry {
        filename: request.filename.clone(),
        url: request.url.clone(),
        bytes_downloaded: resume_from,
        total_bytes: preflight.content_length,
        started_at_secs: started_at,
        last_updated_secs: now,
        expected_sha256: request.expected_sha256.clone(),
        expected_etag: request.expected_etag.clone(),
    });
    save_resume_manifest(&request.model_dir, &manifest)?;

    // ── GET request ───────────────────────────────────────────────────────────
    let mut req = client.get(&request.url);
    if resume_from > 0 {
        req = req.header("Range", format!("bytes={resume_from}-"));
    }

    let response = req.send().await.context("GET request failed")?;
    let status = response.status();

    let actual_etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let is_fresh = match status.as_u16() {
        200 => true,  // Full content; ignore resume offset.
        206 => false, // Partial content; server accepted the Range header.
        _ => bail!("unexpected HTTP status {} for '{}'", status, request.url),
    };

    let effective_resume_from = if is_fresh { 0 } else { resume_from };

    // ── Open part file ────────────────────────────────────────────────────────
    let mut file = if is_fresh || effective_resume_from == 0 {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&part_path)
            .await
            .with_context(|| format!("could not create part file: {}", part_path.display()))?
    } else {
        OpenOptions::new()
            .append(true)
            .open(&part_path)
            .await
            .with_context(|| {
                format!(
                    "could not open part file for append: {}",
                    part_path.display()
                )
            })?
    };

    // ── Download loop ─────────────────────────────────────────────────────────
    let mut bytes_downloaded = effective_resume_from;
    let mut bytes_since_manifest = 0u64;
    let mut speed_timer = Instant::now();
    let mut bytes_since_speed = 0u64;
    let mut speed_bps = 0.0_f64;
    let mut response = response;

    loop {
        if guard.is_cancelled() {
            file.flush().await.ok();
            persist_manifest_entry(
                &mut manifest,
                &request,
                bytes_downloaded,
                preflight.content_length,
                started_at,
            );
            save_resume_manifest(&request.model_dir, &manifest).ok();
            let _ = progress_tx.send(DownloadEvent::Cancelled).await;
            bail!("download cancelled by user");
        }

        let chunk = match response.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break, // EOF — download complete.
            Err(e) => {
                file.flush().await.ok();
                persist_manifest_entry(
                    &mut manifest,
                    &request,
                    bytes_downloaded,
                    preflight.content_length,
                    started_at,
                );
                save_resume_manifest(&request.model_dir, &manifest).ok();
                let _ = progress_tx
                    .send(DownloadEvent::Failed(e.to_string()))
                    .await;
                return Err(anyhow::Error::new(e).context("network error during download"));
            }
        };

        file.write_all(&chunk)
            .await
            .context("I/O error writing to part file")?;

        let len = chunk.len() as u64;
        bytes_downloaded += len;
        bytes_since_manifest += len;
        bytes_since_speed += len;

        let elapsed = speed_timer.elapsed();
        if elapsed.as_secs_f64() >= 1.0 {
            speed_bps = bytes_since_speed as f64 / elapsed.as_secs_f64();
            bytes_since_speed = 0;
            speed_timer = Instant::now();
        }

        let _ = progress_tx
            .send(DownloadEvent::Progress(DownloadProgress {
                filename: request.filename.clone(),
                bytes_downloaded,
                total_bytes: preflight.content_length,
                speed_bps,
            }))
            .await;

        if bytes_since_manifest >= MANIFEST_UPDATE_INTERVAL {
            bytes_since_manifest = 0;
            persist_manifest_entry(
                &mut manifest,
                &request,
                bytes_downloaded,
                preflight.content_length,
                started_at,
            );
            save_resume_manifest(&request.model_dir, &manifest).ok();
        }
    }

    file.flush().await.context("could not flush part file")?;
    drop(file);

    // ── Validation ────────────────────────────────────────────────────────────
   // Use caller-provided ETag if available; fall back to the preflight HEAD ETag.
    let expected_etag = request
        .expected_etag
        .as_deref()
        .or(preflight.etag.as_deref());

    let validation = validate_file(
        &part_path,
        request.expected_sha256.as_deref(),
        expected_etag,
        actual_etag.as_deref(),
        preflight.content_length,
    );

    match validation {
        Ok(outcome) => {
            std::fs::rename(&part_path, &final_path).with_context(|| {
                format!(
                    "could not rename {} to {}",
                    part_path.display(),
                    final_path.display()
                )
            })?;
            manifest.remove(&request.filename);
            save_resume_manifest(&request.model_dir, &manifest).ok();
            let _ = progress_tx
                .send(DownloadEvent::Complete {
                    validation: outcome.clone(),
                })
                .await;
            Ok(outcome)
        }
        Err(e) => {
            // Validation failed: delete the bad file.
            std::fs::remove_file(&part_path).ok();
            manifest.remove(&request.filename);
            save_resume_manifest(&request.model_dir, &manifest).ok();
            let _ = progress_tx
                .send(DownloadEvent::Failed(e.to_string()))
                .await;
            Err(e)
        }
    }
}

/// Scan `model_dir` for `.part` files and return recovery information.
///
/// Cross-references each file with `pending_resume.json` to populate
/// size and staleness data. Used at startup to offer resume / discard.
///
/// # Errors
///
/// Returns an error if `model_dir` cannot be read.
pub fn scan_partial_downloads(model_dir: &Path) -> Result<Vec<PartialDownloadInfo>> {
    let manifest = load_resume_manifest(model_dir).unwrap_or_default();
    let now = unix_now_secs();
    let mut results = Vec::new();

    let read_dir = std::fs::read_dir(model_dir).with_context(|| {
        format!(
            "could not read model directory: {}",
            model_dir.display()
        )
    })?;

    for entry in read_dir.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".part") {
            continue;
        }

        let filename = name.trim_end_matches(".part").to_string();
        let bytes_on_disk = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let resume = manifest.entries.iter().find(|e| e.filename == filename);
        let last_updated = resume.map_or(0, |e| e.last_updated_secs);
        let total_bytes = resume.and_then(|e| e.total_bytes);

        results.push(PartialDownloadInfo {
            filename,
            bytes_on_disk,
            total_bytes,
            last_updated_secs: last_updated,
            is_stale: now.saturating_sub(last_updated) > PART_RETENTION_SECS,
        });
    }

    Ok(results)
}

/// Delete a partial download and remove its manifest entry.
///
/// Safe to call even if the files do not exist.
///
/// # Errors
///
/// Returns an error if the `.part` file exists but cannot be deleted.
pub fn discard_partial(model_dir: &Path, filename: &str) -> Result<()> {
    let part = model_dir.join(format!("{filename}.part"));
    if part.exists() {
        std::fs::remove_file(&part)
            .with_context(|| format!("could not delete {}", part.display()))?;
    }
    let mut manifest = load_resume_manifest(model_dir).unwrap_or_default();
    manifest.remove(filename);
    save_resume_manifest(model_dir, &manifest)
}

// ─── Public (crate-visible) helpers used in tests ────────────────────────────

/// Verify that `model_dir` has enough free space for a `content_length`-byte file.
///
/// Required = content_length × 1.10 + 500 MB buffer.
///
/// # Errors
///
/// Returns a user-facing error message if space is insufficient.
pub(crate) fn check_disk_space(model_dir: &Path, content_length: u64) -> Result<()> {
    let required = required_bytes(content_length);
    let free = free_bytes_for_path(model_dir);

    if free < required {
        let gib = |b: u64| b as f64 / 1_073_741_824.0;
        bail!(
            "Insufficient disk space to download to '{}'. \
             Required: {:.1} GiB, available: {:.1} GiB. \
             Free up space and try again.",
            model_dir.display(),
            gib(required),
            gib(free),
        );
    }
    Ok(())
}

/// Calculate required disk space for a `content_length`-byte download.
pub(crate) fn required_bytes(content_length: u64) -> u64 {
    let overhead = (content_length as f64 * DISK_OVERHEAD_FACTOR) as u64;
    overhead + DISK_BUFFER_BYTES
}

/// Compute the SHA256 hex digest (lowercase) of a file.
///
/// # Errors
///
/// Returns an error on I/O failure.
pub(crate) fn compute_sha256(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("could not open {} for SHA256", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 65_536];
    loop {
        let n = file.read(&mut buf).context("read error during SHA256")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Return `true` if `path` begins with the GGUF magic bytes (`GGUF`).
///
/// # Errors
///
/// Returns an error if the file cannot be opened or is shorter than 4 bytes.
pub(crate) fn check_gguf_magic(path: &Path) -> Result<bool> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("could not open {} for magic check", path.display()))?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)
        .with_context(|| format!("could not read magic bytes from {}", path.display()))?;
    Ok(&magic == GGUF_MAGIC)
}

// ─── Manifest helpers ─────────────────────────────────────────────────────────

impl ResumeManifest {
    /// Insert or replace the entry for `entry.filename`.
    pub fn upsert(&mut self, entry: ResumeEntry) {
        if let Some(slot) = self.entries.iter_mut().find(|e| e.filename == entry.filename) {
            *slot = entry;
        } else {
            self.entries.push(entry);
        }
    }

    /// Remove the entry for `filename`, if present.
    pub fn remove(&mut self, filename: &str) {
        self.entries.retain(|e| e.filename != filename);
    }
}

/// Load `pending_resume.json` from `model_dir`.
/// Returns an empty manifest if the file does not exist.
pub fn load_resume_manifest(model_dir: &Path) -> Result<ResumeManifest> {
    let path = model_dir.join(RESUME_MANIFEST);
    if !path.exists() {
        return Ok(ResumeManifest::default());
    }
    let file = std::fs::File::open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    serde_json::from_reader(file)
        .with_context(|| format!("could not parse {}", path.display()))
}

/// Write `manifest` to `pending_resume.json` in `model_dir`.
pub fn save_resume_manifest(model_dir: &Path, manifest: &ResumeManifest) -> Result<()> {
    let path = model_dir.join(RESUME_MANIFEST);
    let file = std::fs::File::create(&path)
        .with_context(|| format!("could not create {}", path.display()))?;
    serde_json::to_writer_pretty(file, manifest)
        .with_context(|| format!("could not write {}", path.display()))
}

// ─── Private helpers ──────────────────────────────────────────────────────────

async fn preflight_head(client: &reqwest::Client, url: &str) -> Result<PreflightResult> {
    let response = client
        .head(url)
        .send()
        .await
        .with_context(|| format!("HEAD request failed for '{url}'"))?;

    let content_length = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let supports_range = response
        .headers()
        .get("accept-ranges")
        .and_then(|v| v.to_str().ok())
        .map_or(false, |v| v.to_lowercase() != "none");

    Ok(PreflightResult {
        content_length,
        etag,
        supports_range,
    })
}

fn validate_file(
    path: &Path,
    expected_sha256: Option<&str>,
    expected_etag: Option<&str>,
    actual_etag: Option<&str>,
    expected_size: Option<u64>,
) -> Result<ValidationOutcome> {
    // Tier 1: SHA256.
    if let Some(expected) = expected_sha256 {
        let actual = compute_sha256(path)
            .with_context(|| format!("could not compute SHA256 of {}", path.display()))?;
        if actual.to_lowercase() != expected.to_lowercase() {
            bail!(
                "SHA256 mismatch for '{}': expected {expected}, got {actual}",
                path.display()
            );
        }
        return Ok(ValidationOutcome::Sha256Verified);
    }

    // Tier 2: ETag (warning only — do not reject).
    let etag_mismatch = match (expected_etag, actual_etag) {
        (Some(exp), Some(act)) if exp != act => Some(ValidationOutcome::ETagMismatch {
            expected: exp.to_string(),
            actual: act.to_string(),
        }),
        _ => None,
    };

    // Tier 3: file-size consistency.
    if let Some(expected_bytes) = expected_size {
        let actual_size = std::fs::metadata(path)
            .with_context(|| format!("could not stat {}", path.display()))?
            .len();
        if actual_size != expected_bytes {
            bail!(
                "File size mismatch for '{}': expected {expected_bytes} bytes, got {actual_size}",
                path.display()
            );
        }
    }

    // Tier 4: GGUF magic.
    if !check_gguf_magic(path)
        .with_context(|| format!("could not read magic bytes from {}", path.display()))?
    {
        bail!(
            "'{}' is not a valid GGUF file (magic bytes do not match)",
            path.display()
        );
    }

    if let Some(mismatch) = etag_mismatch {
        return Ok(mismatch);
    }

    Ok(ValidationOutcome::StructuralPass)
}

fn free_bytes_for_path(path: &Path) -> u64 {
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64)> = None;
    let mut candidate = Some(path);

    while let Some(p) = candidate {
        for disk in disks.list() {
            let mount = disk.mount_point();
            if p == mount {
                let len = mount.as_os_str().len();
                if best.map_or(true, |(l, _)| len > l) {
                    best = Some((len, disk.available_space()));
                }
            }
        }
        if best.is_some() {
            break;
        }
        candidate = p.parent();
    }

    best.map_or_else(
        || {
            disks
                .list()
                .iter()
                .map(|d| d.available_space())
                .max()
                .unwrap_or(0)
        },
        |(_, free)| free,
    )
}

fn persist_manifest_entry(
    manifest: &mut ResumeManifest,
    request: &DownloadRequest,
    bytes_downloaded: u64,
    total_bytes: Option<u64>,
    started_at_secs: u64,
) {
    manifest.upsert(ResumeEntry {
        filename: request.filename.clone(),
        url: request.url.clone(),
        bytes_downloaded,
        total_bytes,
        started_at_secs,
        last_updated_secs: unix_now_secs(),
        expected_sha256: request.expected_sha256.clone(),
        expected_etag: request.expected_etag.clone(),
    });
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}