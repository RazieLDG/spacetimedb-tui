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

- Live updates are temporarily unavailable while safety controls are tightened. Use manual refresh for table data. Bounded scoped Live will return later; broad automatic subscriptions are currently unavailable.
- Spreadsheet editing now uses one-row spreadsheet Save: multiple changed cells in one row form one guided `WritePlan`; moving to another row prompts Save, Discard, or Stay.
- Guided update/delete require declared primary keys and matching generations, with no unsafe override.
- Raw SQL is a separately labeled expert path; Raw SQL does not receive the guided CRUD guarantee and is never automatically retried.
- Unknown mutation outcomes are not automatically retried after transport ownership begins. Refresh the affected scope before another guided attempt.

### Validation

- `cargo fmt --all -- --check` passes.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.
- `cargo test --all-features --locked` passes.
- `cargo build --release --locked` passes.
- README, in-app help, and CHANGELOG all state that Live updates are temporarily unavailable, one-row spreadsheet Save is the supported spreadsheet contract, guided update/delete require declared primary keys and matching generations with no unsafe override, Raw SQL is a separately labeled expert path without the guided CRUD guarantee, and Unknown mutation outcomes are not automatically retried.
