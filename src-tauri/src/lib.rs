#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]

pub mod binary;
pub mod catalog;
pub mod downloader;
pub mod hardware;
pub mod registry;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}! You've been greeted from Rust!")
}

/// Runs the Tauri application.
///
/// # Panics
///
/// Panics if the Tauri application fails to start.
#[allow(clippy::expect_used)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod hardware_tests {
    use crate::hardware;
    use std::path::PathBuf;

    fn storage_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".llm-toolkit")
            .join("models")
    }

    #[test]
    fn smoke_collect() {
        let r = hardware::collect(storage_path()).expect("collect failed");
        assert!(!r.cpu_model.is_empty(), "cpu_model must not be empty");
        assert!(r.cpu_cores > 0, "cpu_cores must be > 0");
        assert!(!r.cpu_arch.is_empty(), "cpu_arch must not be empty");
        assert!(r.ram_total_gb > 0.0, "ram_total_gb must be > 0");
        assert!(!r.os_name.is_empty(), "os_name must not be empty");
    }

    #[test]
    fn serde_round_trip() {
        let r = hardware::collect(storage_path()).expect("collect failed");
        let json = serde_json::to_string(&r).expect("serialize failed");
        let restored: hardware::HardwareReport =
            serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(r.cpu_model, restored.cpu_model);
        assert_eq!(r.cpu_cores, restored.cpu_cores);
        assert_eq!(r.ram_total_gb, restored.ram_total_gb);
        assert_eq!(r.os_name, restored.os_name);
        assert_eq!(r.disk_free_gb, restored.disk_free_gb);
    }

    #[test]
    fn bytes_to_gib_conversion() {
        let sixteen_gib: u64 = 16 * 1_073_741_824;
        assert_eq!(hardware::bytes_to_gib(sixteen_gib), 16.0);
        assert_eq!(hardware::bytes_to_gib(0), 0.0);
    }

    #[test]
    fn disk_fallback_on_unknown_path() {
        let fake = PathBuf::from("/nonexistent/path/that/does/not/match");
        let result = hardware::collect(fake);
        assert!(result.is_ok(), "collect must not error on non-existent path");
    }
}

#[cfg(test)]
mod catalog_tests {
    use crate::catalog::{self, Category};

    #[test]
    fn default_catalog_loads() {
        let cat = catalog::load_default().expect("default catalog must load");
        assert!(!cat.models.is_empty(), "catalog must contain at least one model");
    }

    #[test]
    fn all_models_have_valid_fields() {
        let cat = catalog::load_default().expect("load failed");
        for m in &cat.models {
            assert!(!m.repo_id.is_empty(), "repo_id must not be empty");
            assert!(!m.filename.is_empty(), "filename must not be empty");
            assert!(m.filename.ends_with(".gguf"), "filename must end with .gguf");
            assert!(!m.parameter_count.is_empty(), "parameter_count must not be empty");
            assert!(!m.quant.is_empty(), "quant must not be empty");
            assert!(m.approx_size_gb > 0.0, "approx_size_gb must be > 0");
            assert!(m.min_ram_gb > 0, "min_ram_gb must be > 0");
        }
    }

    #[test]
    fn advanced_models_require_sufficient_ram() {
        let cat = catalog::load_default().expect("load failed");
        for m in cat.models.iter().filter(|m| m.category == Category::Advanced) {
            assert!(
                m.min_ram_gb >= 24,
                "advanced model {} must require >= 24 GB RAM, got {}",
                m.filename,
                m.min_ram_gb
            );
        }
    }

    #[test]
    fn recommended_models_fit_typical_hardware() {
        let cat = catalog::load_default().expect("load failed");
        for m in cat.models.iter().filter(|m| m.category == Category::Recommended) {
            assert!(
                m.min_ram_gb <= 16,
                "recommended model {} has min_ram_gb={}, exceeds 16 GB laptop threshold",
                m.filename,
                m.min_ram_gb
            );
        }
    }

