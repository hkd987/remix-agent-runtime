pub mod discovery;
pub mod executor;
pub mod types;

pub use discovery::{default_skill_dirs, discover_all_skills, inject_skills_into_system_prompt};
pub use executor::SkillAwareExecutor;
pub use types::{SkillEntry, SkillMetadata, SkillSet};
