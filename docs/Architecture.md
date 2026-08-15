# Architecture

## Overview

LLM Toolkit is a Tauri v2 desktop application with a Rust backend and a Vanilla TypeScript frontend.

## Layer Responsibilities

### Backend (Rust / src-tauri)
- Hardware detection
- Model catalog management
- Binary (llama-server) acquisition and management
- Download management with resume and recovery
- Server process lifecycle management
- Watchdog monitoring
- All business logic

### Frontend (TypeScript / src)
- Display only — no business logic
- Receives state via Tauri events
- Sends user actions via Tauri commands
- No direct filesystem or process access

## Key Modules

| Module | Path | Responsibility |
|--------|------|----------------|
| Hardware | src-tauri/src/hardware.rs | CPU, RAM, OS, disk, VRAM detection |
| Recommendation | src-tauri/src/recommendation.rs | Safe model recommendation logic |
| Binary Manager | src-tauri/src/binary/manager.rs | llama-server download and version management |
| Catalog | src-tauri/src/catalog.rs | Static model catalog parsing |
| Registry | src-tauri/src/registry.rs | Active download tracking and locking |
| Downloader | src-tauri/src/downloader.rs | Hardened download with resume and validation |
| Server Manager | src-tauri/src/server_manager.rs | llama-server process state machine |
| Watchdog | src-tauri/src/watchdog.rs | RAM/VRAM threshold monitoring |

## Data Flow