    #[test]
    fn invalid_toml_returns_error() {
        let result = catalog::parse("this is not valid toml ][[[");
        assert!(result.is_err(), "invalid TOML must return Err");
    }

    #[test]
    fn unknown_category_returns_error() {
        let bad = r#"
[[models]]
repo_id         = "owner/repo"
filename        = "model.gguf"
parameter_count = "7B"
quant           = "Q4_K_M"
approx_size_gb  = 4.1
category        = "superadvanced"
min_ram_gb      = 8
"#;
        let result = catalog::parse(bad);
        assert!(result.is_err(), "unknown category must return Err");
    }
}

#[cfg(test)]
mod registry_tests {
    use crate::registry::{AcquireResult, Registry};
    use std::sync::atomic::Ordering;

    fn test_registry() -> Registry {
        let lock_dir = std::env::temp_dir().join("llm-toolkit-test-locks");
        Registry::new(lock_dir).expect("registry creation failed")
    }

    #[test]
    fn acquire_new_filename_succeeds() {
        let reg = test_registry();
        let result = reg.try_acquire("model-a.gguf").expect("try_acquire failed");
        assert!(matches!(result, AcquireResult::Acquired(_)), "expected Acquired");
    }

    #[test]
    fn duplicate_acquire_returns_already_active() {
        let reg = test_registry();
        let first = reg.try_acquire("model-b.gguf").expect("first acquire failed");
        assert!(matches!(first, AcquireResult::Acquired(_)));
        let second = reg.try_acquire("model-b.gguf").expect("second acquire failed");
        assert!(
            matches!(second, AcquireResult::AlreadyActive),
            "expected AlreadyActive on second acquire"
        );
    }

    #[test]
    fn drop_guard_releases_slot() {
        let reg = test_registry();
        {
            let result = reg.try_acquire("model-c.gguf").expect("acquire failed");
            assert!(matches!(result, AcquireResult::Acquired(_)));
        }
        let result = reg.try_acquire("model-c.gguf").expect("re-acquire failed");
        assert!(matches!(result, AcquireResult::Acquired(_)), "expected Acquired after drop");
    }

    #[test]
    fn cancel_via_guard_sets_flag() {
        let reg = test_registry();
        if let AcquireResult::Acquired(guard) =
            reg.try_acquire("model-d.gguf").expect("acquire failed")
        {
            assert!(!guard.is_cancelled());
            guard.cancel();
            assert!(guard.is_cancelled());
        } else {
            panic!("expected Acquired");
        }
    }

    #[test]
    fn cancellation_token_reflects_guard_state() {
        let reg = test_registry();
        if let AcquireResult::Acquired(guard) =
            reg.try_acquire("model-e.gguf").expect("acquire failed")
        {
            let token = guard.cancellation_token();
            assert!(!token.load(Ordering::SeqCst));
            guard.cancel();
            assert!(token.load(Ordering::SeqCst));
        } else {
            panic!("expected Acquired");
        }
    }

    #[test]
    fn cancel_via_registry_sets_flag() {
        let reg = test_registry();
        if let AcquireResult::Acquired(guard) =
            reg.try_acquire("model-f.gguf").expect("acquire failed")
        {
            let token = guard.cancellation_token();
            let found = reg.cancel("model-f.gguf");
            assert!(found, "cancel must return true for active download");
            assert!(token.load(Ordering::SeqCst));
        } else {
            panic!("expected Acquired");
        }
    }

    #[test]
    fn is_active_reflects_guard_lifetime() {
        let reg = test_registry();
        {
            let result = reg.try_acquire("model-g.gguf").expect("acquire failed");
            assert!(matches!(result, AcquireResult::Acquired(_)));
            assert!(reg.is_active("model-g.gguf"));
        }
        assert!(!reg.is_active("model-g.gguf"));
    }
}

#[cfg(test)]
mod downloader_tests {
    use crate::downloader::{self, ResumeEntry, ResumeManifest};
    use std::io::Write as _;
    use std::path::PathBuf;

