use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::model::{Declared, Env, Request, Response, Spec};

const PING_TIMEOUT_S: u64 = 2;

pub fn call(socket: &Path, request: &Request) -> io::Result<Response> {
    crate::commands::output::contacted();
    let versioned = crate::durability::negotiation::envelope(request);
    let mut stream = UnixStream::connect(socket)?;
    if matches!(request, Request::Ping) {
        stream.set_read_timeout(Some(std::time::Duration::from_secs(PING_TIMEOUT_S)))?;
    }
    if let Request::Wait { timeout_ms, .. } = request
        && *timeout_ms <= 1000
    {
        stream.set_read_timeout(Some(std::time::Duration::from_millis(timeout_ms + 1000)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(1)))?;
    }
    let mut bytes = serde_json::to_vec(&versioned)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    serde_json::from_str(&line)
        .map(crate::durability::negotiation::understood)
        .map_err(io::Error::other)
}

pub fn argv_of(words: &[String]) -> Vec<String> {
    words.to_vec()
}

pub fn spec(
    words: &[String],
    cwd: PathBuf,
    session: String,
    declared: Declared,
    queue: Option<String>,
) -> Spec {
    Spec {
        argv: argv_of(words),
        cwd,
        session,
        declared,
        queue,
    }
}

pub fn env() -> Env {
    if let Some(env) = crate::clientif::invocation::env() {
        return env;
    }
    Env {
        vars: std::env::vars().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_argument_and_multiple_arguments_remain_literal() {
        assert_eq!(
            argv_of(&["cargo test | tail".to_string()]),
            vec!["cargo test | tail"]
        );
        assert_eq!(
            argv_of(&["cargo".to_string(), "test".to_string()]),
            vec!["cargo", "test"]
        );
    }

    #[test]
    fn resource_submissions_cannot_be_mistaken_for_legacy_requests() {
        use std::os::unix::net::UnixListener;
        let path =
            std::env::temp_dir().join(format!("job-version-envelope-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            writeln!(
                stream,
                "{{\"Error\":{{\"message\":\"unsupported envelope\"}}}}"
            )
            .unwrap();
            value
        });
        let request = Request::Submit {
            spec: Box::new(Spec {
                argv: vec!["true".to_owned()],
                cwd: std::env::temp_dir(),
                session: "test".to_owned(),
                queue: None,
                declared: Declared {
                    resources: crate::resource_policy::Controls {
                        cpu_limit_milli: Some(crate::resource_policy::Limit::Value(500)),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }),
            env: Env { vars: Vec::new() },
            idempotency_key: None,
        };
        assert!(matches!(
            call(&path, &request).unwrap(),
            Response::Error { .. }
        ));
        let wire = peer.join().unwrap();
        assert!(wire.get("Submit").is_none());
        assert_eq!(wire["Versioned"]["protocol"], crate::model::PROTOCOL);
        assert_eq!(
            wire["Versioned"]["request"]["Submit"]["spec"]["declared"]["cpu_limit_milli"],
            500
        );
        std::fs::remove_file(path).unwrap();
    }
}
