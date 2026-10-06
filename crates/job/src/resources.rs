use serde::{Deserialize, Serialize};

use crate::units::format_bytes;

pub const MILLI: u64 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vector {
    pub cores_milli: u64,
    pub memory: u64,
    pub pids: u64,
}

impl Vector {
    pub fn add(self, other: Vector) -> Vector {
        Vector {
            cores_milli: self.cores_milli.saturating_add(other.cores_milli),
            memory: self.memory.saturating_add(other.memory),
            pids: self.pids.saturating_add(other.pids),
        }
    }

    pub fn fits_within(self, capacity: Vector) -> bool {
        self.cores_milli <= capacity.cores_milli
            && self.memory <= capacity.memory
            && self.pids <= capacity.pids
    }

    pub fn first_excess(self, capacity: Vector) -> Option<String> {
        if self.cores_milli > capacity.cores_milli {
            return Some(format!(
                "{} cores, the pool has {}",
                format_cores(self.cores_milli),
                format_cores(capacity.cores_milli)
            ));
        }
        if self.memory > capacity.memory {
            return Some(format!(
                "{} memory, the pool has {}",
                format_bytes(self.memory),
                format_bytes(capacity.memory)
            ));
        }
        if self.pids > capacity.pids {
            return Some(format!(
                "{} processes, the pool has {}",
                self.pids, capacity.pids
            ));
        }
        None
    }
}

pub fn format_cores(cores_milli: u64) -> String {
    if cores_milli.is_multiple_of(MILLI) {
        let cores = cores_milli / MILLI;
        if cores == 1 {
            "1 core".to_string()
        } else {
            format!("{cores} cores")
        }
    } else {
        format!("{:.2} cores", cores_milli as f64 / MILLI as f64)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Unset,
    Declared,
    History { runs: usize },
    RepositoryHistory { runs: usize },
    Rule { tool: String },
    Default,
    AfterStop { limit: u64 },
}

impl Source {
    pub fn describe(&self) -> String {
        match self {
            Source::Unset => "not requested".to_string(),
            Source::Declared => "declared".to_string(),
            Source::History { runs } => format!("from {runs} earlier runs"),
            Source::RepositoryHistory { runs } => {
                format!("from {runs} runs of other cargo commands in this repository")
            }
            Source::Rule { tool } => format!("the rule for {tool}"),
            Source::Default => "the default for an unknown command".to_string(),
            Source::AfterStop { limit } => {
                format!("twice the {} it was stopped at", format_bytes(*limit))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservation {
    pub vector: Vector,
    pub disk: u64,
    pub devices: Vec<String>,
    pub cores_source: Source,
    pub memory_source: Source,
    pub disk_source: Source,
    pub predicted_ms: Option<u64>,
    pub wall_limit_ms: Option<u64>,
}

impl Reservation {
    pub fn describe(&self) -> String {
        let mut parts = vec![
            format!(
                "{} ({})",
                format_cores(self.vector.cores_milli),
                self.cores_source.describe()
            ),
            format!(
                "{} memory ({})",
                format_bytes(self.vector.memory),
                self.memory_source.describe()
            ),
            format!("{} processes", self.vector.pids),
        ];
        if self.disk_source == Source::Declared {
            parts.push(format!("{} disk (declared)", format_bytes(self.disk)));
        }
        for device in &self.devices {
            parts.push(format!("{device} alone"));
        }
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector(cores: u64, memory: u64, pids: u64) -> Vector {
        Vector {
            cores_milli: cores * MILLI,
            memory,
            pids,
        }
    }

    #[test]
    fn a_vector_fits_only_when_every_component_fits() {
        let capacity = vector(4, 100, 10);
        assert!(vector(4, 100, 10).fits_within(capacity));
        assert!(!vector(5, 1, 1).fits_within(capacity));
        assert!(!vector(1, 101, 1).fits_within(capacity));
        assert!(!vector(1, 1, 11).fits_within(capacity));
    }

    #[test]
    fn the_first_excess_names_the_resource() {
        let capacity = vector(4, 1 << 30, 10);
        assert_eq!(
            vector(1, 2 << 30, 1).first_excess(capacity),
            Some("2.00 GiB memory, the pool has 1.00 GiB".to_string())
        );
        assert_eq!(vector(1, 1, 1).first_excess(capacity), None);
    }

    #[test]
    fn cores_read_as_whole_or_fractional() {
        assert_eq!(format_cores(1000), "1 core");
        assert_eq!(format_cores(4000), "4 cores");
        assert_eq!(format_cores(1500), "1.50 cores");
    }
}
