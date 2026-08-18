/// catalog.rs
///
/// Parses the curated model catalog (catalog.toml) into typed Rust structs.
/// The catalog is embedded in the binary at compile time via `include_str!`.
///
/// Scope: static catalog only (D-06). No dynamic discovery.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ─── Public types ─────────────────────────────────────────────────────────────

/// Model category. Controls which models are surfaced to the user.
///
/// `Advanced` models (32B) are only shown on systems with sufficient RAM.
/// See D-05.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Recommended,
    Advanced,
}

/// A single model entry from `catalog.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// HuggingFace repository identifier (e.g. "bartowski/Llama-3.2-3B-Instruct-GGUF").
    pub repo_id: String,

    /// Exact GGUF filename within the repository.
    pub filename: String,

    /// Human-readable parameter count label (e.g. "7B").
    pub parameter_count: String,

    /// Quantization level (e.g. "Q4_K_M").
    pub quant: String,

    /// Approximate download size in GiB.
    pub approx_size_gb: f64,

    /// Whether this model is recommended for typical hardware or requires
    /// advanced hardware (32B, D-05).
    pub category: Category,

    /// Minimum system RAM in GiB required to run this model safely.
    /// The recommendation engine must never surface a model whose
    /// `min_ram_gb` exceeds 90% of detected RAM.
    pub min_ram_gb: u32,
}

/// The full model catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub models: Vec<Model>,
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Load the default catalog embedded in the binary at compile time.
///
/// # Errors
///
/// Returns an error if the embedded TOML is malformed — which indicates a
/// build-time mistake, not a runtime condition.
pub fn load_default() -> Result<Catalog> {
    // catalog.toml lives alongside catalog.rs in src/.
    let raw = include_str!("catalog.toml");
    parse(raw)
}

/// Parse a catalog from a raw TOML string.
///
/// Exposed as `pub` so that tests and future tooling can parse alternative
/// catalogs without touching the filesystem.
///
/// # Errors
///
/// Returns an error if `raw` is not valid TOML or does not match the
/// expected schema.
pub fn parse(raw: &str) -> Result<Catalog> {
    toml::from_str(raw).context("failed to parse catalog TOML")
}