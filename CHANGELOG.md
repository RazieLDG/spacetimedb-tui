# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Changed

- Live updates are temporarily unavailable while safety controls are tightened. Use manual refresh for table data. Bounded scoped Live will return later; broad automatic subscriptions are currently unavailable.
- Spreadsheet editing now uses one-row spreadsheet Save: multiple changed cells in one row form one guided `WritePlan`; moving to another row prompts Save, Discard, or Stay.
- Guided update/delete require declared primary keys and matching generations, with no unsafe override.
- Raw SQL is a separately labeled expert path; Raw SQL does not receive the guided CRUD guarantee and is never automatically retried.
- Unknown mutation outcomes are not automatically retried after transport ownership begins. Refresh the affected scope before another guided attempt.
