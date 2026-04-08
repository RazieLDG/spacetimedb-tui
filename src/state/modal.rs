//! Modal dialog state — confirm prompts and multi-field forms.
//!
//! Lives in `state` rather than `ui` because it owns mutable input
//! state that survives across renders. The renderer in `ui::components`
//! reads it but does not own it.
//!
//! Two flavours:
//! - [`Modal::Confirm`] — yes/no prompt with a free-form prompt and
//!   a stashed [`ModalAction`] describing what to run on accept.
//! - [`Modal::Form`] — N labelled text fields plus an `intent`
//!   describing what to do with the filled-in values.
//!
//! Both variants funnel through [`ModalAction`] so the app event loop
//! can dispatch them with a single match.

use crate::ui::components::input::InputState;

/// One labelled input field inside a [`Modal::Form`].
#[derive(Debug, Clone)]
pub struct FormField {
    /// Display label shown next to the field, e.g. `"name (String)"`.
    pub label: String,
    /// The text the user is editing.
    pub input: InputState,
    /// Optional placeholder hint shown when the input is empty.
    pub placeholder: Option<String>,
}

impl FormField {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            input: InputState::new(),
            placeholder: None,
        }
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }
}

/// What to do when a modal is accepted (Enter / `y`).
///
/// We don't keep callbacks because that would require boxing
/// closures with a tonne of lifetimes; instead we describe the
/// intent and let the app event loop translate it back into the
/// right async call.
#[derive(Debug, Clone)]
pub enum ModalAction {
    /// Call the named reducer with the form's values, JSON-parsed in
    /// declaration order. Used by Module-tab Enter.
    CallReducer {
        reducer: String,
        /// SpacetimeDB type tag for each field, used to coerce the
        /// raw input into a sensible JSON value (`"42"` → `42` for a
        /// numeric param, `"hi"` → `"hi"` for a string param, etc.).
        param_types: Vec<String>,
    },
    /// Insert a row into a table by translating the form fields into
    /// an `INSERT INTO <table> (...) VALUES (...)` statement.
    InsertRow {
        table: String,
        column_types: Vec<String>,
    },
    /// Update a single row by primary key. The form's first field is
    /// always the WHERE clause value (the existing PK), the rest are
    /// the new column values.
    UpdateRow {
        table: String,
        pk_column: String,
        column_types: Vec<String>,
        /// Original PK value, captured at modal-open time so the
        /// WHERE clause survives any edits the user makes to the
        /// PK field.
        original_pk: String,
    },
    /// Delete a single row identified by `where_sql` (already
    /// quoted / formatted by the caller). Confirm dialog only.
    DeleteRow {
        table: String,
        where_sql: String,
    },
}

impl ModalAction {
    /// Short human-readable label used in the status bar after the
    /// op completes (e.g. `"call insert_user"`).
    pub fn op_label(&self) -> String {
        match self {
            ModalAction::CallReducer { reducer, .. } => format!("call {reducer}"),
            ModalAction::InsertRow { table, .. } => format!("insert into {table}"),
            ModalAction::UpdateRow { table, .. } => format!("update {table}"),
            ModalAction::DeleteRow { table, .. } => format!("delete from {table}"),
        }
    }
}

/// A live modal dialog. The app event loop checks
/// `AppState.modal.is_some()` at the top of `handle_key` and routes
/// every key into the modal until the user accepts or cancels.
#[derive(Debug, Clone)]
pub enum Modal {
    /// Yes/no prompt. Accept runs `action`, cancel discards it.
    Confirm {
        /// Headline rendered in bold at the top of the popup.
        title: String,
        /// Free-form body, usually two short lines describing the
        /// destructive op.
        prompt: String,
        action: ModalAction,
    },
    /// Multi-field form prompt. Accept runs `action` after building
    /// values from the form fields.
    Form {
        title: String,
        fields: Vec<FormField>,
        /// Index of the field that currently owns the cursor.
        focus: usize,
        action: ModalAction,
    },
}

impl Modal {
    /// Convenience constructor for a confirm dialog.
    pub fn confirm(
        title: impl Into<String>,
        prompt: impl Into<String>,
        action: ModalAction,
    ) -> Self {
        Modal::Confirm {
            title: title.into(),
            prompt: prompt.into(),
            action,
        }
    }

    /// Convenience constructor for a form dialog.
    pub fn form(
        title: impl Into<String>,
        fields: Vec<FormField>,
        action: ModalAction,
    ) -> Self {
        Modal::Form {
            title: title.into(),
            fields,
            focus: 0,
            action,
        }
    }

    /// The title shown in the popup border.
    pub fn title(&self) -> &str {
        match self {
            Modal::Confirm { title, .. } | Modal::Form { title, .. } => title,
        }
    }

    /// The action to dispatch when the user accepts.
    pub fn action(&self) -> &ModalAction {
        match self {
            Modal::Confirm { action, .. } | Modal::Form { action, .. } => action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_label_for_each_action() {
        assert_eq!(
            ModalAction::CallReducer {
                reducer: "insert_user".to_string(),
                param_types: vec![],
            }
            .op_label(),
            "call insert_user"
        );
        assert_eq!(
            ModalAction::InsertRow {
                table: "users".to_string(),
                column_types: vec![],
            }
            .op_label(),
            "insert into users"
        );
        assert_eq!(
            ModalAction::DeleteRow {
                table: "users".to_string(),
                where_sql: "id = 1".to_string(),
            }
            .op_label(),
            "delete from users"
        );
    }

    #[test]
    fn confirm_modal_exposes_action_and_title() {
        let m = Modal::confirm(
            "Delete row?",
            "users WHERE id = 1",
            ModalAction::DeleteRow {
                table: "users".to_string(),
                where_sql: "id = 1".to_string(),
            },
        );
        assert_eq!(m.title(), "Delete row?");
        assert!(matches!(m.action(), ModalAction::DeleteRow { .. }));
    }
}
