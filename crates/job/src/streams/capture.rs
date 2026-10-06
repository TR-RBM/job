use std::io::{PipeReader, Read};
use std::os::fd::AsRawFd;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use super::{HEADER, READ, Record, Recorder, Source, invalid, message};

pub fn pipes(
    stdout: PipeReader,
    stderr: PipeReader,
    mut log: Recorder,
    ended: Receiver<i32>,
    remote: bool,
) -> super::Outcome {
    let mut pipes = [stdout, stderr];
    let mut open = [true, true];
    for pipe in &pipes {
        let fd = pipe.as_raw_fd();
        if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
            log.error(std::io::Error::last_os_error().to_string());
            return log.finish();
        }
    }
    log.paced();
    let mut deadline = None;
    let mut buffer = vec![0; READ];
    let mut framed = Vec::new();
    let mut invalid_frame = false;
    let mut remote_complete = false;
    while open.iter().any(|v| *v) {
        if deadline.is_none() && ended.try_recv().is_ok() {
            deadline = Some(Instant::now() + Duration::from_secs(1));
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            log.error(message("output pipes remained open after workload exit"));
            break;
        }
        let mut fds = std::array::from_fn::<_, 2, _>(|i| libc::pollfd {
            fd: if open[i] { pipes[i].as_raw_fd() } else { -1 },
            events: libc::POLLIN,
            revents: 0,
        });
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, 50) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            log.error(error.to_string());
            break;
        }
        log.tick();
        for i in 0..2 {
            if !open[i] || fds[i].revents == 0 {
                continue;
            }
            match pipes[i].read(&mut buffer) {
                Ok(0) => open[i] = false,
                Ok(n) if remote && i == 0 => {
                    if invalid_frame {
                        continue;
                    }
                    framed.extend_from_slice(&buffer[..n]);
                    while framed.len() >= HEADER {
                        let header: &[u8; HEADER] = framed[..HEADER].try_into().unwrap();
                        let decoded = Record::length(header).and_then(|length| {
                            if framed.len() < HEADER + length {
                                return Ok(None);
                            }
                            Record::decode(header, framed[HEADER..HEADER + length].to_vec())
                                .map(|r| Some((r, HEADER + length)))
                        });
                        match decoded {
                            Ok(Some((record, length)))
                                if !remote_complete
                                    && matches!(
                                        record.source,
                                        Source::Stdout
                                            | Source::Stderr
                                            | Source::Diagnostic
                                            | Source::Gap
                                            | Source::End
                                    ) =>
                            {
                                if record.source == Source::End {
                                    remote_complete = true;
                                } else {
                                    log.remote(&record);
                                }
                                framed.drain(..length);
                            }
                            Ok(None) => break,
                            _ => {
                                log.error(invalid().to_string());
                                invalid_frame = true;
                                framed.clear();
                                break;
                            }
                        }
                    }
                }
                Ok(n) => log.write(
                    if i == 0 {
                        Source::Stdout
                    } else if remote {
                        Source::Diagnostic
                    } else {
                        Source::Stderr
                    },
                    &buffer[..n],
                ),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                    ) => {}
                Err(e) => {
                    log.error(e.to_string());
                    open[i] = false;
                }
            }
        }
    }
    if remote && (!framed.is_empty() || !remote_complete) {
        log.error(message("output recording is incomplete"));
    }
    log.finish()
}
