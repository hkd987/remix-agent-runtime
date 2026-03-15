use crate::config::schema::ReasoningStagesConfig;

/// The current reasoning phase of the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningPhase {
    Planning,
    Execution,
    Verification,
}

/// Compute the thinking budget tokens for the current iteration based on reasoning stages.
///
/// Divides the agent run into three phases based on progress through max_iterations:
/// - Planning (early): higher budget for thorough analysis
/// - Execution (middle): lower budget for efficient implementation
/// - Verification (late): higher budget for careful review
pub fn compute_thinking_budget(
    config: &ReasoningStagesConfig,
    iteration: u32,
    max_iterations: u32,
) -> u32 {
    let phase = determine_phase(config, iteration, max_iterations);
    match phase {
        ReasoningPhase::Planning => config.planning_budget_tokens,
        ReasoningPhase::Execution => config.execution_budget_tokens,
        ReasoningPhase::Verification => config.verification_budget_tokens,
    }
}

/// Determine which reasoning phase the agent is in based on iteration progress.
///
/// If `planning_threshold >= verification_threshold`, the execution phase is
/// unreachable and the agent transitions directly from planning to verification.
pub fn determine_phase(
    config: &ReasoningStagesConfig,
    iteration: u32,
    max_iterations: u32,
) -> ReasoningPhase {
    if max_iterations == 0 {
        return ReasoningPhase::Planning;
    }
    let progress = iteration as f32 / max_iterations as f32;
    if progress < config.planning_threshold {
        ReasoningPhase::Planning
    } else if progress >= config.verification_threshold {
        ReasoningPhase::Verification
    } else {
        ReasoningPhase::Execution
    }
}

