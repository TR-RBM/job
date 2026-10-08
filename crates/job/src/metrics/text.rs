use std::fmt::Display;
use std::fmt::Write;

pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

pub enum Kind {
    Gauge,
    Counter,
    Histogram,
}

impl Kind {
    fn word(&self) -> &'static str {
        match self {
            Kind::Gauge => "gauge",
            Kind::Counter => "counter",
            Kind::Histogram => "histogram",
        }
    }
}

#[derive(Default)]
pub struct Exposition {
    out: String,
}

fn escaped_help(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\n', "\\n")
}

fn escaped_value(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

impl Exposition {
    pub fn family(&mut self, name: &str, kind: Kind, help: &str) {
        let _ = writeln!(self.out, "# HELP {name} {}", escaped_help(help));
        let _ = writeln!(self.out, "# TYPE {name} {}", kind.word());
    }

    pub fn sample(&mut self, name: &str, labels: &[(&str, &str)], value: impl Display) {
        self.out.push_str(name);
        if !labels.is_empty() {
            self.out.push('{');
            for (at, (label, text)) in labels.iter().enumerate() {
                if at > 0 {
                    self.out.push(',');
                }
                let _ = write!(self.out, "{label}=\"{}\"", escaped_value(text));
            }
            self.out.push('}');
        }
        let _ = writeln!(self.out, " {value}");
    }

    pub fn text(self) -> String {
        self.out
    }
}

pub fn seconds(ms: u64) -> f64 {
    ms as f64 / 1000.0
}

pub fn cores(milli: u64) -> f64 {
    milli as f64 / 1000.0
}

pub fn flag(value: bool) -> u8 {
    u8::from(value)
}
