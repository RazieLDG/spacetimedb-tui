# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Changed

- Live updates are temporarily unavailable while safety controls are tightened. Use manual refresh for table data. Bounded scoped Live will return later; broad automatic subscriptions are currently unavailable.
- Spreadsheet editing now uses one-row spreadsheet Save: multiple changed cells in one row form one guided `WritePlan`; moving to another row prompts Save, Discard, or Stay.
- Guided update/delete require declared primary keys and matching generations, with no unsafe override.
- Raw SQL is a separately labeled expert path; Raw SQL does not receive the guided CRUD guarantee and is never automatically retried.
- Unknown mutation outcomes are not automatically retried after transport ownership begins. Refresh the affected scope before another guided attempt.

### Phase 1 validation gate

- `cargo fmt --all -- --check` passes.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.
- `cargo test --all-features --locked` passes and includes the original 127-test baseline plus Phase 1 safety tests.
- `cargo build --release --locked` passes.
- README, in-app help, and CHANGELOG all state that Live updates are temporarily unavailable, one-row spreadsheet Save is the supported spreadsheet contract, guided update/delete require declared primary keys and matching generations with no unsafe override, Raw SQL is a separately labeled expert path without the guided CRUD guarantee, and Unknown mutation outcomes are not automatically retried.
