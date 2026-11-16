//! Untyped DevTools protocol commands (`method` + JSON `params`), used where
//! the generated types are not needed or not expressive enough (CDP sessions,
//! commands whose optional fields must stay absent, ...).

use std::borrow::Cow;

use chromiumoxide::types::{Command, Method, MethodId};
use serde::Serialize;
use serde_json::Value;

/// A raw CDP command; its response is the raw JSON `result`.
#[derive(Debug, Clone)]
pub(crate) struct RawCommand {
    method: String,
    params: Value,
}

impl RawCommand {
    pub(crate) fn new(method: impl Into<String>, params: Value) -> Self {
        let params = if params.is_null() {
            Value::Object(Default::default())
        } else {
            params
        };
        Self {
            method: method.into(),
            params,
        }
    }
}

impl Serialize for RawCommand {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.params.serialize(serializer)
    }
}

impl Method for RawCommand {
    fn identifier(&self) -> MethodId {
        Cow::Owned(self.method.clone())
    }
}

impl Command for RawCommand {
    type Response = Value;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_command_serializes_params() {
        let cmd = RawCommand::new("Page.navigate", serde_json::json!({ "url": "about:blank" }));
        assert_eq!(cmd.identifier(), "Page.navigate");
        assert_eq!(
            serde_json::to_value(&cmd).unwrap(),
            serde_json::json!({ "url": "about:blank" })
        );
        let empty = RawCommand::new("Browser.getVersion", Value::Null);
        assert_eq!(serde_json::to_value(&empty).unwrap(), serde_json::json!({}));
    }
}
