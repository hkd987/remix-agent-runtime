use crate::config::schema::ReminderConfig;

/// Trigger condition for a system reminder.
#[derive(Debug, Clone, PartialEq)]
pub enum ReminderTrigger {
    /// Fire every N iterations.
    EveryNIterations(u32),
    /// Fire before specific tools are executed.
    BeforeTools(Vec<String>),
}

/// A system reminder with its content and trigger condition.
#[derive(Debug, Clone)]
pub struct SystemReminder {
    pub content: String,
    pub trigger: ReminderTrigger,
}

/// Scheduler that determines which reminders should fire at a given point.
pub struct ReminderScheduler {
    reminders: Vec<SystemReminder>,
}

impl ReminderScheduler {
    /// Build a scheduler from config entries.
    ///
    /// A single `ReminderConfig` may produce up to two `SystemReminder`s if
    /// both `every_n_iterations` and `before_tools` are specified.
    pub fn from_configs(configs: &[ReminderConfig]) -> Self {
        let mut reminders = Vec::new();
        for config in configs {
            if let Some(n) = config.every_n_iterations {
                reminders.push(SystemReminder {
                    content: config.content.clone(),
                    trigger: ReminderTrigger::EveryNIterations(n),
                });
            }
            if let Some(tools) = &config.before_tools {
                reminders.push(SystemReminder {
                    content: config.content.clone(),
                    trigger: ReminderTrigger::BeforeTools(tools.clone()),
                });
            }
        }
        Self { reminders }
    }

    /// Check which reminders should fire given the current state.
    ///
    /// `iteration` is the current loop iteration (1-based).
    /// `pending_tools` are tool names about to be executed.
    ///
    /// Returns reminder content strings (with `[SYSTEM_REMINDER]` prefix)
    /// that should be injected into the conversation.
    pub fn check(&self, iteration: u32, pending_tools: &[String]) -> Vec<String> {
        let mut triggered = Vec::new();
        for reminder in &self.reminders {
            match &reminder.trigger {
                ReminderTrigger::EveryNIterations(n) => {
                    if *n > 0 && iteration.is_multiple_of(*n) {
                        triggered.push(format!("[SYSTEM_REMINDER] {}", reminder.content));
                    }
                }
                ReminderTrigger::BeforeTools(tools) => {
                    if pending_tools.iter().any(|pt| tools.contains(pt)) {
                        triggered.push(format!("[SYSTEM_REMINDER] {}", reminder.content));
                    }
                }
            }
        }
        triggered
    }

    /// Returns true if the scheduler has no reminders configured.
    pub fn is_empty(&self) -> bool {
        self.reminders.is_empty()
    }

    /// Returns the number of reminders in the scheduler.
    pub fn len(&self) -> usize {
        self.reminders.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_configs_empty() {
        let scheduler = ReminderScheduler::from_configs(&[]);
        assert!(scheduler.is_empty());
        assert_eq!(scheduler.len(), 0);
    }

    #[test]
    fn test_from_configs_every_n_iterations() {
        let configs = vec![ReminderConfig {
            content: "Check progress".to_string(),
            every_n_iterations: Some(5),
            before_tools: None,
        }];
        let scheduler = ReminderScheduler::from_configs(&configs);
        assert_eq!(scheduler.len(), 1);
        assert_eq!(scheduler.reminders[0].content, "Check progress");
        assert_eq!(
            scheduler.reminders[0].trigger,
            ReminderTrigger::EveryNIterations(5)
        );
    }

    #[test]
    fn test_from_configs_before_tools() {
        let configs = vec![ReminderConfig {
            content: "Verify before executing".to_string(),
            every_n_iterations: None,
            before_tools: Some(vec!["navigate".to_string(), "click".to_string()]),
        }];
        let scheduler = ReminderScheduler::from_configs(&configs);
        assert_eq!(scheduler.len(), 1);
        assert_eq!(
            scheduler.reminders[0].trigger,
            ReminderTrigger::BeforeTools(vec!["navigate".to_string(), "click".to_string()])
        );
    }

    #[test]
    fn test_from_configs_both_triggers() {
        let configs = vec![ReminderConfig {
            content: "Dual trigger".to_string(),
            every_n_iterations: Some(3),
            before_tools: Some(vec!["execute".to_string()]),
        }];
        let scheduler = ReminderScheduler::from_configs(&configs);
        assert_eq!(scheduler.len(), 2);
        assert_eq!(
            scheduler.reminders[0].trigger,
            ReminderTrigger::EveryNIterations(3)
        );
        assert_eq!(
            scheduler.reminders[1].trigger,
            ReminderTrigger::BeforeTools(vec!["execute".to_string()])
        );
    }

    #[test]
    fn test_check_every_n_fires_on_multiples() {
        let configs = vec![ReminderConfig {
            content: "Periodic check".to_string(),
            every_n_iterations: Some(5),
            before_tools: None,
        }];
        let scheduler = ReminderScheduler::from_configs(&configs);
        let no_tools: Vec<String> = vec![];

        assert_eq!(scheduler.check(5, &no_tools).len(), 1);
        assert_eq!(scheduler.check(10, &no_tools).len(), 1);
        assert_eq!(scheduler.check(15, &no_tools).len(), 1);
    }

    #[test]
    fn test_check_every_n_does_not_fire_off_cycle() {
        let configs = vec![ReminderConfig {
            content: "Periodic check".to_string(),
            every_n_iterations: Some(5),
            before_tools: None,
        }];
        let scheduler = ReminderScheduler::from_configs(&configs);
        let no_tools: Vec<String> = vec![];

        assert!(scheduler.check(3, &no_tools).is_empty());
        assert!(scheduler.check(7, &no_tools).is_empty());
        assert!(scheduler.check(1, &no_tools).is_empty());
    }

    #[test]
    fn test_check_before_tools_fires_on_match() {
        let configs = vec![ReminderConfig {
            content: "Tool warning".to_string(),
            every_n_iterations: None,
            before_tools: Some(vec!["navigate".to_string(), "click".to_string()]),
        }];
        let scheduler = ReminderScheduler::from_configs(&configs);
        let pending = vec!["navigate".to_string()];

        let results = scheduler.check(1, &pending);
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("Tool warning"));
    }

