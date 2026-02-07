use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialType {
    UsernamePassword,
    ApiKey,
    Token,
    Cookie,
    Custom,
}

fn default_credential_type() -> CredentialType {
    CredentialType::UsernamePassword
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Credential {
    pub name: String,
    #[serde(default = "default_credential_type")]
    pub credential_type: CredentialType,
    #[serde(default)]
    pub fields: HashMap<String, String>,
    pub url_pattern: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_fields: HashMap<&String, &str> =
            self.fields.keys().map(|k| (k, "***")).collect();
        f.debug_struct("Credential")
            .field("name", &self.name)
            .field("credential_type", &self.credential_type)
            .field("fields", &redacted_fields)
            .field("url_pattern", &self.url_pattern)
            .field("metadata", &self.metadata)
            .field("username", &self.username.as_ref().map(|_| "***"))
            .field("password", &self.password.as_ref().map(|_| "***"))
            .finish()
    }
}

/// Normalizes credentials by moving flat username/password fields into the
/// `fields` map when present. This allows YAML configs to use the convenient
/// flat syntax while the rest of the system always reads from `fields`.
pub fn load_credentials_from_config(creds: &[Credential]) -> Vec<Credential> {
    creds
        .iter()
        .map(|cred| {
            let mut normalized = cred.clone();
            if let Some(ref username) = cred.username {
                normalized
                    .fields
                    .entry("username".to_string())
                    .or_insert_with(|| username.clone());
            }
            if let Some(ref password) = cred.password {
                normalized
                    .fields
                    .entry("password".to_string())
                    .or_insert_with(|| password.clone());
            }
            normalized.username = None;
            normalized.password = None;
            normalized
        })
        .collect()
}

