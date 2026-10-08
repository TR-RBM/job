use serde::{Deserialize, Serialize};

pub mod capability;
pub mod interrupt;
pub mod labels;
pub mod listing;
mod messages;
pub mod moving;
pub mod run;
pub mod settings;
pub mod stdin;
pub mod usage;

pub use messages::message;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Call {
    List { query: listing::Query },
    Move { id: u64, queue: String },
    Settings { path: String, key: Option<String> },
    Rows,
    Set { call: crate::idset::Call },
    Metrics,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    Jobs { listing: listing::Listing },
    Settings { explanation: settings::Explanation },
    Rows { view: crate::model::QueueView },
    Set { report: crate::idset::Report },
    Metrics { text: String },
}

impl Call {
    pub fn read_only(&self) -> bool {
        match self {
            Call::Move { .. } => false,
            Call::Set { call } => call.read_only(),
            _ => true,
        }
    }

    pub fn audited(&self) -> Option<(&'static str, String)> {
        match self {
            Call::Move { id, .. } => Some(("move", id.to_string())),
            Call::Set { call } => call.audited(),
            _ => None,
        }
    }
}

pub fn read_only(request: &crate::model::Request) -> bool {
    match request {
        crate::model::Request::Extended { call } => call.read_only(),
        crate::model::Request::Versioned { request, .. } => read_only(request),
        _ => false,
    }
}

pub fn ask(call: Call) -> Result<Answer, String> {
    match crate::call(crate::model::Request::Extended { call })? {
        crate::model::Response::Extended { answer } => Ok(*answer),
        crate::model::Response::Error { message } => Err(unknown_request(message)),
        other => Err(format!("unexpected answer {other:?}")),
    }
}

pub fn unknown_request(text: String) -> String {
    if text.contains("unknown variant `Extended`")
        || text.contains("unknown variant `Set`")
        || text.contains("unknown variant `Metrics`")
    {
        message("the running service is older than this command; restart it from the same release")
    } else {
        text
    }
}
