use std::cell::RefCell;
use std::fmt::Write;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crate::daemon::Shared;
use crate::model::{Env, Request, Response};
use crate::terminal::Size;

use super::message;

pub struct Invocation {
    pub service: Arc<Shared>,
    pub cwd: Option<PathBuf>,
    pub env: Env,
    pub session: String,
    pub terminal: Size,
    pub enveloped: bool,
    pub usage: bool,
    pub contacted: bool,
    pub out: String,
    pub err: String,
}

thread_local! {
    static CURRENT: RefCell<Option<Invocation>> = const { RefCell::new(None) };
}

struct Entered;

impl Drop for Entered {
    fn drop(&mut self) {
        CURRENT.with(|current| current.borrow_mut().take());
    }
}

struct Suspended(Option<Invocation>);

impl Drop for Suspended {
    fn drop(&mut self) {
        CURRENT.with(|current| *current.borrow_mut() = self.0.take());
    }
}

fn read<T>(view: impl FnOnce(&Invocation) -> T) -> Option<T> {
    CURRENT.with(|current| current.borrow().as_ref().map(view))
}

fn change(apply: impl FnOnce(&mut Invocation)) -> bool {
    CURRENT.with(|current| current.borrow_mut().as_mut().map(apply).is_some())
}

pub fn run<T>(
    invocation: Invocation,
    action: impl FnOnce() -> T,
) -> (Option<T>, Option<Invocation>) {
    CURRENT.with(|current| *current.borrow_mut() = Some(invocation));
    let entered = Entered;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).ok();
    let invocation = CURRENT.with(|current| current.borrow_mut().take());
    drop(entered);
    (result, invocation)
}

pub fn active() -> bool {
    read(|_| ()).is_some()
}

pub fn write(error: bool, line: bool, arguments: std::fmt::Arguments<'_>) -> bool {
    change(|invocation| {
        let sink = if error {
            &mut invocation.err
        } else {
            &mut invocation.out
        };
        let _ = sink.write_fmt(arguments);
        if line {
            sink.push('\n');
        }
    })
}

pub fn usage() -> bool {
    change(|invocation| invocation.usage = true)
}

pub fn contacted() -> bool {
    change(|invocation| invocation.contacted = true)
}

pub fn enveloped() -> Option<bool> {
    read(|invocation| invocation.enveloped)
}

pub fn flags() -> Option<(bool, bool)> {
    read(|invocation| (invocation.usage, invocation.contacted))
}

pub fn session() -> String {
    read(|invocation| invocation.session.clone())
        .or_else(|| std::env::var("JOB_SESSION").ok())
        .unwrap_or_else(|| "unnamed".to_owned())
}

pub fn terminal() -> Size {
    read(|invocation| invocation.terminal).unwrap_or_else(Size::current)
}

pub fn env() -> Option<Env> {
    read(|invocation| invocation.env.clone())
}

pub fn cwd() -> io::Result<PathBuf> {
    match read(|invocation| invocation.cwd.clone()) {
        Some(Some(cwd)) => Ok(cwd),
        Some(None) => Err(io::Error::other(message(
            "the request gives no cwd, and this command needs one",
        ))),
        None => std::env::current_dir(),
    }
}

pub fn file(path: &str) -> Result<PathBuf, String> {
    if active() && !std::path::Path::new(path).is_absolute() {
        cwd()
            .map(|cwd| cwd.join(path))
            .map_err(|error| error.to_string())
    } else {
        Ok(PathBuf::from(path))
    }
}

pub fn exchange(request: &Request) -> Option<Result<Response, String>> {
    let mut invocation = CURRENT.with(|current| current.borrow_mut().take())?;
    invocation.contacted = true;
    let service = Arc::clone(&invocation.service);
    let suspended = Suspended(Some(invocation));
    let answer = crate::daemon::client_service::carried(&service, request);
    drop(suspended);
    Some(answer.map_err(|error| error.to_string()))
}
