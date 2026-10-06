use serde_json::Value;

use crate::model::{PROTOCOL, Request, Response};

pub const MIN_PROTOCOL: u32 = PROTOCOL;

fn unsupported() -> Response {
    Response::Unsupported {
        min: MIN_PROTOCOL,
        max: PROTOCOL,
        version: crate::daemon::version(),
    }
}

pub fn envelope(request: &Request) -> Request {
    match request {
        Request::Versioned { .. } => request.clone(),
        _ => Request::Versioned {
            protocol: PROTOCOL,
            min: Some(MIN_PROTOCOL),
            request: Box::new(request.clone()),
        },
    }
}

pub enum Refusal {
    Unreadable(Box<Response>),
    Unknown(String, Box<Response>),
    Unsupported(Option<Box<Request>>, Box<Response>),
}

fn failure(text: String) -> Box<Response> {
    Box::new(Response::Error { message: text })
}

fn garbled(detail: &dyn std::fmt::Display) -> Refusal {
    Refusal::Unreadable(failure(format!("unreadable request: {detail}")))
}

fn name(raw: &Value) -> String {
    match raw {
        Value::String(name) => name.clone(),
        Value::Object(map) if map.len() == 1 => map.keys().next().cloned().unwrap_or_default(),
        _ => "?".to_owned(),
    }
}

fn spoken(text: &str, protocol: u64) -> String {
    super::message(text)
        .replace("{min}", &MIN_PROTOCOL.to_string())
        .replace("{max}", &PROTOCOL.to_string())
        .replace("{version}", &crate::daemon::version())
        .replace("{protocol}", &protocol.to_string())
}

fn inner(protocol: u64, raw: &Value) -> Result<Request, Refusal> {
    let error = match serde_json::from_value::<Request>(raw.clone()) {
        Ok(request) => return Ok(request),
        Err(error) => error.to_string(),
    };
    let name: String = name(raw).chars().take(64).collect();
    if error.starts_with("unknown variant") {
        let message = spoken(
            "this service does not know request {name}; it speaks request protocol {min} to {max} (version {version}) and the request came as protocol {protocol}",
            protocol,
        )
        .replace("{name}", &name);
        Err(Refusal::Unknown(name, failure(message)))
    } else {
        let message = spoken(
            "this service cannot read request {name} ({error}); it speaks request protocol {min} to {max} (version {version}) and the request came as protocol {protocol}",
            protocol,
        )
        .replace("{name}", &name)
        .replace("{error}", &error);
        Err(Refusal::Unknown(name, failure(message)))
    }
}

pub fn receive(line: &[u8]) -> Result<Request, Refusal> {
    let value: Value = serde_json::from_slice(line).map_err(|error| garbled(&error))?;
    let body = match &value {
        Value::Object(map) if map.len() == 1 => map.get("Versioned"),
        _ => None,
    };
    let Some(body) = body else {
        let request: Request = serde_json::from_value(value).map_err(|error| garbled(&error))?;
        return if super::audit::mutating(&request) {
            Err(Refusal::Unsupported(
                Some(Box::new(request)),
                Box::new(unsupported()),
            ))
        } else {
            Ok(request)
        };
    };
    let missing = || garbled(&"Versioned needs protocol, an optional min and request");
    let protocol = body
        .get("protocol")
        .and_then(Value::as_u64)
        .ok_or_else(missing)?;
    let lowest = match body.get("min") {
        None | Some(Value::Null) => protocol,
        Some(min) => min.as_u64().ok_or_else(missing)?,
    };
    let raw = body.get("request").ok_or_else(missing)?;
    if lowest > protocol
        || lowest > u64::from(PROTOCOL)
        || protocol < u64::from(MIN_PROTOCOL)
        || name(raw) == "Versioned"
    {
        return Err(Refusal::Unsupported(
            serde_json::from_value(raw.clone()).ok().map(Box::new),
            Box::new(unsupported()),
        ));
    }
    inner(protocol, raw)
}

pub fn understood(response: Response) -> Response {
    match response {
        Response::Unsupported { min, max, version } => Response::Error {
            message: super::message(
                "this job speaks request protocol {own_min} to {own_max} (version {own_version}); the daemon speaks {min} to {max} (version {version}); restart the daemon from the same release, or install it for both",
            )
            .replace("{own_min}", &MIN_PROTOCOL.to_string())
            .replace("{own_max}", &PROTOCOL.to_string())
            .replace("{own_version}", &crate::daemon::version())
            .replace("{min}", &min.to_string())
            .replace("{max}", &max.to_string())
            .replace("{version}", &version),
        },
        other => other,
    }
}
