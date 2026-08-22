//! Guided-write plumbing shared by form/confirm flows and background write
//! tasks: plan locks, staleness blocks, pending drafts, deferred refreshes,
//! spreadsheet lifecycle state, and postcondition fetch classification.

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaRequestContext {
    pub(crate) database: String,
    pub(crate) schema_generation: u64,
    pub(crate) database_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TableBrowseOrigin {
    ManualRefresh,
    Navigation,
    Automatic,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableBrowseRequestContext {
    pub(crate) database: String,
    pub(crate) schema: Option<String>,
    pub(crate) table: String,
    pub(crate) origin: TableBrowseOrigin,
    pub(crate) server_context_generation: u64,
    pub(crate) table_generation: u64,
    pub(crate) schema_generation: u64,
    pub(crate) database_generation: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingGuidedUpdateDraft {
    pub(crate) plan_id: WritePlanId,
    pub(crate) qualified_table: QualifiedTable,
    pub(crate) table_info: TableInfo,
    pub(crate) original_raw_row: Vec<serde_json::Value>,
    pub(crate) generations: Generations,
    pub(crate) original_form_values: Vec<GuidedFormValue>,
    pub(crate) requested_row: usize,
    pub(crate) requested_column: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingGuidedUpdateConfirmationForm {
    pub(crate) draft: PendingGuidedUpdateDraft,
    pub(crate) title: String,
    pub(crate) fields: Vec<crate::state::modal::FormField>,
    pub(crate) focus: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuidedWriteTarget {
    pub(crate) table: QualifiedTable,
    pub(crate) primary_key: CompletePrimaryKey,
    pub(crate) table_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeferredTableRefresh {
    pub(crate) table: QualifiedTable,
    pub(crate) server_context_generation: u64,
    pub(crate) database_generation: u64,
    pub(crate) schema_generation: u64,
    pub(crate) table_generation: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct GuidedWriteBlockKey {
    pub(crate) database: String,
    pub(crate) schema: Option<String>,
    pub(crate) table: String,
}

/// Why guided writes are currently refused for a table.
///
/// The two reasons are deliberately different products of the safety
/// contract. `RequiresManualRefresh` is the strong block demanded by
/// `Unknown`, `Conflict`, and `CriticalSafetyError`: the user must
/// personally reload before we accept another write. `StaleAfterVerifiedWrite`
/// is the weak barrier a *successful* write raises when it invalidated a
/// same-row owner: the cached rows are simply out of date, so any accepted
/// reload (including the automatic one the success itself queues) clears it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GuidedWriteUnavailable {
    RequiresManualRefresh,
    StaleAfterVerifiedWrite,
}

impl GuidedWriteBlockKey {
    pub(crate) fn from_table(table: &QualifiedTable) -> Self {
        Self {
            database: table.database.clone(),
            schema: table.schema.clone(),
            table: table.table.clone(),
        }
    }

    pub(crate) fn matches_browse_context(&self, context: &TableBrowseRequestContext) -> bool {
        self.database == context.database
            && self.schema == context.schema
            && self.table == context.table
    }
}

impl GuidedWriteTarget {
    pub(crate) fn from_plan(plan: &WritePlan) -> Self {
        Self {
            table: plan.table.clone(),
            primary_key: plan.original_primary_key.clone(),
            table_generation: plan.row_generation,
        }
    }

    pub(crate) fn same_row_identity(&self, other: &Self) -> bool {
        self.table == other.table && self.primary_key == other.primary_key
    }
}

#[derive(Debug, Default)]
pub(crate) struct GuidedWriteLocks {
    pub(crate) targets_by_plan_id: HashMap<WritePlanId, GuidedWriteTarget>,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingSpreadsheetEditRequest {
    pub(crate) target: EditRowTarget,
    pub(crate) column_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SpreadsheetSaveLifecycle {
    AwaitingConfirmation(WritePlanId),
    InFlight(WritePlanId),
    InFlightInvalidated(WritePlanId),
}

impl SpreadsheetSaveLifecycle {
    pub(crate) fn plan_id(self) -> WritePlanId {
        match self {
            Self::AwaitingConfirmation(plan_id)
            | Self::InFlight(plan_id)
            | Self::InFlightInvalidated(plan_id) => plan_id,
        }
    }

    pub(crate) fn is_in_flight(self) -> bool {
        matches!(self, Self::InFlight(_) | Self::InFlightInvalidated(_))
    }

    pub(crate) fn is_invalidated(self) -> bool {
        matches!(self, Self::InFlightInvalidated(_))
    }

    pub(crate) fn invalidate_if_in_flight(self) -> Self {
        match self {
            Self::InFlight(plan_id) | Self::InFlightInvalidated(plan_id) => {
                Self::InFlightInvalidated(plan_id)
            }
            Self::AwaitingConfirmation(plan_id) => Self::AwaitingConfirmation(plan_id),
        }
    }
}

impl GuidedWriteLocks {
    pub(crate) fn acquire(&mut self, plan: &WritePlan) -> bool {
        let target = GuidedWriteTarget::from_plan(plan);
        if self
            .targets_by_plan_id
            .values()
            .any(|locked| locked.same_row_identity(&target))
        {
            return false;
        }
        self.targets_by_plan_id.insert(plan.id, target);
        true
    }

    pub(crate) fn is_locked(&self, plan: &WritePlan) -> bool {
        let target = GuidedWriteTarget::from_plan(plan);
        self.targets_by_plan_id
            .values()
            .any(|locked| locked.same_row_identity(&target))
    }

    pub(crate) fn has_lock_for_table(&self, table: &QualifiedTable) -> bool {
        self.targets_by_plan_id
            .values()
            .any(|locked| locked.table == *table)
    }

    pub(crate) fn release(&mut self, plan_id: WritePlanId) -> Option<GuidedWriteTarget> {
        self.targets_by_plan_id.remove(&plan_id)
    }
}

impl super::App {
    /// Raise the strong "user must reload before we write again" block.
    ///
    /// The epoch is deliberately the generation observed *at the moment the
    /// outcome is applied*, not the generation the plan captured. A refresh
    /// that was already in flight when the outcome landed cannot prove
    /// anything about the post-outcome state, so it must not unblock.
    pub(crate) fn block_guided_writes_for_target(&mut self, target: &GuidedWriteTarget) {
        let key = GuidedWriteBlockKey::from_table(&target.table);
        let generation = self.table_generation;
        self.guided_write_blocks
            .entry(key)
            .and_modify(|blocked_generation| {
                *blocked_generation = (*blocked_generation).max(generation);
            })
            .or_insert(generation);
        self.deferred_guided_refresh = self
            .deferred_guided_refresh
            .take()
            .filter(|refresh| refresh.table != target.table);
    }

    /// Raise the weak "cached rows are behind a verified write" barrier.
    ///
    /// Unlike [`Self::block_guided_writes_for_target`] this is not part of the
    /// manual-refresh contract: it only records that the rows currently on
    /// screen predate a confirmed mutation. It is retired when those rows are
    /// replaced or dropped, and while it is up the app keeps driving the
    /// reload that replaces them, so the user is never left stranded.
    pub(crate) fn mark_guided_write_data_stale(&mut self, table: &QualifiedTable) {
        self.guided_write_stale_data
            .insert(GuidedWriteBlockKey::from_table(table));
    }

    pub(crate) fn guided_write_data_is_stale(&self, table: &QualifiedTable) -> bool {
        self.guided_write_stale_data
            .contains(&GuidedWriteBlockKey::from_table(table))
    }

    pub(crate) fn guided_write_blocked_generation(&self, table: &QualifiedTable) -> Option<u64> {
        self.guided_write_blocks
            .get(&GuidedWriteBlockKey::from_table(table))
            .copied()
    }

    pub(crate) fn guided_write_unavailable_reason(
        &self,
        table: &QualifiedTable,
    ) -> Option<GuidedWriteUnavailable> {
        if self.guided_write_blocked_generation(table).is_some() {
            return Some(GuidedWriteUnavailable::RequiresManualRefresh);
        }
        self.guided_write_data_is_stale(table)
            .then_some(GuidedWriteUnavailable::StaleAfterVerifiedWrite)
    }

    pub(crate) fn set_guided_write_unavailable_error(
        &mut self,
        table: &QualifiedTable,
        reason: GuidedWriteUnavailable,
    ) {
        let message = match reason {
            GuidedWriteUnavailable::RequiresManualRefresh => format!(
                "Guided writes for {} are blocked until manual refresh loads newer table data",
                table.table
            ),
            GuidedWriteUnavailable::StaleAfterVerifiedWrite => format!(
                "Guided writes for {} are paused until the reload after the last verified write lands",
                table.table
            ),
        };
        self.state.set_error(message);
    }

    /// Refuse a guided write when the target is blocked or stale, explaining
    /// which of the two it is. Returns `true` when the caller must stop.
    pub(crate) fn refuse_guided_write_if_unavailable(&mut self, table: &QualifiedTable) -> bool {
        match self.guided_write_unavailable_reason(table) {
            Some(reason) => {
                self.set_guided_write_unavailable_error(table, reason);
                true
            }
            None => false,
        }
    }

    pub(crate) fn unblock_guided_writes_after_accepted_browse(
        &mut self,
        context: &TableBrowseRequestContext,
    ) {
        // An accepted load replaces the rows the barrier was raised for.
        self.guided_write_stale_data
            .retain(|key| !key.matches_browse_context(context));
        if context.origin != TableBrowseOrigin::ManualRefresh {
            return;
        }
        self.guided_write_blocks.retain(|key, blocked_generation| {
            !(key.matches_browse_context(context) && context.table_generation > *blocked_generation)
        });
    }

    pub(crate) fn next_write_plan_id(&mut self) -> WritePlanId {
        let id = self.next_write_plan_id;
        self.next_write_plan_id = WritePlanId(self.next_write_plan_id.0.saturating_add(1));
        id
    }

    pub(crate) fn current_generations(&self) -> Generations {
        Generations {
            schema: self.schema_generation,
            row: self.table_generation,
            server_context: self.server_context_generation,
            database: self.database_generation,
        }
    }

    pub(crate) fn discard_pending_guided_writes(&mut self) {
        self.clear_spreadsheet_owned_modal();
        self.clear_discarded_guided_owned_modal();
        self.write_plans.clear();
        self.pending_update_draft = None;
        self.pending_update_confirmation_form = None;
        self.pending_spreadsheet_edit_request = None;
        if let Some(lifecycle) = self.spreadsheet_save_lifecycle {
            self.spreadsheet_save_lifecycle = if lifecycle.is_in_flight() {
                Some(lifecycle.invalidate_if_in_flight())
            } else {
                None
            };
        }
    }

    pub(crate) fn clear_discarded_guided_owned_modal(&mut self) {
        let should_clear = self
            .state
            .modal
            .as_ref()
            .is_some_and(|modal| match modal.action() {
                crate::state::modal::ModalAction::Safety(
                    crate::state::modal::SafetyModalAction::DirtyRowChoice {
                        requested_row,
                        requested_column,
                    },
                ) => self.pending_update_draft.as_ref().is_some_and(|draft| {
                    draft.requested_row == *requested_row
                        && draft.requested_column == *requested_column
                }),
                crate::state::modal::ModalAction::Safety(
                    crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
                ) => {
                    self.write_plans.contains_key(plan_id)
                        || self
                            .pending_update_confirmation_form
                            .as_ref()
                            .is_some_and(|snapshot| snapshot.draft.plan_id == *plan_id)
                }
                _ => false,
            });
        if should_clear {
            self.state.modal = None;
        }
    }

    pub(crate) fn clear_spreadsheet_edit_state(&mut self) {
        self.clear_spreadsheet_owned_modal();
        self.state.edit_mode = None;
        self.pending_spreadsheet_edit_request = None;
        self.spreadsheet_save_lifecycle = None;
    }

    pub(crate) fn clear_spreadsheet_owned_modal(&mut self) {
        let should_clear = self
            .state
            .modal
            .as_ref()
            .is_some_and(|modal| match modal.action() {
                crate::state::modal::ModalAction::SpreadsheetDirtyRowChoice { .. }
                | crate::state::modal::ModalAction::DiscardPendingEdits => true,
                crate::state::modal::ModalAction::Safety(
                    crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
                ) => self.spreadsheet_lifecycle_plan_id() == Some(*plan_id),
                _ => false,
            });
        if should_clear {
            self.state.modal = None;
        }
    }

    pub(crate) fn spreadsheet_lifecycle_plan_id(&self) -> Option<WritePlanId> {
        self.spreadsheet_save_lifecycle
            .map(SpreadsheetSaveLifecycle::plan_id)
    }

    pub(crate) fn spreadsheet_edits_are_frozen(&self) -> bool {
        self.spreadsheet_save_lifecycle
            .is_some_and(SpreadsheetSaveLifecycle::is_in_flight)
    }

    pub(crate) fn selected_table_matches(&self, table: &QualifiedTable) -> bool {
        self.state.selected_database() == Some(table.database.as_str())
            && self
                .state
                .selected_table()
                .is_some_and(|selected| selected.table_name == table.table)
    }

    pub(crate) fn target_for_guided_update_draft(
        draft: &PendingGuidedUpdateDraft,
    ) -> Option<GuidedWriteTarget> {
        let primary_key =
            complete_primary_key_from_table_row(&draft.table_info, &draft.original_raw_row).ok()?;
        Some(GuidedWriteTarget {
            table: draft.qualified_table.clone(),
            primary_key,
            table_generation: draft.generations.row,
        })
    }

    pub(crate) fn discard_same_target_pending_guided_update(
        &mut self,
        target: &GuidedWriteTarget,
    ) -> bool {
        let discard_draft = self
            .pending_update_draft
            .as_ref()
            .and_then(Self::target_for_guided_update_draft)
            .is_some_and(|draft_target| draft_target.same_row_identity(target));
        if discard_draft {
            self.pending_update_draft = None;
        }

        let discard_confirmation_snapshot = self
            .pending_update_confirmation_form
            .as_ref()
            .and_then(|snapshot| Self::target_for_guided_update_draft(&snapshot.draft))
            .is_some_and(|snapshot_target| snapshot_target.same_row_identity(target));
        if discard_confirmation_snapshot {
            self.pending_update_confirmation_form = None;
        }

        let removed_plan_ids: Vec<WritePlanId> = self
            .write_plans
            .iter()
            .filter_map(|(plan_id, plan)| {
                let plan_target = GuidedWriteTarget::from_plan(plan);
                plan_target.same_row_identity(target).then_some(*plan_id)
            })
            .collect();
        for plan_id in &removed_plan_ids {
            self.write_plans.remove(plan_id);
        }

        let should_clear_modal =
            self.state
                .modal
                .as_ref()
                .is_some_and(|modal| match modal.action() {
                    crate::state::modal::ModalAction::Safety(
                        crate::state::modal::SafetyModalAction::DirtyRowChoice { .. },
                    ) => discard_draft,
                    crate::state::modal::ModalAction::Safety(
                        crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
                    ) => removed_plan_ids.contains(plan_id),
                    _ => false,
                });
        if should_clear_modal {
            self.state.modal = None;
        }
        // A spreadsheet save that was still awaiting confirmation loses its
        // plan and modal here; leaving the marker behind would freeze every
        // later spreadsheet interaction on a plan that no longer exists.
        if self.spreadsheet_save_lifecycle.is_some_and(|lifecycle| {
            !lifecycle.is_in_flight() && removed_plan_ids.contains(&lifecycle.plan_id())
        }) {
            self.spreadsheet_save_lifecycle = None;
        }
        discard_draft || discard_confirmation_snapshot || !removed_plan_ids.is_empty()
    }

    pub(crate) fn restore_guided_update_plan_after_local_rejection(&mut self, plan: WritePlan) {
        if self
            .pending_update_confirmation_form
            .as_ref()
            .is_some_and(|snapshot| snapshot.draft.plan_id == plan.id)
        {
            let plan_id = plan.id;
            self.write_plans.insert(plan_id, plan);
            self.restore_pending_update_confirmation_form(plan_id);
        }
    }

    pub(crate) fn guided_current_table_interaction_is_active(&self) -> bool {
        if let (Some(database), Some(table)) = (
            self.state.selected_database(),
            self.state
                .selected_table()
                .map(|table| table.table_name.as_str()),
        ) {
            let selected_table = QualifiedTable {
                database: database.to_string(),
                schema: None,
                table: table.to_string(),
            };
            if self.guided_write_locks.has_lock_for_table(&selected_table) {
                return true;
            }
        }
        if self
            .pending_update_draft
            .as_ref()
            .is_some_and(|draft| self.selected_table_matches(&draft.qualified_table))
        {
            return true;
        }
        if self.pending_spreadsheet_edit_request.is_some()
            || self.spreadsheet_save_lifecycle.is_some()
            || self.state.edit_mode.is_some()
        {
            return true;
        }
        if let Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) = self
            .state
            .modal
            .as_ref()
            .map(crate::state::modal::Modal::action)
        {
            return self
                .write_plans
                .get(plan_id)
                .is_some_and(|plan| self.selected_table_matches(&plan.table));
        }
        false
    }

    pub(crate) fn read_owner_is_active(&self) -> bool {
        self.state.query_loading || self.state.schema_loading
    }

    pub(crate) fn deferred_table_refresh_for(&self, table: QualifiedTable) -> DeferredTableRefresh {
        DeferredTableRefresh {
            table,
            server_context_generation: self.server_context_generation,
            database_generation: self.database_generation,
            schema_generation: self.schema_generation,
            table_generation: self.table_generation,
        }
    }

    pub(crate) fn deferred_refresh_matches_current(&self, refresh: &DeferredTableRefresh) -> bool {
        self.selected_table_matches(&refresh.table)
            && self.server_context_generation == refresh.server_context_generation
            && self.database_generation == refresh.database_generation
            && self.schema_generation == refresh.schema_generation
            && self.table_generation == refresh.table_generation
    }

    pub(crate) async fn refresh_or_defer_guided_table(&mut self, table: QualifiedTable) {
        // Only the manual-refresh block suppresses an automatic reload; the
        // weak staleness barrier exists precisely so this reload can clear it.
        if self.guided_write_blocked_generation(&table).is_some() {
            self.deferred_guided_refresh = self
                .deferred_guided_refresh
                .take()
                .filter(|refresh| refresh.table != table);
            return;
        }
        if self.guided_current_table_interaction_is_active() || self.read_owner_is_active() {
            self.deferred_guided_refresh = Some(self.deferred_table_refresh_for(table));
            return;
        }
        if self.selected_table_matches(&table) {
            self.deferred_guided_refresh = None;
            self.load_table_data(TableBrowseOrigin::Automatic).await;
        }
    }

    /// The table whose rows are behind a verified write and which we are still
    /// responsible for reloading.
    ///
    /// The weak staleness barrier refuses guided writes, so something must
    /// always be driving the load that clears it. A queued deferred refresh can
    /// be dropped by an unrelated invalidation, so the barrier on the selected
    /// table is itself a standing reload request and outlives that queue entry.
    pub(crate) fn selected_table_awaits_stale_reload(&self) -> bool {
        let Some(database) = self.state.selected_database() else {
            return false;
        };
        let Some(table) = self.state.selected_table() else {
            return false;
        };
        let selected = QualifiedTable {
            database: database.to_string(),
            schema: None,
            table: table.table_name.clone(),
        };
        self.guided_write_data_is_stale(&selected)
            && self.guided_write_blocked_generation(&selected).is_none()
    }

    pub(crate) async fn flush_deferred_guided_refresh_if_unowned(&mut self) {
        if self
            .deferred_guided_refresh
            .as_ref()
            .is_some_and(|refresh| !self.deferred_refresh_matches_current(refresh))
        {
            self.deferred_guided_refresh = None;
        }
        if self.guided_current_table_interaction_is_active() || self.read_owner_is_active() {
            return;
        }
        let refresh = self.deferred_guided_refresh.take();
        if let Some(refresh) = refresh {
            if self
                .guided_write_blocked_generation(&refresh.table)
                .is_some()
            {
                return;
            }
            if self.deferred_refresh_matches_current(&refresh) {
                self.load_table_data(TableBrowseOrigin::Automatic).await;
                return;
            }
        }
        // The queued reload was dropped or never matched, but the barrier is
        // still refusing writes on the table in front of the user, so drive the
        // reload that retires it.
        if self.selected_table_awaits_stale_reload() {
            self.load_table_data(TableBrowseOrigin::Automatic).await;
        }
    }

    pub(crate) fn block_if_spreadsheet_edits_frozen(&mut self, action: &str) -> bool {
        if self.spreadsheet_edits_are_frozen() {
            self.state.set_notification(format!(
                "Spreadsheet save is in flight; {action} after it finishes"
            ));
            true
        } else {
            false
        }
    }
}

pub(crate) enum PostconditionFetchError {
    Critical(String),
    Unknown(String),
}

impl PostconditionFetchError {
    pub(crate) fn critical(reason: impl Into<String>) -> Self {
        Self::Critical(reason.into())
    }

    pub(crate) fn unknown(reason: impl Into<String>) -> Self {
        Self::Unknown(reason.into())
    }
}

pub(crate) fn verification_report_from_postcondition_fetch_error(
    error: PostconditionFetchError,
) -> WriteVerificationReport {
    let outcome = match error {
        PostconditionFetchError::Critical(reason) => {
            MutationOutcome::CriticalSafetyError { reason }
        }
        PostconditionFetchError::Unknown(reason) => MutationOutcome::Unknown { reason },
    };
    WriteVerificationReport { outcome }
}

pub(crate) async fn build_postcondition_evidence(
    client: &SpacetimeClient,
    db: &str,
    table: &TableInfo,
    postcondition_queries: PostconditionQueries,
) -> Result<PostconditionEvidence, PostconditionFetchError> {
    match postcondition_queries {
        PostconditionQueries::Delete {
            original_key_lookup,
        } => {
            let rows = run_lookup(client, db, table, &original_key_lookup).await?;
            match rows {
                LookupRows::Zero => Ok(PostconditionEvidence::Delete {
                    original_tuple_present: false,
                }),
                LookupRows::One(_) => Ok(PostconditionEvidence::Delete {
                    original_tuple_present: true,
                }),
                LookupRows::Multiple(count) => Err(PostconditionFetchError::critical(format!(
                    "complete-key delete verification returned {count} rows; expected at most 1"
                ))),
            }
        }
        PostconditionQueries::Update {
            original_key_lookup,
            new_key_lookup: None,
        } => {
            let rows = run_lookup(client, db, table, &original_key_lookup).await?;
            let row_by_original_key = match rows {
                LookupRows::Zero => None,
                LookupRows::One(row) => Some(row),
                LookupRows::Multiple(count) => {
                    return Err(PostconditionFetchError::critical(format!(
                        "complete-key update verification returned {count} rows; expected at most 1"
                    )));
                }
            };
            Ok(PostconditionEvidence::Update(Box::new(
                PostconditionUpdate {
                    table: table.clone(),
                    row_by_original_key,
                    row_by_new_key: None,
                    old_key_still_present: None,
                },
            )))
        }
        PostconditionQueries::Update {
            original_key_lookup,
            new_key_lookup: Some(new_key_lookup),
        } => {
            let old_rows = run_lookup(client, db, table, &original_key_lookup).await?;
            let old_key_still_present = match old_rows {
                LookupRows::Zero => false,
                LookupRows::One(row) => {
                    return Ok(PostconditionEvidence::Update(Box::new(
                        PostconditionUpdate {
                            table: table.clone(),
                            row_by_original_key: Some(row),
                            row_by_new_key: None,
                            old_key_still_present: Some(true),
                        },
                    )));
                }
                LookupRows::Multiple(count) => {
                    return Err(PostconditionFetchError::critical(format!(
                        "complete-key old tuple verification returned {count} rows; expected at most 1"
                    )));
                }
            };
            let new_rows = run_lookup(client, db, table, &new_key_lookup).await?;
            let row_by_new_key = match new_rows {
                LookupRows::Zero => None,
                LookupRows::One(row) => Some(row),
                LookupRows::Multiple(count) => {
                    return Err(PostconditionFetchError::critical(format!(
                        "complete-key new tuple verification returned {count} rows; expected at most 1"
                    )));
                }
            };
            Ok(PostconditionEvidence::Update(Box::new(
                PostconditionUpdate {
                    table: table.clone(),
                    row_by_original_key: None,
                    row_by_new_key,
                    old_key_still_present: Some(old_key_still_present),
                },
            )))
        }
    }
}

pub(crate) async fn run_lookup(
    client: &SpacetimeClient,
    db: &str,
    table: &TableInfo,
    sql: &str,
) -> Result<LookupRows, PostconditionFetchError> {
    let result = tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.query_sql(db, sql))
        .await
        .map_err(|_| PostconditionFetchError::unknown("postcondition verification timed out"))?
        .map_err(|error| {
            PostconditionFetchError::unknown(format!(
                "postcondition verification failed: {error:#}"
            ))
        })?;
    normalize_lookup_result(table, &result)
        .map_err(|error| PostconditionFetchError::unknown(format!("{error:?}")))
}
