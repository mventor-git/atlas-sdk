//! Minimal payload type.
//!
//! ponytail: hand-rolled instead of serde. Contracts and events need a
//! language-neutral-ish payload for the proof; serde would be the upgrade
//! path if payloads must round-trip arbitrary JSON.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    pub fn str(s: impl Into<String>) -> Self {
        Value::Str(s.into())
    }
    pub fn num(n: impl Into<f64>) -> Self {
        Value::Num(n.into())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(m) => m.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Convenience for building a small map payload.
    pub fn map(pairs: Vec<(&str, Value)>) -> Self {
        Value::Map(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Num(n) => write!(f, "{n}"),
            Value::Str(s) => write!(f, "{s}"),
            Value::List(items) => {
                let rendered: Vec<String> = items.iter().map(|i| i.to_string()).collect();
                write!(f, "[{}]", rendered.join(", "))
            }
            Value::Map(entries) => {
                let rendered: Vec<String> =
                    entries.iter().map(|(k, v)| format!("{k}={v}")).collect();
                write!(f, "{{{}}}", rendered.join(", "))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_nested_payload() {
        let v = Value::map(vec![
            ("kind", Value::str("order")),
            ("total", Value::num(42)),
            ("tags", Value::List(vec![Value::str("a"), Value::str("b")])),
        ]);
        assert_eq!(v.to_string(), "{kind=order, tags=[a, b], total=42}");
    }

    #[test]
    fn accesses_fields() {
        let v = Value::map(vec![("quantity", Value::num(3))]);
        assert_eq!(v.get("quantity").and_then(Value::as_num), Some(3.0));
        assert!(v.get("missing").is_none());
    }
}
