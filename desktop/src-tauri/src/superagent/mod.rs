//! UCode's native port of the DSH Superagent domain semantics.
//!
//! This module intentionally has no Tauri, model-provider, filesystem, or HTTP
//! dependencies. The desktop host and the Executor protocol are adapters around
//! this core, never alternative owners of its state.

mod client;
mod engine;
mod memory;
mod model;
mod protocol;
mod sqlite;
mod stage;

pub use client::{ExecutorClient, ExecutorClientError};
pub use engine::{Engine, EngineError, ReceiveResult};
pub use memory::MemoryRepositories;
pub use model::*;
pub use protocol::*;
pub use sqlite::{PersistTriggerResult, SqliteRepositories, SqliteRepositoryError};
pub use stage::{ticket_stage, TicketStage};
