#[cfg(test)]
pub mod conformance;
pub mod file_store;
pub mod traits;
pub mod types;

#[cfg(feature = "postgres")]
pub mod postgres_store;

pub use file_store::FileSessionStore;
pub use traits::{most_recent_session, SessionStorage};
pub use types::{SessionId, SessionMetadata, SessionSnapshot, SessionStatus};

#[cfg(feature = "postgres")]
pub use postgres_store::PostgresSessionStore;

// Backward compatibility alias
pub type SessionStore = FileSessionStore;
