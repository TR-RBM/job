use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const HEAD_LIMIT: u64 = 32 << 20;
pub const TAIL_LIMIT: u64 = 32 << 20;

pub struct CappedLog {
    head: File,
    head_written: u64,
    head_limit: u64,
    segment_limit: u64,
    segments: [PathBuf; 2],
    current: usize,
    segment: Option<File>,
    segment_written: u64,
    dropped: u64,
    rotated: bool,
}

impl CappedLog {
    pub fn create(path: &Path) -> io::Result<CappedLog> {
        CappedLog::with_limits(path, HEAD_LIMIT, TAIL_LIMIT)
    }

    pub fn with_limits(path: &Path, head_limit: u64, tail_limit: u64) -> io::Result<CappedLog> {
        let segment_path = |n: u8| path.with_extension(format!("tail{n}"));
        Ok(CappedLog {
            head: File::create(path)?,
            head_written: 0,
            head_limit,
            segment_limit: tail_limit / 2,
            segments: [segment_path(0), segment_path(1)],
            current: 0,
            segment: None,
            segment_written: 0,
            dropped: 0,
            rotated: false,
        })
    }

    pub fn write_all(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        if self.head_written < self.head_limit {
            let take = bytes
                .len()
                .min((self.head_limit - self.head_written) as usize);
            self.head.write_all(&bytes[..take])?;
            self.head_written += take as u64;
            bytes = &bytes[take..];
        }
        while !bytes.is_empty() {
            if self.segment.is_none() || self.segment_written >= self.segment_limit {
                self.rotate()?;
            }
            let room = (self.segment_limit - self.segment_written) as usize;
            let take = bytes.len().min(room.max(1));
            if let Some(segment) = self.segment.as_mut() {
                segment.write_all(&bytes[..take])?;
            }
            self.segment_written += take as u64;
            bytes = &bytes[take..];
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        if self.segment.is_some() {
            self.current = 1 - self.current;
            self.rotated = true;
        }
        let path = &self.segments[self.current];
        if self.rotated {
            self.dropped += fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        }
        self.segment = Some(File::create(path)?);
        self.segment_written = 0;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<u64> {
        let total = self.head_written + self.dropped + self.tail_len();
        if self.segment.is_some() {
            if self.dropped > 0 {
                writeln!(
                    self.head,
                    "\n[job: {} bytes of output were not kept; the first {} and the last {} are]",
                    self.dropped,
                    self.head_written,
                    self.tail_len()
                )?;
            }
            let order = if self.rotated {
                [1 - self.current, self.current]
            } else {
                [self.current, self.current]
            };
            let mut seen = None;
            for index in order {
                if seen == Some(index) {
                    continue;
                }
                seen = Some(index);
                if let Ok(mut segment) = File::open(&self.segments[index]) {
                    io::copy(&mut segment, &mut self.head)?;
                }
                let _ = fs::remove_file(&self.segments[index]);
            }
        }
        self.head.flush()?;
        Ok(total)
    }

    fn tail_len(&self) -> u64 {
        self.segments
            .iter()
            .map(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0))
            .sum()
    }
}

pub fn read_lines(path: &Path) -> io::Result<Vec<String>> {
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(path)?
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("job-log-test-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("output.log")
    }

    #[test]
    fn a_short_output_is_kept_whole() {
        let path = temp("short");
        let mut log = CappedLog::with_limits(&path, 100, 40).unwrap();
        log.write_all(b"hello\n").unwrap();
        assert_eq!(log.finish().unwrap(), 6);
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
    }

    #[test]
    fn a_long_output_keeps_its_head_and_its_tail_and_says_what_was_dropped() {
        let path = temp("long");
        let mut log = CappedLog::with_limits(&path, 10, 20).unwrap();
        for i in 0..100u8 {
            log.write_all(&[b'a' + i % 26]).unwrap();
        }
        assert_eq!(log.finish().unwrap(), 100);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("abcdefghij"));
        assert!(text.contains("bytes of output were not kept"));
        let expected_tail: String = (80..100u8).map(|i| (b'a' + i % 26) as char).collect();
        assert!(text.ends_with(&expected_tail), "{text}");
        assert!(!path.with_extension("tail0").exists());
    }

    #[test]
    fn the_whole_log_never_exceeds_head_and_tail() {
        let path = temp("bounded");
        let mut log = CappedLog::with_limits(&path, 1000, 2000).unwrap();
        let chunk = vec![b'x'; 777];
        for _ in 0..100 {
            log.write_all(&chunk).unwrap();
        }
        log.finish().unwrap();
        let kept = fs::read(&path)
            .unwrap()
            .iter()
            .filter(|&&b| b == b'x')
            .count();
        assert!(kept <= 3000, "{kept}");
        assert!(kept >= 2000, "{kept}");
    }
}