    /// Create a unique temporary directory for each test.
    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "llm-toolkit-dl-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("could not create tmp dir");
        dir
    }

    fn sample_entry(filename: &str) -> ResumeEntry {
        ResumeEntry {
            filename: filename.to_string(),
            url: format!("https://example.com/{filename}"),
            bytes_downloaded: 1000,
            total_bytes: Some(5000),
            started_at_secs: 1_000_000,
            last_updated_secs: 1_000_100,
            expected_sha256: None,
            expected_etag: None,
        }
    }

    /// required_bytes must equal content_length * 1.10 + 500 MB.
    #[test]
    fn required_bytes_formula() {
        let one_gib: u64 = 1_073_741_824;
        let result = downloader::required_bytes(one_gib);
        let expected = (one_gib as f64 * 1.10) as u64 + 500 * 1024 * 1024;
        assert_eq!(result, expected);
    }

    /// check_disk_space must not panic for zero-byte content on any real disk.
    #[test]
    fn check_disk_space_zero_content_does_not_panic() {
        let _ = downloader::check_disk_space(&std::env::temp_dir(), 0);
    }

    /// upsert inserts a new entry and updates an existing one.
    #[test]
    fn manifest_upsert_insert_and_update() {
        let mut m = ResumeManifest::default();
        m.upsert(sample_entry("a.gguf"));
        assert_eq!(m.entries.len(), 1);

        // Update in place.
        let mut updated = sample_entry("a.gguf");
        updated.bytes_downloaded = 9999;
        m.upsert(updated);
        assert_eq!(m.entries.len(), 1, "upsert must not duplicate");
        assert_eq!(m.entries[0].bytes_downloaded, 9999);
    }

    /// remove deletes the correct entry without affecting others.
    #[test]
    fn manifest_remove_correct_entry() {
        let mut m = ResumeManifest::default();
        m.upsert(sample_entry("a.gguf"));
        m.upsert(sample_entry("b.gguf"));
        m.remove("a.gguf");
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].filename, "b.gguf");
    }

    /// save + load round-trip preserves all fields.
    #[test]
    fn manifest_round_trips_to_disk() {
        let dir = tmp_dir();
        let mut m = ResumeManifest::default();
        let mut e = sample_entry("test.gguf");
        e.expected_sha256 = Some("abc123".to_string());
        m.upsert(e);

        downloader::save_resume_manifest(&dir, &m).expect("save failed");
        let loaded = downloader::load_resume_manifest(&dir).expect("load failed");

        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].filename, "test.gguf");
        assert_eq!(loaded.entries[0].bytes_downloaded, 1000);
        assert_eq!(
            loaded.entries[0].expected_sha256.as_deref(),
            Some("abc123")
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// load returns an empty manifest when the file does not exist.
    #[test]
    fn manifest_load_missing_file_returns_default() {
        let dir = tmp_dir();
        let result = downloader::load_resume_manifest(&dir).expect("load failed");
        assert!(result.entries.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// check_gguf_magic returns true for a file that starts with `GGUF`.
    #[test]
    fn gguf_magic_valid() {
        let dir = tmp_dir();
        let path = dir.join("valid.gguf");
        let mut f = std::fs::File::create(&path).expect("create failed");
        f.write_all(b"GGUF\x00\x00\x00\x00").expect("write failed");
        drop(f);

        assert!(
            downloader::check_gguf_magic(&path).expect("magic check failed"),
            "valid GGUF magic must return true"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// check_gguf_magic returns false for a file with wrong magic bytes.
    #[test]
    fn gguf_magic_invalid() {
        let dir = tmp_dir();
        let path = dir.join("invalid.bin");
        let mut f = std::fs::File::create(&path).expect("create failed");
        f.write_all(b"NOTG\x00\x00\x00\x00").expect("write failed");
        drop(f);

        assert!(
            !downloader::check_gguf_magic(&path).expect("magic check failed"),
            "invalid magic must return false"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// SHA256 is deterministic: same content → same hash; different → different.
    #[test]
    fn sha256_deterministic_and_unique() {
        let dir = tmp_dir();
        let path_a1 = dir.join("a1.bin");
        let path_a2 = dir.join("a2.bin");
        let path_b = dir.join("b.bin");

        std::fs::write(&path_a1, b"content-A").expect("write failed");
        std::fs::write(&path_a2, b"content-A").expect("write failed");
        std::fs::write(&path_b, b"content-B").expect("write failed");

        let h_a1 = downloader::compute_sha256(&path_a1).expect("sha256 failed");
        let h_a2 = downloader::compute_sha256(&path_a2).expect("sha256 failed");
        let h_b = downloader::compute_sha256(&path_b).expect("sha256 failed");

        assert_eq!(h_a1, h_a2, "same content → same hash");
        assert_ne!(h_a1, h_b, "different content → different hash");
        assert_eq!(h_a1.len(), 64, "SHA256 hex must be 64 characters");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// scan_partial_downloads finds .part files and cross-references the manifest.
    #[test]
    fn scan_finds_part_files() {
        let dir = tmp_dir();
        std::fs::write(dir.join("model.gguf.part"), b"partial data").expect("write failed");

        let mut m = ResumeManifest::default();
        let mut e = sample_entry("model.gguf");
        e.bytes_downloaded = 12;
        e.total_bytes = Some(1000);
        m.upsert(e);
        downloader::save_resume_manifest(&dir, &m).expect("save failed");

        // A completed file should not appear in the scan.
        std::fs::write(dir.join("other.gguf"), b"complete").expect("write failed");

        let partials = downloader::scan_partial_downloads(&dir).expect("scan failed");
        assert_eq!(partials.len(), 1);
        assert_eq!(partials[0].filename, "model.gguf");
        assert_eq!(partials[0].bytes_on_disk, 12);
        assert_eq!(partials[0].total_bytes, Some(1000));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// discard_partial removes the .part file and the manifest entry.
    #[test]
    fn discard_partial_removes_file_and_entry() {
        let dir = tmp_dir();
        let part = dir.join("model.gguf.part");
        std::fs::write(&part, b"partial").expect("write failed");

        let mut m = ResumeManifest::default();
        m.upsert(sample_entry("model.gguf"));
        downloader::save_resume_manifest(&dir, &m).expect("save failed");

        downloader::discard_partial(&dir, "model.gguf").expect("discard failed");

        assert!(!part.exists(), ".part file must be deleted");
        let reloaded = downloader::load_resume_manifest(&dir).expect("load failed");
        assert!(reloaded.entries.is_empty(), "manifest entry must be removed");

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod binary_manager_tests {
    use crate::binary::manager::{BinaryManager, BinaryStatus, SUPPORTED_LLAMA_VERSION};
    use std::path::PathBuf;

    /// Create a unique temporary directory for each test.
    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "llm-toolkit-bin-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("could not create tmp dir");
        dir
    }

    fn make_manager(bin_dir: PathBuf) -> BinaryManager {
        BinaryManager::new(bin_dir, reqwest::Client::new()).expect("manager creation failed")
    }

    // ─── platform_release ────────────────────────────────────────────────────

    /// platform_release must succeed on the current build target.
    #[test]
    fn platform_release_current_os_supported() {
        BinaryManager::platform_release().expect("platform_release must succeed on CI target");
    }

    /// The URL must use HTTPS and contain the pinned version string.
    #[test]
    fn platform_release_url_is_valid() {
        let rel = BinaryManager::platform_release().expect("platform_release failed");
        assert!(
            rel.url.starts_with("https://"),
            "URL must use HTTPS, got: {}",
            rel.url
        );
        assert!(
            rel.url.contains(SUPPORTED_LLAMA_VERSION),
            "URL must contain the pinned version '{}', got: {}",
            SUPPORTED_LLAMA_VERSION,
            rel.url
        );
        assert!(
            rel.url.ends_with(".zip"),
            "URL must point to a zip archive, got: {}",
            rel.url
        );
    }

    /// binary_entry must be the correct executable filename for this platform.
    #[test]
    fn platform_release_binary_entry_matches_os() {
        let rel = BinaryManager::platform_release().expect("platform_release failed");
        #[cfg(target_os = "windows")]
        assert_eq!(rel.binary_entry, "llama-server.exe");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(rel.binary_entry, "llama-server");
    }

    // ─── BinaryManager::new ──────────────────────────────────────────────────

    /// new() must create the bin_dir if it does not exist.
    #[test]
    fn new_creates_bin_dir() {
        let dir = tmp_dir();
        let bin_dir = dir.join("nested").join("bin");
        assert!(!bin_dir.exists(), "bin_dir must not pre-exist");
        make_manager(bin_dir.clone());
        assert!(bin_dir.exists(), "new() must create bin_dir");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ─── version lock ────────────────────────────────────────────────────────

    /// read_version_lock returns None when the lock file is absent.
    #[test]
    fn version_lock_missing_returns_none() {
        let dir = tmp_dir();
        let mgr = make_manager(dir.clone());
        let v = mgr.read_version_lock().expect("read_version_lock failed");
        assert!(v.is_none(), "expected None for missing lock file");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// write then read returns the same version string.
    #[test]
    fn version_lock_round_trip() {
        let dir = tmp_dir();
        let mgr = make_manager(dir.clone());
        mgr.write_version_lock("b1234").expect("write failed");
        let v = mgr.read_version_lock().expect("read failed");
        assert_eq!(v.as_deref(), Some("b1234"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// write_version_lock overwrites a previous value.
    #[test]
    fn version_lock_overwrites() {
        let dir = tmp_dir();
        let mgr = make_manager(dir.clone());
        mgr.write_version_lock("b0001").expect("first write failed");
        mgr.write_version_lock("b9999").expect("second write failed");
        let v = mgr.read_version_lock().expect("read failed");
        assert_eq!(v.as_deref(), Some("b9999"));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ─── status ──────────────────────────────────────────────────────────────

    /// status() is NotInstalled when the binary is absent.
    #[test]
    fn status_not_installed_no_binary() {
        let dir = tmp_dir();
        let mgr = make_manager(dir.clone());
        let s = mgr.status().expect("status failed");
        assert_eq!(s, BinaryStatus::NotInstalled);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// status() is NotInstalled when the binary exists but the lock is absent.
    #[test]
    fn status_not_installed_no_lock() {
        let dir = tmp_dir();
        let mgr = make_manager(dir.clone());
        // Create a dummy binary file.
        std::fs::write(mgr.binary_path(), b"dummy").expect("write failed");
        let s = mgr.status().expect("status failed");
        assert_eq!(s, BinaryStatus::NotInstalled);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// status() is Installed when the binary exists and the lock matches the pin.
    #[test]
    fn status_installed_when_lock_matches_pin() {
        let dir = tmp_dir();
        let mgr = make_manager(dir.clone());
        std::fs::write(mgr.binary_path(), b"dummy").expect("write failed");
        mgr.write_version_lock(SUPPORTED_LLAMA_VERSION)
            .expect("write lock failed");
        let s = mgr.status().expect("status failed");
        assert_eq!(
            s,
            BinaryStatus::Installed {
                version: SUPPORTED_LLAMA_VERSION.to_string()
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// status() is OutOfDate when the binary exists but the lock differs from the pin.
    #[test]
    fn status_out_of_date_when_lock_differs() {
        let dir = tmp_dir();
        let mgr = make_manager(dir.clone());
        std::fs::write(mgr.binary_path(), b"dummy").expect("write failed");
        mgr.write_version_lock("b0001").expect("write lock failed");
        let s = mgr.status().expect("status failed");
        assert_eq!(
            s,
            BinaryStatus::OutOfDate {
                installed: "b0001".to_string(),
                pinned: SUPPORTED_LLAMA_VERSION.to_string(),
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}