/// Validate that the reasoning stages config is consistent.
/// Returns an error message if planning_threshold >= verification_threshold.
pub fn validate_config(config: &ReasoningStagesConfig) -> Result<(), String> {
    if config.planning_threshold >= config.verification_threshold {
        return Err(format!(
            "reasoning_stages: planning_threshold ({}) must be less than verification_threshold ({})",
            config.planning_threshold, config.verification_threshold
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> ReasoningStagesConfig {
        ReasoningStagesConfig::default()
    }

    #[test]
    fn test_planning_phase_early_iterations() {
        let config = default_config();
        // 0.3 threshold → iterations 0-14 of 50 are planning
        assert_eq!(determine_phase(&config, 0, 50), ReasoningPhase::Planning);
        assert_eq!(determine_phase(&config, 10, 50), ReasoningPhase::Planning);
        assert_eq!(determine_phase(&config, 14, 50), ReasoningPhase::Planning);
    }

    #[test]
    fn test_execution_phase_middle_iterations() {
        let config = default_config();
        // 0.3-0.8 → iterations 15-39 of 50 are execution
        assert_eq!(determine_phase(&config, 15, 50), ReasoningPhase::Execution);
        assert_eq!(determine_phase(&config, 25, 50), ReasoningPhase::Execution);
        assert_eq!(determine_phase(&config, 39, 50), ReasoningPhase::Execution);
    }

    #[test]
    fn test_verification_phase_late_iterations() {
        let config = default_config();
        // 0.8 threshold → iterations 40-50 of 50 are verification
        assert_eq!(
            determine_phase(&config, 40, 50),
            ReasoningPhase::Verification
        );
        assert_eq!(
            determine_phase(&config, 50, 50),
            ReasoningPhase::Verification
        );
    }

    #[test]
    fn test_compute_budget_returns_correct_tokens() {
        let config = default_config();
        assert_eq!(compute_thinking_budget(&config, 0, 50), 10_000);
        assert_eq!(compute_thinking_budget(&config, 25, 50), 5_000);
        assert_eq!(compute_thinking_budget(&config, 45, 50), 10_000);
    }

    #[test]
    fn test_custom_thresholds() {
        let config = ReasoningStagesConfig {
            planning_budget_tokens: 20_000,
            execution_budget_tokens: 8_000,
            verification_budget_tokens: 15_000,
            planning_threshold: 0.2,
            verification_threshold: 0.9,
        };
        // 20% of 100 = 20
        assert_eq!(determine_phase(&config, 19, 100), ReasoningPhase::Planning);
        assert_eq!(determine_phase(&config, 20, 100), ReasoningPhase::Execution);
        // 90% of 100 = 90
        assert_eq!(determine_phase(&config, 89, 100), ReasoningPhase::Execution);
        assert_eq!(
            determine_phase(&config, 90, 100),
            ReasoningPhase::Verification
        );

        assert_eq!(compute_thinking_budget(&config, 10, 100), 20_000);
        assert_eq!(compute_thinking_budget(&config, 50, 100), 8_000);
        assert_eq!(compute_thinking_budget(&config, 95, 100), 15_000);
    }

    #[test]
    fn test_zero_max_iterations_defaults_to_planning() {
        let config = default_config();
        assert_eq!(determine_phase(&config, 0, 0), ReasoningPhase::Planning);
    }

    #[test]
    fn test_single_iteration_run() {
        let config = default_config();
        // iteration 1 of 1 = 100% → verification
        assert_eq!(determine_phase(&config, 1, 1), ReasoningPhase::Verification);
    }

    #[test]
    fn test_boundary_at_planning_threshold() {
        let config = default_config();
        // Exactly at 0.3: 15/50 = 0.3 → should be execution (>= threshold)
        assert_eq!(determine_phase(&config, 15, 50), ReasoningPhase::Execution);
        // Just below: 14/50 = 0.28 → planning
        assert_eq!(determine_phase(&config, 14, 50), ReasoningPhase::Planning);
    }

    #[test]
    fn test_boundary_at_verification_threshold() {
        let config = default_config();
        // Exactly at 0.8: 40/50 = 0.8 → verification
        assert_eq!(
            determine_phase(&config, 40, 50),
            ReasoningPhase::Verification
        );
        // Just below: 39/50 = 0.78 → execution
        assert_eq!(determine_phase(&config, 39, 50), ReasoningPhase::Execution);
    }

    #[test]
    fn test_config_defaults() {
        let config = default_config();
        assert_eq!(config.planning_budget_tokens, 10_000);
        assert_eq!(config.execution_budget_tokens, 5_000);
        assert_eq!(config.verification_budget_tokens, 10_000);
        assert!((config.planning_threshold - 0.3).abs() < f32::EPSILON);
        assert!((config.verification_threshold - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn test_validate_config_valid() {
        let config = default_config();
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_config_invalid_equal_thresholds() {
        let config = ReasoningStagesConfig {
            planning_threshold: 0.5,
            verification_threshold: 0.5,
            ..default_config()
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_invalid_inverted_thresholds() {
        let config = ReasoningStagesConfig {
            planning_threshold: 0.9,
            verification_threshold: 0.2,
            ..default_config()
        };
        let err = validate_config(&config);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("must be less than"));
    }

    #[test]
    fn test_inverted_thresholds_skips_execution_phase() {
        // When thresholds are inverted, execution phase is unreachable.
        // planning_threshold=0.8 means < 0.8 is Planning.
        // verification_threshold=0.3 means >= 0.3 is Verification.
        // But since Planning check comes first, anything < 0.8 stays Planning.
        let config = ReasoningStagesConfig {
            planning_threshold: 0.8,
            verification_threshold: 0.3,
            ..default_config()
        };
        // progress=0.1 < 0.8 → Planning
        assert_eq!(determine_phase(&config, 5, 50), ReasoningPhase::Planning);
        // progress=0.5 < 0.8 → Planning (planning check dominates)
        assert_eq!(determine_phase(&config, 25, 50), ReasoningPhase::Planning);
        // progress=0.9 >= 0.8, and >= 0.3 → Verification
        assert_eq!(
            determine_phase(&config, 45, 50),
            ReasoningPhase::Verification
        );
    }
}
