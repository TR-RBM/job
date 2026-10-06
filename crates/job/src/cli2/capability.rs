use serde::{Deserialize, Serialize};

use super::message;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub supported: bool,
    pub reason: String,
}

pub fn filesystem_quota() -> Feature {
    Feature {
        supported: false,
        reason: message(
            "the service sets no file system quota; --write-budget counts the bytes a Job writes and stops the Job when it passes the budget",
        ),
    }
}
