# Testing Strategy

## Principles

- Backend tested before frontend is built.
- Coverage measured on critical path only, not total lines.
- Fault injection is mandatory, not optional.
- Integration tests validate real process behavior.
- No mocking of filesystem or process behavior unless unavoidable.

## Coverage Tool

cargo-llvm-cov (v0.8.7)

Run coverage:
```bash
cargo llvm-cov --workspace
```

Generate HTML report:
```bash
cargo llvm-cov --workspace --html
```

## Coverage Target

Critical path coverage >= 80% (D-10).

Critical path modules:
- catalog.rs — catalog parsing
- registry.rs — lock, unlock, cancel
- downloader.rs — resume logic, validation hierarchy, preflight check
- hardware.rs — hardware report struct population
- server_manager.rs — state machine transitions
- watchdog.rs — threshold detection

## Test Types

### Unit Tests
- Location: inline in each module (src-tauri/src/)
- Scope: pure logic, no I/O
- Must cover each validation tier independently
- Must cover recommendation engine filtering rules

### Integration Tests
- Location: tests/
- Scope: full module interaction with real filesystem and process calls
- Required scenarios:
  - Download TinyLlama → cancel → resume → validate
  - Server launch → health check → clean shutdown
  - Server crash → log written → state transitions to Crashed
  - Watchdog warning threshold triggered
  - Watchdog critical threshold → graceful shutdown

### Fault Injection Tests
- Location: tests/fault_injection/
- Required scenarios:

| Scenario | Expected Result |
|----------|----------------|
| Download interrupted mid-transfer | Resume succeeds on restart |
| Disk full during download | Preflight blocked or graceful error |
| Permission denied on model directory | Clear error, no silent failure |
| Network loss during download | Graceful pause, resume-ready state |
| llama-server process crash | Crash log written, state → Crashed |
| Corrupted GGUF file | Tier 4 rejects, user offered re-download |

### End-to-End (E2E) Tests
- Location: .github/workflows/e2e-smoke.yml
- Trigger: every push to main, every release tag
- Workflow:
  1. Acquire llama-server binary
  2. Download TinyLlama
  3. Launch llama-server
  4. POST /v1/chat/completions
  5. Assert valid JSON response
  6. Shutdown cleanly
  7. Assert exit code 0

## Phase Gates

| Phase | Gate |
|-------|------|
| 1A exit | Critical path coverage >= 80% on Phase 1A modules |
| 1B exit | Integration tests pass for server lifecycle and watchdog |
| 1C exit | All fault injection scenarios pass |
| 3 exit | No known release blockers across all target platforms |
| 5 exit | No critical or high severity issues from external testers |