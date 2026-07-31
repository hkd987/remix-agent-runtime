use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use crate::error::AgentError;

// Re-export real crate types for use throughout the runtime
pub use remix_credentials::{Credential, CredentialSet, CredentialType};

/// Raw credential type for YAML deserialization.
/// Maps to `remix_credentials::CredentialType` during conversion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RawCredentialType {
    UsernamePassword,
    ApiKey,
    Token,
    Cookie,
    Custom,
}

fn default_credential_type() -> RawCredentialType {
    RawCredentialType::UsernamePassword
}

/// Raw credential struct for YAML config deserialization.
/// Implements `Clone` (required for `AppConfig`). Converted to the real
/// `remix_credentials::Credential` at the boundary via [`convert_raw_credentials`].
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct RawCredential {
    pub name: String,
    #[serde(default = "default_credential_type")]
    pub credential_type: RawCredentialType,
    #[serde(default)]
    pub fields: HashMap<String, String>,
    pub url_pattern: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl fmt::Debug for RawCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_fields: HashMap<&String, &str> =
            self.fields.keys().map(|k| (k, "***")).collect();
        f.debug_struct("RawCredential")
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

/// Normalizes flat username/password into the fields map, then converts
/// `RawCredential` values into real `remix_credentials::Credential` values
/// collected into a `CredentialSet`.
pub fn convert_raw_credentials(raws: &[RawCredential]) -> Result<CredentialSet, AgentError> {
    let mut set = CredentialSet::new();

    for raw in raws {
        let mut fields: Vec<(String, String)> = raw
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Normalize flat username/password into fields (don't overwrite existing)
        if let Some(ref username) = raw.username {
            if !raw.fields.contains_key("username") {
                fields.push(("username".to_string(), username.clone()));
            }
        }
        if let Some(ref password) = raw.password {
            if !raw.fields.contains_key("password") {
                fields.push(("password".to_string(), password.clone()));
            }
        }

        let cred_type = match raw.credential_type {
            RawCredentialType::UsernamePassword => CredentialType::UsernamePassword,
            RawCredentialType::ApiKey => CredentialType::ApiKey,
            RawCredentialType::Token => CredentialType::Token,
            RawCredentialType::Cookie => CredentialType::Cookie,
            RawCredentialType::Custom => CredentialType::Custom,
        };

        let credential = remix_credentials::Credential::new(
            raw.name.clone(),
            cred_type,
            fields,
            raw.url_pattern.clone(),
            raw.metadata.clone(),
        )
        .map_err(|e| AgentError::Config(format!("Invalid credential '{}': {}", raw.name, e)))?;

        set.add(credential);
    }

    Ok(set)
}

/// Formats credentials for inclusion in an LLM system prompt.
/// Returns `None` if no credentials are provided.
pub fn inject_credentials_into_system_prompt(creds: &CredentialSet) -> Option<String> {
    if creds.is_empty() {
        return None;
    }

    let mut lines = vec!["Available credentials:".to_string()];

    for cred in creds.iter() {
        let type_str = match cred.credential_type() {
            CredentialType::UsernamePassword => "username_password",
            CredentialType::ApiKey => "api_key",
            CredentialType::Token => "token",
            CredentialType::Cookie => "cookie",
            CredentialType::Custom => "custom",
        };

        let pattern = cred.url_pattern().unwrap_or("*");

        lines.push(format!("- {} ({}) for {}:", cred.name(), type_str, pattern));

        // Emit fields sorted by key for deterministic output
        let mut sorted_keys: Vec<&String> = cred.fields().keys().collect();
        sorted_keys.sort();

        for key in sorted_keys {
            if let Some(secret) = cred.field(key) {
                lines.push(format!("  {}: {}", key, secret.expose()));
            }
        }
    }

    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    /// (name, type, fields, optional domain) — named so the signature below stays
    /// readable instead of being a four-deep nested tuple.
    type CredentialSpec<'a> = (
        &'a str,
        CredentialType,
        Vec<(&'a str, &'a str)>,
        Option<&'a str>,
    );

    use super::*;

    fn make_raw_credential(
        name: &str,
        cred_type: RawCredentialType,
        fields: Vec<(&str, &str)>,
        url_pattern: Option<&str>,
        username: Option<&str>,
        password: Option<&str>,
    ) -> RawCredential {
        RawCredential {
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

    fn make_credential_set(creds: Vec<CredentialSpec<'_>>) -> CredentialSet {
        let mut set = CredentialSet::new();
        for (name, cred_type, fields, url_pattern) in creds {
            let field_vec: Vec<(String, String)> = fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            set.add(
                remix_credentials::Credential::new(
                    name.to_string(),
                    cred_type,
                    field_vec,
                    url_pattern.map(|s| s.to_string()),
                    HashMap::new(),
                )
                .unwrap(),
            );
        }
        set
    }

    // --- RawCredential tests ---

    #[test]
    fn test_debug_redacts_fields() {
        let cred = make_raw_credential(
            "test",
            RawCredentialType::UsernamePassword,
            vec![("password", "s3cret_value")],
            None,
            Some("alice_jones"),
            Some("hunter2"),
        );
        let debug_str = format!("{:?}", cred);
        assert!(!debug_str.contains("s3cret_value"));
        assert!(!debug_str.contains("alice_jones"));
        assert!(!debug_str.contains("hunter2"));
        assert!(debug_str.contains("***"));
        assert!(debug_str.contains("password"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_debug_redacts_no_fields() {
        let cred =
            make_raw_credential("empty", RawCredentialType::Custom, vec![], None, None, None);
        let debug_str = format!("{:?}", cred);
        assert!(debug_str.contains("empty"));
    }

    // --- convert_raw_credentials tests ---

    #[test]
    fn test_convert_normalizes_username_password() {
        let raws = vec![make_raw_credential(
            "login",
            RawCredentialType::UsernamePassword,
            vec![],
            Some("*.example.com"),
            Some("user@example.com"),
            Some("s3cret"),
        )];
        let set = convert_raw_credentials(&raws).unwrap();
        assert_eq!(set.len(), 1);
        let cred = set.get("login").unwrap();
        assert_eq!(cred.field("username").unwrap().expose(), "user@example.com");
        assert_eq!(cred.field("password").unwrap().expose(), "s3cret");
    }

    #[test]
    fn test_convert_does_not_overwrite_existing_fields() {
        let raws = vec![make_raw_credential(
            "login",
            RawCredentialType::UsernamePassword,
            vec![("username", "existing_user"), ("password", "existing_pass")],
            None,
            Some("new_user"),
            Some("new_pass"),
        )];
        let set = convert_raw_credentials(&raws).unwrap();
        let cred = set.get("login").unwrap();
        assert_eq!(cred.field("username").unwrap().expose(), "existing_user");
        assert_eq!(cred.field("password").unwrap().expose(), "existing_pass");
    }

    #[test]
    fn test_convert_empty() {
        let set = convert_raw_credentials(&[]).unwrap();
        assert!(set.is_empty());
    }

    #[test]
    fn test_convert_all_types() {
        let raws = vec![
            make_raw_credential(
                "up",
                RawCredentialType::UsernamePassword,
                vec![("username", "u"), ("password", "p")],
                None,
                None,
                None,
            ),
            make_raw_credential(
                "ak",
                RawCredentialType::ApiKey,
                vec![("api_key", "k")],
                None,
                None,
                None,
            ),
            make_raw_credential(
                "tok",
                RawCredentialType::Token,
                vec![("token", "t")],
                None,
                None,
                None,
            ),
            make_raw_credential(
                "ck",
                RawCredentialType::Cookie,
                vec![("cookie", "c")],
                None,
                None,
                None,
            ),
            make_raw_credential(
                "cust",
                RawCredentialType::Custom,
                vec![("x", "y")],
                None,
                None,
                None,
            ),
        ];
        let set = convert_raw_credentials(&raws).unwrap();
        assert_eq!(set.len(), 5);
        assert_eq!(
            set.get("up").unwrap().credential_type(),
            CredentialType::UsernamePassword
        );
        assert_eq!(
            set.get("ak").unwrap().credential_type(),
            CredentialType::ApiKey
        );
        assert_eq!(
            set.get("tok").unwrap().credential_type(),
            CredentialType::Token
        );
        assert_eq!(
            set.get("ck").unwrap().credential_type(),
            CredentialType::Cookie
        );
        assert_eq!(
            set.get("cust").unwrap().credential_type(),
            CredentialType::Custom
        );
    }

    #[test]
    fn test_convert_validation_error_empty_name() {
        let raws = vec![make_raw_credential(
            "",
            RawCredentialType::ApiKey,
            vec![("api_key", "k")],
            None,
            None,
            None,
        )];
        let result = convert_raw_credentials(&raws);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("name"));
    }

    #[test]
    fn test_convert_validation_error_missing_required_field() {
        let raws = vec![make_raw_credential(
            "bad",
            RawCredentialType::ApiKey,
            vec![],
            None,
            None,
            None,
        )];
        let result = convert_raw_credentials(&raws);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("api_key"));
    }

    // --- inject_credentials_into_system_prompt tests ---

    #[test]
    fn test_inject_credentials_none_when_empty() {
        let set = CredentialSet::new();
        assert!(inject_credentials_into_system_prompt(&set).is_none());
    }

    #[test]
    fn test_inject_credentials_formats_correctly() {
        let set = make_credential_set(vec![(
            "example_login",
            CredentialType::UsernamePassword,
            vec![("password", "s3cret"), ("username", "user@example.com")],
            Some("*.example.com"),
        )]);
        let prompt = inject_credentials_into_system_prompt(&set).unwrap();
        assert!(prompt.contains("Available credentials:"));
        assert!(prompt.contains("example_login (username_password) for *.example.com:"));
        assert!(prompt.contains("  username: user@example.com"));
        assert!(prompt.contains("  password: s3cret"));
    }

    #[test]
    fn test_inject_credentials_default_pattern() {
        let set = make_credential_set(vec![(
            "my_api",
            CredentialType::ApiKey,
            vec![("api_key", "abc123")],
            None,
        )]);
        let prompt = inject_credentials_into_system_prompt(&set).unwrap();
        assert!(prompt.contains("my_api (api_key) for *:"));
    }

    #[test]
    fn test_inject_credentials_multiple() {
        let set = make_credential_set(vec![
            (
                "login1",
                CredentialType::UsernamePassword,
                vec![("username", "u1"), ("password", "p1")],
                Some("a.com"),
            ),
            (
                "login2",
                CredentialType::Token,
                vec![("token", "tok123")],
                Some("b.com"),
            ),
        ]);
        let prompt = inject_credentials_into_system_prompt(&set).unwrap();
        assert!(prompt.contains("login1 (username_password) for a.com:"));
        assert!(prompt.contains("login2 (token) for b.com:"));
    }

    #[test]
    fn test_raw_credential_type_serde_roundtrip() {
        let types = vec![
            RawCredentialType::UsernamePassword,
            RawCredentialType::ApiKey,
            RawCredentialType::Token,
            RawCredentialType::Cookie,
            RawCredentialType::Custom,
        ];
        for ct in types {
            let json = serde_json::to_string(&ct).unwrap();
            let deserialized: RawCredentialType = serde_json::from_str(&json).unwrap();
            assert_eq!(ct, deserialized);
        }
    }

    #[test]
    fn test_raw_credential_yaml_deserialization() {
        let yaml = r#"
name: test_cred
credential_type: api_key
fields:
  api_key: "my-secret-key"
url_pattern: "*.api.com"
"#;
        let cred: RawCredential = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cred.name, "test_cred");
        assert_eq!(cred.credential_type, RawCredentialType::ApiKey);
        assert_eq!(cred.fields.get("api_key").unwrap(), "my-secret-key");
        assert_eq!(cred.url_pattern.as_deref(), Some("*.api.com"));
    }

    #[test]
    fn test_raw_credential_default_type() {
        let yaml = r#"
name: default_type_cred
fields: {}
"#;
        let cred: RawCredential = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cred.credential_type, RawCredentialType::UsernamePassword);
    }

    #[test]
    fn test_all_credential_types_in_prompt() {
        let types = vec![
            (
                CredentialType::UsernamePassword,
                "username_password",
                vec![("username", "u"), ("password", "p")],
            ),
            (CredentialType::ApiKey, "api_key", vec![("api_key", "k")]),
            (CredentialType::Token, "token", vec![("token", "t")]),
            (CredentialType::Cookie, "cookie", vec![("cookie", "c")]),
            (CredentialType::Custom, "custom", vec![("x", "y")]),
        ];
        for (ct, expected_str, fields) in types {
            let set = make_credential_set(vec![("t", ct, fields, None)]);
            let prompt = inject_credentials_into_system_prompt(&set).unwrap();
            assert!(
                prompt.contains(expected_str),
                "Expected '{}' in prompt for credential type",
                expected_str
            );
        }
    }

    // --- YAML backward compatibility ---

    #[test]
    fn test_yaml_backward_compat_flat_username_password() {
        let yaml = r#"
name: "site_login"
credential_type: username_password
username: "admin"
password: "secret"
url_pattern: "*.example.com"
"#;
        let raw: RawCredential = serde_yaml::from_str(yaml).unwrap();
        let set = convert_raw_credentials(&[raw]).unwrap();
        assert_eq!(set.len(), 1);
        let cred = set.get("site_login").unwrap();
        assert_eq!(cred.field("username").unwrap().expose(), "admin");
        assert_eq!(cred.field("password").unwrap().expose(), "secret");
        assert_eq!(cred.url_pattern(), Some("*.example.com"));
    }
}
