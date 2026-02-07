use crate::error::AgentError;

/// Replaces `${VAR_NAME}` patterns in the input string with actual environment
/// variable values. Returns an error if a referenced variable is not set.
pub fn interpolate_env_vars(input: &str) -> Result<String, AgentError> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' {
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut var_name = String::new();
                let mut found_closing = false;

                for inner in chars.by_ref() {
                    if inner == '}' {
                        found_closing = true;
                        break;
                    }
                    var_name.push(inner);
                }

                if !found_closing {
                    return Err(AgentError::Config(format!(
                        "Unclosed variable reference: ${{{}",
                        var_name
                    )));
                }

                if var_name.is_empty() {
                    return Err(AgentError::Config(
                        "Empty variable name in ${{}}".to_string(),
                    ));
                }

                let value = std::env::var(&var_name).map_err(|_| {
                    AgentError::Config(format!("Environment variable '{}' is not set", var_name))
                })?;
                result.push_str(&value);
            } else {
                // Bare '$' not followed by '{', keep literal
                result.push(ch);
            }
        } else {
            result.push(ch);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_substitution() {
        std::env::set_var("TEST_ENV_INTERP_BASIC", "hello");
        let result = interpolate_env_vars("value=${TEST_ENV_INTERP_BASIC}").unwrap();
        assert_eq!(result, "value=hello");
        std::env::remove_var("TEST_ENV_INTERP_BASIC");
    }

    #[test]
    fn test_multiple_vars() {
        std::env::set_var("TEST_ENV_INTERP_A", "foo");
        std::env::set_var("TEST_ENV_INTERP_B", "bar");
        let result = interpolate_env_vars("${TEST_ENV_INTERP_A} and ${TEST_ENV_INTERP_B}").unwrap();
        assert_eq!(result, "foo and bar");
        std::env::remove_var("TEST_ENV_INTERP_A");
        std::env::remove_var("TEST_ENV_INTERP_B");
    }

    #[test]
    fn test_missing_var_error() {
        std::env::remove_var("DEFINITELY_NOT_SET_VAR_XYZ");
        let result = interpolate_env_vars("${DEFINITELY_NOT_SET_VAR_XYZ}");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("DEFINITELY_NOT_SET_VAR_XYZ"));
    }

    #[test]
    fn test_no_vars_passthrough() {
        let input = "plain text without any variables";
        let result = interpolate_env_vars(input).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_bare_dollar_passthrough() {
        let input = "costs $100 dollars";
        let result = interpolate_env_vars(input).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_unclosed_brace_error() {
        let result = interpolate_env_vars("${UNCLOSED");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unclosed"));
    }

    #[test]
    fn test_empty_var_name_error() {
        let result = interpolate_env_vars("${}");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Empty"));
    }

    #[test]
    fn test_adjacent_vars() {
        std::env::set_var("TEST_ENV_INTERP_X", "ab");
        std::env::set_var("TEST_ENV_INTERP_Y", "cd");
        let result = interpolate_env_vars("${TEST_ENV_INTERP_X}${TEST_ENV_INTERP_Y}").unwrap();
        assert_eq!(result, "abcd");
        std::env::remove_var("TEST_ENV_INTERP_X");
        std::env::remove_var("TEST_ENV_INTERP_Y");
    }

    #[test]
    fn test_var_at_start_and_end() {
        std::env::set_var("TEST_ENV_INTERP_SE", "val");
        let result = interpolate_env_vars("${TEST_ENV_INTERP_SE}").unwrap();
        assert_eq!(result, "val");
        std::env::remove_var("TEST_ENV_INTERP_SE");
    }

    #[test]
    fn test_url_with_vars() {
        std::env::set_var("TEST_ENV_INTERP_HOST", "api.example.com");
        std::env::set_var("TEST_ENV_INTERP_PORT", "8080");
        let result =
            interpolate_env_vars("https://${TEST_ENV_INTERP_HOST}:${TEST_ENV_INTERP_PORT}/v1")
                .unwrap();
        assert_eq!(result, "https://api.example.com:8080/v1");
        std::env::remove_var("TEST_ENV_INTERP_HOST");
        std::env::remove_var("TEST_ENV_INTERP_PORT");
    }
}
