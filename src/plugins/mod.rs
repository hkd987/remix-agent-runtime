pub mod components;
pub mod composite_executor;
pub mod discovery;
pub mod github;
pub mod hook_executor;
pub mod types;

pub use composite_executor::CompositeToolExecutor;
pub use discovery::discover_all_plugins;
pub use hook_executor::HookAwareExecutor;
pub use types::{PluginSet, ResolvedPlugin};
