use super::interrupt::Policy;
use super::message;

pub fn prepare(
    chosen: Option<Policy>,
    terminal: bool,
    compatibility: bool,
    session: &str,
) -> Result<(), String> {
    if terminal {
        return match chosen {
            Some(_) => Err(message(
                "--on-interrupt goes with run in pipe mode; with --pty the terminal carries Ctrl-C to the Job",
            )),
            None => Ok(()),
        };
    }
    if compatibility && chosen.is_none() {
        return Ok(());
    }
    super::interrupt::install(chosen.unwrap_or(Policy::Forward), session)
}
