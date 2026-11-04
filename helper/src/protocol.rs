use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, serde::Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    /// Structured facts about the failure, e.g. the dialog behind `dialog_open`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A failed request whose error may carry structured `data`. Plain `(code, message)` errors
/// convert into it, so `?` keeps working on them.
#[derive(Debug)]
pub struct Failure {
    pub code: String,
    pub message: String,
    pub data: Option<Value>,
}

impl From<(String, String)> for Failure {
    fn from((code, message): (String, String)) -> Self {
        Self { code, message, data: None }
    }
}

impl Failure {
    /// Ticket 87: marks whether the request sent its input before it failed, as `data.input_sent`
    /// (the server's batch `sent`). A mark already there is kept: the code nearest the input knows
    /// best. Where it is not known, no mark is set.
    pub fn with_input_sent(mut self, sent: bool) -> Self {
        let data = self.data.get_or_insert_with(|| json!({}));
        if let Value::Object(fields) = data {
            fields.entry("input_sent").or_insert(Value::Bool(sent));
        }
        self
    }

    pub fn into_response(self, id: u64) -> Response {
        Response {
            id,
            ok: false,
            result: None,
            error: Some(ErrorBody { code: self.code, message: self.message, data: self.data }),
        }
    }
}

pub fn ok(id: u64, result: Value) -> Response {
    Response { id, ok: true, result: Some(result), error: None }
}

pub fn err(id: u64, code: &str, message: impl Into<String>) -> Response {
    Response {
        id,
        ok: false,
        result: None,
        error: Some(ErrorBody { code: code.to_string(), message: message.into(), data: None }),
    }
}

#[allow(dead_code)]
pub fn event(name: &str, data: Option<Value>) -> Value {
    match data {
        Some(data) => json!({ "event": name, "data": data }),
        None => json!({ "event": name }),
    }
}
