# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Added
- Typed command registry used by keyboard shortcuts, palette entries, contextual help, and future mouse actions.
- Reducer and effect boundaries with deterministic runtime ports for tests.
- Centralized navigation, resource, workbench, activity, and safety state under one `AppState` owner with no second mutable state owner.
- Normalized configuration provenance so explicit CLI values win over environment and lower sources, including `--no-tls` and custom theme strings.
- `restore_session` defaults to true for missing and partial user configuration.

### Changed
- Spreadsheet editing now uses one-row spreadsheet Save: multiple changed cells in one row form one guided `WritePlan`; moving to another row prompts Save, Discard, or Stay.
- Guided update/delete require declared primary keys and matching generations, with no unsafe override.
- Raw SQL is a separately labeled expert path; Raw SQL does not receive the guided CRUD guarantee and is never automatically retried.
- Unknown mutation outcomes are not automatically retried after transport ownership begins. Refresh the affected scope before another guided attempt.

### Validation
- `cargo fmt --all -- --check` passes.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.
- `cargo test --all-features --locked` passes.
- `cargo build --release --locked` passes.
- README, in-app help, and CHANGELOG all state that Live updates are scoped to the selected table only, one-row spreadsheet Save is the supported spreadsheet contract, guided update/delete require declared primary keys and matching generations with no unsafe override, Raw SQL is a separately labeled expert path without the guided CRUD guarantee, and Unknown mutation outcomes are not automatically retried.

## 0.1.2 — 2026-08-21

### Added
- Manual Release workflow builds Linux / macOS / Windows binaries on demand (`gh workflow run Release` or a `vX.Y.Z` tag), not on every commit.

### Changed
- Live updates are scoped to the selected table only; broad automatic all-table subscriptions are unavailable. Toggle with Ctrl+L. Use manual refresh if live is off. Large snapshots are split into `SubscribeSingle` chunks under the WebSocket size limit.
- Sidebar `j`/`k` walks the visible database/table tree so any nested table can be selected, not just the first one. Windows key-release events no longer skip rows.
- Table browse SQL quotes identifiers. The Live tab decodes current `v1.json.spacetimedb` `updates[]` / `TransactionUpdateLight` payloads.
