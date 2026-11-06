//! Deterministic redaction for event payloads and external transport.

use serde_json::{Map, Value};

/// Redaction policy with explicit secrets and JSON keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionPolicy {
    secrets: Vec<String>,
    keys: Vec<String>,
}

impl RedactionPolicy {
    /// Creates a policy with secret values and sensitive JSON keys.
    #[must_use]
    pub fn new(secrets: Vec<String>, keys: Vec<String>) -> Self {
        Self { secrets, keys }
    }

    /// Redacts configured secrets and sensitive keys.
    #[must_use]
    pub fn redact(&self, input: &str) -> String {
        match serde_json::from_str::<Value>(input) {
            Ok(value) => serde_json::to_string(&self.redact_value(value))
                .unwrap_or_else(|_| self.redact_plain(input)),
            Err(_) => self.redact_plain(input),
        }
    }

    fn redact_value(&self, value: Value) -> Value {
        match value {
            Value::Object(mut object) => {
                for key in &self.keys {
                    if object.contains_key(key) {
                        object.insert(key.clone(), Value::String("[REDACTED]".to_owned()));
                    }
                }
                Value::Object(
                    object
                        .into_iter()
                        .map(|(key, value)| (key, self.redact_value(value)))
                        .collect::<Map<String, Value>>(),
                )
            }
            Value::Array(values) => Value::Array(
                values
                    .into_iter()
                    .map(|value| self.redact_value(value))
                    .collect(),
            ),
            Value::String(text) => Value::String(self.redact_plain(&text)),
            other => other,
        }
    }

    fn redact_plain(&self, input: &str) -> String {
        let mut output = input.to_owned();
        for secret in &self.secrets {
            output = output.replace(secret, "[REDACTED]");
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_json_keys() {
        let policy = RedactionPolicy::new(vec![], vec!["token".to_owned(), "password".to_owned()]);
        let input = r#"{"token":"secret-token","password":"hunter2","message":"hello"}"#;

        let output = policy.redact(input);

        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("secret-token"));
        assert!(!output.contains("hunter2"));
        assert!(output.contains("hello"));
    }

    #[test]
    fn redacts_plain_secrets() {
        let policy = RedactionPolicy::new(vec!["api-key-123".to_owned()], vec![]);

        assert_eq!(policy.redact("key=api-key-123"), "key=[REDACTED]");
    }

    #[test]
    fn malformed_input_is_redacted_as_text() {
        let policy = RedactionPolicy::new(vec!["secret".to_owned()], vec![]);

        assert_eq!(policy.redact("not json secret"), "not json [REDACTED]");
    }
}
