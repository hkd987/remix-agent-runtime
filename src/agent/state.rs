use std::time::Instant;

use crate::llm::types::{ContentBlock, Message, Role};
use crate::output::result::{AgentResult, AgentStatus, StepRecord};

pub struct AgentState {
    messages: Vec<Message>,
    steps: Vec<StepRecord>,
    iteration: u32,
    start_time: Instant,
}

impl AgentState {
    pub fn new(task: &str) -> Self {
        Self {
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: task.to_string(),
                }],
            }],
            steps: Vec::new(),
            iteration: 0,
            start_time: Instant::now(),
        }
    }

    pub fn add_assistant_message(&mut self, content: Vec<ContentBlock>) {
        self.messages.push(Message {
            role: Role::Assistant,
            content,
        });
    }

    pub fn add_tool_results(&mut self, results: Vec<ContentBlock>) {
        self.messages.push(Message {
            role: Role::User,
            content: results,
        });
    }

    pub fn record_step(&mut self, step: StepRecord) {
        self.steps.push(step);
    }

    pub fn increment_iteration(&mut self) {
        self.iteration += 1;
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn current_iteration(&self) -> u32 {
        self.iteration
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    pub fn into_result(self, status: AgentStatus, final_text: Option<String>) -> AgentResult {
        let total_iterations = self.iteration;
        let duration = self.elapsed_ms();
        match status {
            AgentStatus::Success => {
                let mut result =
                    AgentResult::success(final_text.unwrap_or_default(), self.steps, duration);
                result.total_iterations = total_iterations;
                result
            }
            AgentStatus::Error => {
                let mut result =
                    AgentResult::error(final_text.unwrap_or_default(), self.steps, duration);
                result.total_iterations = total_iterations;
                result
            }
            AgentStatus::Timeout => {
                let mut result = AgentResult::timeout(self.steps, duration);
                result.total_iterations = total_iterations;
                result
            }
            AgentStatus::MaxIterations => {
                AgentResult::max_iterations(self.steps, total_iterations, duration)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_new_creates_initial_user_message() {
        let state = AgentState::new("Do something");
        assert_eq!(state.messages().len(), 1);
        assert!(matches!(state.messages()[0].role, Role::User));
        match &state.messages()[0].content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Do something"),
            _ => panic!("Expected Text content block"),
        }
        assert_eq!(state.current_iteration(), 0);
    }

    #[test]
    fn test_add_assistant_message() {
        let mut state = AgentState::new("task");
        state.add_assistant_message(vec![ContentBlock::Text {
            text: "response".to_string(),
        }]);
        assert_eq!(state.messages().len(), 2);
        assert!(matches!(state.messages()[1].role, Role::Assistant));
        match &state.messages()[1].content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "response"),
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_add_tool_results() {
        let mut state = AgentState::new("task");
        state.add_tool_results(vec![ContentBlock::ToolResult {
            tool_use_id: "toolu_01".to_string(),
            content: "result data".to_string(),
            is_error: None,
        }]);
        assert_eq!(state.messages().len(), 2);
        assert!(matches!(state.messages()[1].role, Role::User));
        match &state.messages()[1].content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "toolu_01");
                assert_eq!(content, "result data");
                assert!(is_error.is_none());
            }
            _ => panic!("Expected ToolResult content block"),
        }
    }

    #[test]
    fn test_record_step() {
        let mut state = AgentState::new("task");
        let step = StepRecord {
            iteration: 1,
            tool: "navigate".to_string(),
            input: json!({"url": "https://example.com"}),
            output: json!({"success": true}),
            duration_ms: 500,
            is_error: None,
        };
        state.record_step(step.clone());
        let result = state.into_result(AgentStatus::Success, Some("done".to_string()));
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].tool, "navigate");
    }

    #[test]
    fn test_increment_iteration() {
        let mut state = AgentState::new("task");
        assert_eq!(state.current_iteration(), 0);
        state.increment_iteration();
        assert_eq!(state.current_iteration(), 1);
        state.increment_iteration();
        assert_eq!(state.current_iteration(), 2);
    }

    #[test]
    fn test_elapsed_ms() {
        let state = AgentState::new("task");
        // elapsed should be >= 0 (it just started)
        assert!(state.elapsed_ms() < 1000);
    }

    #[test]
    fn test_into_result_success() {
        let mut state = AgentState::new("task");
        state.increment_iteration();
        let result = state.into_result(AgentStatus::Success, Some("completed".to_string()));
        assert_eq!(result.status, AgentStatus::Success);
        assert_eq!(result.result, Some("completed".to_string()));
        assert_eq!(result.total_iterations, 1);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_into_result_error() {
        let mut state = AgentState::new("task");
        state.increment_iteration();
        let result = state.into_result(AgentStatus::Error, Some("something broke".to_string()));
        assert_eq!(result.status, AgentStatus::Error);
        assert_eq!(result.error, Some("something broke".to_string()));
        assert_eq!(result.total_iterations, 1);
        assert!(result.result.is_none());
    }

    #[test]
    fn test_into_result_timeout() {
        let mut state = AgentState::new("task");
        state.increment_iteration();
        state.increment_iteration();
        let result = state.into_result(AgentStatus::Timeout, None);
        assert_eq!(result.status, AgentStatus::Timeout);
        assert_eq!(result.total_iterations, 2);
        assert_eq!(result.error, Some("Agent timed out".to_string()));
    }

    #[test]
    fn test_into_result_max_iterations() {
        let mut state = AgentState::new("task");
        for _ in 0..5 {
            state.increment_iteration();
        }
        let result = state.into_result(AgentStatus::MaxIterations, None);
        assert_eq!(result.status, AgentStatus::MaxIterations);
        assert_eq!(result.total_iterations, 5);
        assert_eq!(result.error, Some("Max iterations (5) reached".to_string()));
    }

    #[test]
    fn test_into_result_success_default_text() {
        let state = AgentState::new("task");
        let result = state.into_result(AgentStatus::Success, None);
        assert_eq!(result.result, Some(String::new()));
    }
}
