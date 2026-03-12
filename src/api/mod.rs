//! SpacetimeDB API layer.
//!
//! Provides:
//! - [`client::SpacetimeClient`] — async HTTP client for all REST endpoints.
//! - [`types`] — strongly-typed request/response structs.
//! - [`ws`] — WebSocket client for real-time subscriptions and log streaming.

pub mod client;
pub mod types;
pub mod ws;

// Re-export the most commonly used items at the `api` level for ergonomic
// use elsewhere in the codebase.
pub use client::SpacetimeClient;
