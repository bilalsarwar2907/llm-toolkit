# Threat Model

## Scope

This threat model covers the v1 local desktop application only.
No cloud services, no multi-user serving, no remote access.

## Assets

| Asset | Description |
|-------|-------------|
| Downloaded models | GGUF files stored in ~/.llm-toolkit/models/ |
| llama-server binary | Pinned official binary in ~/.llm-toolkit/bin/ |
| User chat history | In-memory only — not persisted in v1 |
| config.toml | Application settings |
| pending_resume.json | In-flight download metadata |
| Crash logs | Stored in ~/.llm-toolkit/logs/ |

## Threat Scenarios

### T-01: Corrupted or Tampered Model File
- **Risk:** Malicious or corrupted GGUF file causes undefined behavior in llama-server.
- **Mitigation:** SHA256 checksum verification (Tier 1). GGUF structural sanity check (Tier 4). Reject and delete on failure.

### T-02: Tampered llama-server Binary
- **Risk:** Replaced binary executes arbitrary code with user privileges.
- **Mitigation:** SHA256 verification on download. Version lock file. No auto-update — explicit user approval required.

### T-03: Man-in-the-Middle on Download
- **Risk:** Download traffic intercepted and binary or model replaced in transit.
- **Mitigation:** TLS enforced on all download connections. Certificate validation not bypassed.

### T-04: Disk Exhaustion
- **Risk:** Download fills disk, causing system instability.
- **Mitigation:** Preflight disk space check before every download. Required space = Content-Length + 10% + 500 MB buffer. Download blocked if check fails.

### T-05: Silent OOM During Inference
- **Risk:** llama-server consumes all available RAM, causing system freeze.
- **Mitigation:** Watchdog monitors RAM continuously. Warning at 85%. Graceful shutdown at 92%. Recommendation engine filters models exceeding safe RAM thresholds.

### T-06: Partial Download Used as Complete
- **Risk:** Incomplete model file passed to llama-server, causing crash or corrupt output.
- **Mitigation:** Validation hierarchy (Tier 1–4) run before any model is marked ready. Partial files stored with .part extension and never exposed as complete.

### T-07: Unsafe Model Recommended
- **Risk:** A model requiring more RAM than available is recommended to the user.
- **Mitigation:** Recommendation engine enforces min_ram_gb against detected RAM. 32B never recommended to systems with <= 24 GB RAM (D-05).

## Out of Scope for v1

- Network-level attacks beyond TLS (no remote API exposed)
- Multi-user privilege separation (single user, local only)
- Model output safety / content filtering
- Supply chain attacks on Rust dependencies