/// Formats credentials for inclusion in an LLM system prompt.
/// Returns `None` if no credentials are provided.
pub fn inject_credentials_into_system_prompt(creds: &[Credential]) -> Option<String> {
    if creds.is_empty() {
        return None;
    }

    let mut lines = vec!["Available credentials:".to_string()];

    for cred in creds {
        let type_str = match cred.credential_type {
            CredentialType::UsernamePassword => "username_password",
            CredentialType::ApiKey => "api_key",
            CredentialType::Token => "token",
            CredentialType::Cookie => "cookie",
            CredentialType::Custom => "custom",
        };

        let pattern = cred.url_pattern.as_deref().unwrap_or("*");

        lines.push(format!("- {} ({}) for {}:", cred.name, type_str, pattern));

        // Emit fields sorted by key for deterministic output
        let mut sorted_fields: Vec<_> = cred.fields.iter().collect();
        sorted_fields.sort_by_key(|(k, _)| (*k).clone());

        for (key, value) in sorted_fields {
            lines.push(format!("  {}: {}", key, value));
        }
    }

    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_credential(
        name: &str,
        cred_type: CredentialType,
        fields: Vec<(&str, &str)>,
        url_pattern: Option<&str>,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Credential {
        Credential {
            name: name.to_string(),
            credential_type: cred_type,
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            url_pattern: url_pattern.map(|s| s.to_string()),
            metadata: HashMap::new(),
            username: username.map(|s| s.to_string()),
            password: password.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_debug_redacts_fields() {
        let cred = make_credential(
            "test",
            CredentialType::UsernamePassword,
            vec![("password", "s3cret_value")],
            None,
            Some("alice_jones"),
            Some("hunter2"),
        );
        let debug_str = format!("{:?}", cred);
        // Actual secret values must not appear in debug output
        assert!(!debug_str.contains("s3cret_value"));
        assert!(!debug_str.contains("alice_jones"));
        assert!(!debug_str.contains("hunter2"));
        // Redaction markers must appear
        assert!(debug_str.contains("***"));
        // Struct field name "password" is expected (it's a key, not a value)
        assert!(debug_str.contains("password"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_debug_redacts_no_fields() {
        let cred = make_credential("empty", CredentialType::Custom, vec![], None, None, None);
        let debug_str = format!("{:?}", cred);
        assert!(debug_str.contains("empty"));
    }

    #[test]
    fn test_load_credentials_normalizes_username_password() {
        let creds = vec![make_credential(
            "login",
            CredentialType::UsernamePassword,
            vec![],
            Some("*.example.com"),
            Some("user@example.com"),
            Some("s3cret"),
        )];
        let normalized = load_credentials_from_config(&creds);
        assert_eq!(normalized.len(), 1);
        let cred = &normalized[0];
        assert_eq!(cred.fields.get("username").unwrap(), "user@example.com");
        assert_eq!(cred.fields.get("password").unwrap(), "s3cret");
        assert!(cred.username.is_none());
        assert!(cred.password.is_none());
    }

    #[test]
    fn test_load_credentials_does_not_overwrite_existing_fields() {
        let creds = vec![make_credential(
            "login",
            CredentialType::UsernamePassword,
            vec![("username", "existing_user")],
            None,
            Some("new_user"),
            None,
        )];
        let normalized = load_credentials_from_config(&creds);
        assert_eq!(
            normalized[0].fields.get("username").unwrap(),
            "existing_user"
        );
    }

    #[test]
    fn test_load_credentials_empty() {
        let normalized = load_credentials_from_config(&[]);
        assert!(normalized.is_empty());
    }

    #[test]
    fn test_inject_credentials_none_when_empty() {
        assert!(inject_credentials_into_system_prompt(&[]).is_none());
    }

    #[test]
    fn test_inject_credentials_formats_correctly() {
        let creds = vec![make_credential(
            "example_login",
            CredentialType::UsernamePassword,
            vec![("password", "s3cret"), ("username", "user@example.com")],
            Some("*.example.com"),
            None,
            None,
        )];
        let prompt = inject_credentials_into_system_prompt(&creds).unwrap();
        assert!(prompt.contains("Available credentials:"));
        assert!(prompt.contains("example_login (username_password) for *.example.com:"));
        assert!(prompt.contains("  username: user@example.com"));
        assert!(prompt.contains("  password: s3cret"));
    }

    #[test]
    fn test_inject_credentials_default_pattern() {
        let creds = vec![make_credential(
            "my_api",
            CredentialType::ApiKey,
            vec![("key", "abc123")],
            None,
            None,
            None,
        )];
        let prompt = inject_credentials_into_system_prompt(&creds).unwrap();
        assert!(prompt.contains("my_api (api_key) for *:"));
    }

    #[test]
    fn test_inject_credentials_multiple() {
        let creds = vec![
            make_credential(
                "login1",
                CredentialType::UsernamePassword,
                vec![("username", "u1")],
                Some("a.com"),
                None,
                None,
            ),
            make_credential(
                "login2",
                CredentialType::Token,
                vec![("token", "tok123")],
                Some("b.com"),
                None,
                None,
            ),
        ];
        let prompt = inject_credentials_into_system_prompt(&creds).unwrap();
        assert!(prompt.contains("login1 (username_password) for a.com:"));
        assert!(prompt.contains("login2 (token) for b.com:"));
    }

    #[test]
    fn test_credential_type_serde_roundtrip() {
        let types = vec![
            CredentialType::UsernamePassword,
            CredentialType::ApiKey,
            CredentialType::Token,
            CredentialType::Cookie,
            CredentialType::Custom,
        ];
        for ct in types {
            let json = serde_json::to_string(&ct).unwrap();
            let deserialized: CredentialType = serde_json::from_str(&json).unwrap();
            assert_eq!(ct, deserialized);
        }
    }

    #[test]
    fn test_credential_yaml_deserialization() {
        let yaml = r#"
name: test_cred
credential_type: api_key
fields:
  api_key: "my-secret-key"
url_pattern: "*.api.com"
"#;
        let cred: Credential = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cred.name, "test_cred");
        assert_eq!(cred.credential_type, CredentialType::ApiKey);
        assert_eq!(cred.fields.get("api_key").unwrap(), "my-secret-key");
        assert_eq!(cred.url_pattern.as_deref(), Some("*.api.com"));
    }

    #[test]
    fn test_credential_default_type() {
        let yaml = r#"
name: default_type_cred
fields: {}
"#;
        let cred: Credential = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cred.credential_type, CredentialType::UsernamePassword);
    }

    #[test]
    fn test_all_credential_types_in_prompt() {
        let types = vec![
            (CredentialType::UsernamePassword, "username_password"),
            (CredentialType::ApiKey, "api_key"),
            (CredentialType::Token, "token"),
            (CredentialType::Cookie, "cookie"),
            (CredentialType::Custom, "custom"),
        ];
        for (ct, expected_str) in types {
            let creds = vec![make_credential("t", ct, vec![], None, None, None)];
            let prompt = inject_credentials_into_system_prompt(&creds).unwrap();
            assert!(
                prompt.contains(expected_str),
                "Expected '{}' in prompt for credential type",
                expected_str
            );
        }
    }
}
