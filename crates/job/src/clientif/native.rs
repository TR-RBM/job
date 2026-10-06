use serde_json::{Map, Value, json};

use crate::daemon::Shared;
use crate::daemon::client_service;
use crate::durability::negotiation::MIN_PROTOCOL;
use crate::model::{PROTOCOL, Request};

use super::message;

fn enveloped(request: &Value) -> bool {
    match request {
        Value::String(name) => name == "Versioned",
        Value::Object(map) => map.len() == 1 && map.contains_key("Versioned"),
        _ => false,
    }
}

pub fn answer(shared: &Shared, args: &Map<String, Value>) -> Result<String, String> {
    let request = args
        .get("request")
        .ok_or_else(|| message("native needs request: one request of the internal protocol"))?;
    if enveloped(request) {
        return Err(message(
            "native does not carry the internal version envelope; send the request itself",
        ));
    }
    let response = match serde_json::from_value::<Request>(request.clone()) {
        Ok(Request::Output { .. }) => {
            return Err(message(
                "native does not carry the internal output stream; use the request output",
            ));
        }
        Ok(Request::LogQuery { id, attempt, query }) => {
            client_service::queried(shared, id, attempt, &query)
        }
        Ok(known)
            if crate::durability::audit::mutating(&known)
                || matches!(known, Request::Done { .. }) =>
        {
            return Err(message(
                "native carries reading requests only; send a change as the words of a job command with the request command",
            ));
        }
        _ => {
            let line = json!({
                "Versioned": {"protocol": PROTOCOL, "min": MIN_PROTOCOL, "request": request},
            })
            .to_string();
            client_service::passed(shared, line.as_bytes())
        }
    };
    let answer = crate::netsecret::rendered(&response)
        .map_err(|error| {
            message("the service could not write its own answer: {error}")
                .replace("{error}", &error.to_string())
        })
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())?;
    Ok(format!("{{\"protocol\":{PROTOCOL},\"answer\":{answer}}}"))
}
