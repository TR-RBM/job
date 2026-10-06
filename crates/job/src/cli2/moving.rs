use std::process::ExitCode;

use super::{Call, message};
use crate::model::{Request, Response};

pub fn run(id: u64, queue: &str, json: bool) -> Result<ExitCode, String> {
    match crate::call(Request::Extended {
        call: Call::Move {
            id,
            queue: queue.to_owned(),
        },
    })? {
        Response::Submitted { job } => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&job).map_err(|error| error.to_string())?
                );
            } else {
                println!(
                    "{}",
                    message("Job {id} is in Queue {queue}")
                        .replace("{id}", &job.id.to_string())
                        .replace("{queue}", job.spec.queue.as_deref().unwrap_or("default"))
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Response::Error { message } => Err(super::unknown_request(message)),
        other => Err(format!("unexpected answer {other:?}")),
    }
}
