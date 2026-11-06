//! A tool's arguments, read with messages the AI can act on.
//!
//! An AI sends numbers as strings now and then, and `"true"` for `true`: both are
//! taken. A missing or mistyped argument names itself in the error, so the next call
//! is right.

use crate::ToolError;
use serde_json::{Map, Value};

/// The arguments object of one call.
#[derive(Debug, Clone, Default)]
pub struct Args(Map<String, Value>);

impl Args {
    pub fn new(value: Value) -> Self {
        match value {
            Value::Object(m) => Self(m),
            _ => Self::default(),
        }
    }

    /// The raw value, absent or `null` read as absent.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    /// Was this argument sent at all (even `null`)? Lets an update tell "leave it"
    /// from "clear it".
    pub fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// Sent explicitly as `null` or `""`: the caller wants the value cleared.
    pub fn clears(&self, key: &str) -> bool {
        match self.0.get(key) {
            Some(Value::Null) => true,
            Some(Value::String(s)) => s.is_empty(),
            _ => false,
        }
    }

    pub fn str(&self, key: &str) -> Result<String, ToolError> {
        self.opt_str(key)?
            .ok_or_else(|| ToolError(format!("missing argument \"{key}\"")))
    }

    pub fn opt_str(&self, key: &str) -> Result<Option<String>, ToolError> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(Value::Number(n)) => Ok(Some(n.to_string())),
            Some(Value::Bool(b)) => Ok(Some(b.to_string())),
            Some(_) => Err(mistyped(key, "a string")),
        }
    }

    pub fn i64(&self, key: &str) -> Result<i64, ToolError> {
        self.opt_i64(key)?
            .ok_or_else(|| ToolError(format!("missing argument \"{key}\"")))
    }

    pub fn opt_i64(&self, key: &str) -> Result<Option<i64>, ToolError> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Number(n)) => n
                .as_i64()
                .or_else(|| n.as_f64().map(|f| f as i64))
                .map(Some)
                .ok_or_else(|| mistyped(key, "an integer")),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(Value::String(s)) => s
                .trim()
                .parse()
                .map(Some)
                .map_err(|_| mistyped(key, "an integer")),
            Some(_) => Err(mistyped(key, "an integer")),
        }
    }

    pub fn opt_f64(&self, key: &str) -> Result<Option<f64>, ToolError> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Number(n)) => Ok(n.as_f64()),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(Value::String(s)) => s
                .trim()
                .parse()
                .map(Some)
                .map_err(|_| mistyped(key, "a number")),
            Some(_) => Err(mistyped(key, "a number")),
        }
    }

    pub fn bool_or(&self, key: &str, default: bool) -> Result<bool, ToolError> {
        Ok(self.opt_bool(key)?.unwrap_or(default))
    }

    pub fn opt_bool(&self, key: &str) -> Result<Option<bool>, ToolError> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => Ok(Some(true)),
                "false" | "no" | "0" => Ok(Some(false)),
                "" => Ok(None),
                _ => Err(mistyped(key, "true or false")),
            },
            Some(Value::Number(n)) => Ok(Some(n.as_i64() != Some(0))),
            Some(_) => Err(mistyped(key, "true or false")),
        }
    }

    /// A list of strings; a single string is a list of one.
    pub fn str_list(&self, key: &str) -> Result<Vec<String>, ToolError> {
        match self.get(key) {
            None => Ok(Vec::new()),
            Some(Value::String(s)) => Ok(vec![s.clone()]),
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| match v {
                    Value::String(s) => Ok(s.clone()),
                    Value::Number(n) => Ok(n.to_string()),
                    _ => Err(mistyped(key, "a list of strings")),
                })
                .collect(),
            Some(_) => Err(mistyped(key, "a list of strings")),
        }
    }

    /// A list of integers (ids); a single one is a list of one.
    pub fn i64_list(&self, key: &str) -> Result<Vec<i64>, ToolError> {
        let un = |v: &Value| -> Result<i64, ToolError> {
            match v {
                Value::Number(n) => n.as_i64().ok_or_else(|| mistyped(key, "integers")),
                Value::String(s) => s.trim().parse().map_err(|_| mistyped(key, "integers")),
                _ => Err(mistyped(key, "integers")),
            }
        };
        match self.get(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(a)) => a.iter().map(un).collect(),
            Some(v) => Ok(vec![un(v)?]),
        }
    }

    /// The whole object, for a tool that reads nested values itself.
    pub fn raw(&self) -> &Map<String, Value> {
        &self.0
    }
}

fn mistyped(key: &str, wanted: &str) -> ToolError {
    ToolError(format!("argument \"{key}\" must be {wanted}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lenient_with_what_an_ai_sends() {
        let a = Args::new(json!({"n": "12", "b": "true", "ids": [1, "2"], "one": "x", "z": null}));
        assert_eq!(a.i64("n").unwrap(), 12);
        assert!(a.bool_or("b", false).unwrap());
        assert_eq!(a.i64_list("ids").unwrap(), vec![1, 2]);
        assert_eq!(a.str_list("one").unwrap(), vec!["x".to_string()]);
        assert_eq!(a.opt_str("z").unwrap(), None);
        assert!(a.has("z") && a.clears("z"));
    }

    #[test]
    fn errors_name_the_argument() {
        let a = Args::new(json!({"n": [1]}));
        assert!(a.i64("n").unwrap_err().0.contains("\"n\""));
        assert!(a.str("missing").unwrap_err().0.contains("\"missing\""));
    }
}
