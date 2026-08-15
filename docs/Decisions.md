# Decisions

All locked decisions for v1. Do not change without a new decision entry.

## Environment

| Tool | Version | Date Confirmed |
|------|---------|---------------|
| rustc | 1.97.1 | 2026-08-15 |
| cargo | 1.97.1 | 2026-08-15 |
| tauri-cli | 2.11.4 | 2026-08-15 |
| cargo-llvm-cov | 0.8.7 | 2026-08-15 |
| cargo-audit | 0.22.2 | 2026-08-15 |
| Node.js | 24.14.1 | 2026-08-15 |
| npm | 11.11.0 | 2026-08-15 |
| MSVC | 19.44.35228 | 2026-08-15 |
| WebView2 | 151.0.4129.86 | 2026-08-15 |

## Locked Decisions

| ID | Decision | Rationale |
|----|----------|-----------|
| D-01 | Tauri v2 (2.11.4) | v1 and v2 APIs differ materially. Pinning prevents mid-project migration cost. |
| D-02 | macOS Intel out of scope | llama-server is optimized for Apple Silicon. Intel Mac is rare in target demographic. Candidate for v1.1. |
| D-03 | Linux AMD/ROCm VRAM not detected | ROCm tooling is not consistently available. Falls back to safe conservative recommendation. Surfaced in UI. |
| D-04 | No in-app self-update | Users update by downloading a new release. Reduces v1 surface area. Future enhancement. |
| D-05 | 32B models gated to Advanced Hardware only | A 16 GB laptop cannot safely run 32B at any common quantization. Recommendation engine must never suggest 32B to systems with 24 GB RAM or less. |
| D-06 | catalog.toml is manually maintained | Update cadence is release-driven. Ownership: project maintainer. No discovery system in v1. |
| D-07 | SSE for streaming chat | Chosen over polling. Tauri v2 SSE complexity is a documented risk. Requires careful implementation. |
| D-08 | Phase 1C recovery dialogs are CLI-level only | UI dialogs for recovery are built in Phase 2A/2B. Phase 1C validates recovery logic through CLI flows only. |
| D-09 | 32B advanced-hardware testing is internal | Community tester pool targets mainstream hardware. Developer validates 32B internally. |
| D-10 | Critical path coverage >= 80%, measured by cargo-llvm-cov | Total line coverage is not the goal. Focus: catalog parsing, registry, resume logic, validation, recovery state. |

## How to Add a New Decision

1. Assign the next ID (D-11, D-12, ...).
2. State the decision clearly and concisely.
3. State the rationale.
4. Record the date.
5. Never remove or overwrite existing entries — append only.