    #[test]
    fn test_check_before_tools_no_match() {
        let configs = vec![ReminderConfig {
            content: "Tool warning".to_string(),
            every_n_iterations: None,
            before_tools: Some(vec!["navigate".to_string()]),
        }];
        let scheduler = ReminderScheduler::from_configs(&configs);
        let pending = vec!["screenshot".to_string()];

        assert!(scheduler.check(1, &pending).is_empty());
    }

    #[test]
    fn test_check_returns_multiple_reminders() {
        let configs = vec![
            ReminderConfig {
                content: "First reminder".to_string(),
                every_n_iterations: Some(2),
                before_tools: None,
            },
            ReminderConfig {
                content: "Second reminder".to_string(),
                every_n_iterations: Some(3),
                before_tools: None,
            },
        ];
        let scheduler = ReminderScheduler::from_configs(&configs);
        let no_tools: Vec<String> = vec![];

        // iteration 6 is divisible by both 2 and 3
        let results = scheduler.check(6, &no_tools);
        assert_eq!(results.len(), 2);
        assert!(results[0].contains("First reminder"));
        assert!(results[1].contains("Second reminder"));
    }

    #[test]
    fn test_check_formats_with_prefix() {
        let configs = vec![ReminderConfig {
            content: "Stay focused".to_string(),
            every_n_iterations: Some(1),
            before_tools: None,
        }];
        let scheduler = ReminderScheduler::from_configs(&configs);
        let no_tools: Vec<String> = vec![];

        let results = scheduler.check(1, &no_tools);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "[SYSTEM_REMINDER] Stay focused");
    }

    #[test]
    fn test_is_empty() {
        let empty = ReminderScheduler::from_configs(&[]);
        assert!(empty.is_empty());

        let non_empty = ReminderScheduler::from_configs(&[ReminderConfig {
            content: "Exists".to_string(),
            every_n_iterations: Some(1),
            before_tools: None,
        }]);
        assert!(!non_empty.is_empty());
    }

    #[test]
    fn test_every_n_zero_never_fires() {
        let configs = vec![ReminderConfig {
            content: "Should never fire".to_string(),
            every_n_iterations: Some(0),
            before_tools: None,
        }];
        let scheduler = ReminderScheduler::from_configs(&configs);
        let no_tools: Vec<String> = vec![];

        // Zero should never trigger (avoids divide-by-zero and semantically means "never")
        for i in 0..20 {
            assert!(scheduler.check(i, &no_tools).is_empty());
        }
    }

    #[test]
    fn test_reminder_config_serde_roundtrip() {
        let config = ReminderConfig {
            content: "Remember to validate".to_string(),
            every_n_iterations: Some(3),
            before_tools: Some(vec!["execute".to_string(), "navigate".to_string()]),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let deserialized: ReminderConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deserialized, config);

        // Also verify JSON roundtrip
        let json = serde_json::to_string(&config).unwrap();
        let from_json: ReminderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(from_json, config);
    }
}
