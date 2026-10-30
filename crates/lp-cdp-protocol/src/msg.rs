//! CDP message envelope: parsing commands and serializing responses/events.
//!
//! `params` stay as borrowed raw JSON until a domain handler deserializes them
//! into its own typed struct, so routing never builds an intermediate value tree.

use std::borrow::Cow;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::ws::OutFrame;

/// JSON-RPC style error codes used by Chrome's DevTools protocol.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const SERVER_ERROR: i64 = -32000;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
    /// JSON-RPC `error.data`: structured details (a failed screenshot match
    /// carries its images).
    pub data: Option<serde_json::Value>,
}

impl CdpError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    pub fn with_data(mut self, data: Option<serde_json::Value>) -> Self {
        self.data = data;
        self
    }

    pub fn server(message: impl Into<String>) -> Self {
        Self::new(code::SERVER_ERROR, message)
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(code::INVALID_PARAMS, message)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(code::METHOD_NOT_FOUND, format!("'{method}' wasn't found"))
    }

    /// Explicit refusal for protocol surface the rewrite does not implement yet.
    pub fn unsupported(method: &str) -> Self {
        Self::new(code::SERVER_ERROR, format!("{method} is not supported by lightningpanda"))
    }
}

pub type CdpResult<T> = Result<T, CdpError>;

/// A parsed command. Borrows from the message text.
#[derive(Debug, Deserialize)]
pub struct Command<'a> {
    pub id: u64,
    #[serde(borrow)]
    pub method: Cow<'a, str>,
    #[serde(borrow, default)]
    params: Option<&'a RawValue>,
    #[serde(rename = "sessionId", borrow, default)]
    pub session_id: Option<Cow<'a, str>>,
}

impl<'a> Command<'a> {
    /// Splits `Domain.method`.
    pub fn domain_and_action(&self) -> Option<(&str, &str)> {
        self.method.split_once('.')
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Deserializes params into `T`; absent params deserialize from `{}`.
    pub fn params<T: DeserializeOwned>(&self) -> CdpResult<T> {
        let _t = crate::perf::span("cdp.params");
        let raw = self.params.map_or("{}", RawValue::get);
        serde_json::from_str(raw).map_err(|e| CdpError::invalid_params(format!("Invalid parameters: {e}")))
    }

    /// `params` for a type that borrows from the message text (`Cow<str>`
    /// fields with `#[serde(borrow)]` avoid copying large strings such as a
    /// `functionDeclaration`).
    pub fn params_borrowed<T: Deserialize<'a>>(&self) -> CdpResult<T> {
        let _t = crate::perf::span("cdp.params");
        let raw = self.params.map_or("{}", RawValue::get);
        serde_json::from_str(raw).map_err(|e| CdpError::invalid_params(format!("Invalid parameters: {e}")))
    }

    pub fn raw_params(&self) -> Option<&'a RawValue> {
        self.params
    }
}

/// Why an incoming text frame could not be parsed into a command. When the id
/// was recoverable an error response must still be sent for it.
#[derive(Debug)]
pub struct ParseFailure {
    pub id: Option<u64>,
    pub session_id: Option<String>,
    pub error: CdpError,
}

pub fn parse_command(text: &str) -> Result<Command<'_>, ParseFailure> {
    serde_json::from_str::<Command<'_>>(text).map_err(|e| {
        // Recover id/sessionId so a client waiting on that id is answered.
        #[derive(Deserialize)]
        struct Loose<'a> {
            id: Option<u64>,
            #[serde(rename = "sessionId", borrow)]
            session_id: Option<Cow<'a, str>>,
        }
        let loose = serde_json::from_str::<Loose<'_>>(text).ok();
        let code = if loose.is_some() { code::INVALID_REQUEST } else { code::PARSE_ERROR };
        ParseFailure {
            id: loose.as_ref().and_then(|l| l.id),
            session_id: loose.and_then(|l| l.session_id.map(Cow::into_owned)),
            error: CdpError::new(code, format!("Message must be a valid command: {e}")),
        }
    })
}

