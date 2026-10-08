mod gather;
mod http;
mod messages;
mod observed;
mod text;

pub use messages::message;

use std::io;
use std::net::{SocketAddr, TcpListener};
use std::process::ExitCode;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::cli2::{Answer, Call};
use crate::daemon::Shared;
use crate::model::{Job, Request, Response, State};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
}

impl Settings {
    pub fn unset(&self) -> bool {
        self.listen.is_none()
    }

    pub fn address(&self) -> Result<Option<SocketAddr>, String> {
        let Some(listen) = &self.listen else {
            return Ok(None);
        };
        listen.parse::<SocketAddr>().map(Some).map_err(|_| {
            message(
                "metrics listen must be an IP address and a port, such as 127.0.0.1:9877 or [::1]:9877, not `{value}`",
            )
            .replace("{value}", listen)
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        self.address().map(|_| ())
    }
}

pub fn bind(settings: &Settings) -> io::Result<Option<TcpListener>> {
    let Some(address) = settings.address().map_err(io::Error::other)? else {
        return Ok(None);
    };
    let listener = TcpListener::bind(address).map_err(|error| {
        io::Error::other(
            message("cannot listen for metrics on {address}: {error}")
                .replace("{address}", &address.to_string())
                .replace("{error}", &error.to_string()),
        )
    })?;
    let bound = listener.local_addr()?;
    eprintln!(
        "job daemon: {}",
        message("metrics at http://{address}/metrics").replace("{address}", &bound.to_string())
    );
    if !bound.ip().is_loopback() {
        eprintln!(
            "job daemon: {}",
            message("the metrics address {address} is not a loopback address; everyone who can reach it can read the metrics")
                .replace("{address}", &bound.to_string())
        );
    }
    Ok(Some(listener))
}

pub fn serve(shared: Arc<Shared>, listener: Option<TcpListener>) -> io::Result<()> {
    match listener {
        Some(listener) => http::serve(shared, listener),
        None => Ok(()),
    }
}

pub fn transition(job: &Job, to: &State, wait_ms: Option<u64>) {
    observed::transition(job, to, wait_ms);
}

pub fn answer(shared: &Shared) -> Response {
    Response::Extended {
        answer: Box::new(Answer::Metrics {
            text: gather::render(shared),
        }),
    }
}

pub fn command(rest: &[String]) -> Result<ExitCode, String> {
    if let Some(word) = rest.first() {
        crate::commands::output::usage();
        return Err(
            message("job metrics takes no `{word}`; see job metrics --help")
                .replace("{word}", word),
        );
    }
    let response = match crate::call(Request::Extended {
        call: Call::Metrics,
    }) {
        Ok(response) => response,
        Err(error) => {
            print!("{}", gather::down());
            return Err(error);
        }
    };
    match response {
        Response::Extended { answer } => match *answer {
            Answer::Metrics { text } => {
                print!("{text}");
                Ok(ExitCode::SUCCESS)
            }
            _ => Err(message("the service gave an unexpected answer")),
        },
        Response::Error { message } => Err(crate::cli2::unknown_request(message)),
        _ => Err(message("the service gave an unexpected answer")),
    }
}