/// Serializes `{"id":..,"result":..,"sessionId":..}` straight into a frame.
pub fn result_frame<T: Serialize + ?Sized>(id: u64, result: &T, session_id: Option<&str>) -> OutFrame {
    #[derive(Serialize)]
    struct Out<'a, T: ?Sized> {
        id: u64,
        result: &'a T,
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<&'a str>,
    }
    to_frame(&Out { id, result, session_id })
}

pub fn error_frame(id: Option<u64>, error: &CdpError, session_id: Option<&str>) -> OutFrame {
    #[derive(Serialize)]
    struct ErrorBody<'a> {
        code: i64,
        message: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<&'a serde_json::Value>,
    }
    #[derive(Serialize)]
    struct Out<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        error: ErrorBody<'a>,
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<&'a str>,
    }
    to_frame(&Out {
        id,
        error: ErrorBody { code: error.code, message: &error.message, data: error.data.as_ref() },
        session_id,
    })
}

pub fn event_frame<T: Serialize + ?Sized>(method: &str, params: &T, session_id: Option<&str>) -> OutFrame {
    #[derive(Serialize)]
    struct Out<'a, T: ?Sized> {
        method: &'a str,
        params: &'a T,
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<&'a str>,
    }
    to_frame(&Out { method, params, session_id })
}

fn to_frame<T: Serialize>(value: &T) -> OutFrame {
    let _t = crate::perf::span("json.serialize");
    let mut frame = OutFrame::with_capacity(128);
    // Serializing plain structs/maps into a Vec cannot fail.
    serde_json::to_writer(frame.payload(), value).expect("CDP message serializes");
    frame
}

/// Empty `{}` result for commands that return nothing.
#[derive(Serialize)]
pub struct Empty {}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(frame: OutFrame) -> String {
        let bytes = frame.finish_text();
        let wire = bytes.as_ref();
        let header = match wire[1] {
            126 => 4,
            127 => 10,
            _ => 2,
        };
        String::from_utf8(wire[header..].to_vec()).unwrap()
    }

    #[test]
    fn parses_command_with_raw_params() {
        let cmd = parse_command(r#"{"id":7,"method":"Page.navigate","params":{"url":"http://x/"},"sessionId":"S1"}"#)
            .unwrap();
        assert_eq!(cmd.id, 7);
        assert_eq!(cmd.domain_and_action(), Some(("Page", "navigate")));
        assert_eq!(cmd.session_id(), Some("S1"));
        #[derive(Deserialize)]
        struct P {
            url: String,
        }
        assert_eq!(cmd.params::<P>().unwrap().url, "http://x/");
    }

    #[test]
    fn missing_params_deserialize_from_empty_object() {
        #[derive(Deserialize)]
        struct P {
            #[serde(default)]
            enabled: bool,
        }
        let cmd = parse_command(r#"{"id":1,"method":"Page.enable"}"#).unwrap();
        assert!(!cmd.params::<P>().unwrap().enabled);
    }

    #[test]
    fn bad_params_are_invalid_params() {
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct P {
            url: String,
        }
        let cmd = parse_command(r#"{"id":1,"method":"Page.navigate","params":{"url":3}}"#).unwrap();
        assert_eq!(cmd.params::<P>().unwrap_err().code, code::INVALID_PARAMS);
    }

    #[test]
    fn parse_failures_recover_id_when_possible() {
        let f = parse_command(r#"{"id":3,"sessionId":"S"}"#).unwrap_err();
        assert_eq!((f.id, f.session_id.as_deref(), f.error.code), (Some(3), Some("S"), code::INVALID_REQUEST));
        let f = parse_command("not json").unwrap_err();
        assert_eq!((f.id, f.error.code), (None, code::PARSE_ERROR));
    }

    #[test]
    fn serializes_envelopes() {
        assert_eq!(text(result_frame(1, &Empty {}, None)), r#"{"id":1,"result":{}}"#);
        assert_eq!(
            text(error_frame(Some(2), &CdpError::method_not_found("X.y"), Some("S"))),
            r#"{"id":2,"error":{"code":-32601,"message":"'X.y' wasn't found"},"sessionId":"S"}"#
        );
        assert_eq!(
            text(event_frame("Page.loadEventFired", &serde_json::json!({"timestamp":1.5}), Some("S"))),
            r#"{"method":"Page.loadEventFired","params":{"timestamp":1.5},"sessionId":"S"}"#
        );
    }
}
