use std::collections::BTreeMap;

use super::message;
use crate::model::{Job, State};
use crate::store::Store;

const KEPT: usize = 32;
const PLAIN: &str = "lost: its supervisor ended without writing a result";

fn file(store: &Store) -> std::path::PathBuf {
    store.root.join("boots.json")
}

fn known(store: &Store) -> BTreeMap<String, u64> {
    crate::store::read_json(&file(store)).unwrap_or_default()
}

fn began() -> Option<u64> {
    std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

pub fn note(store: &Store, boot_id: &str) {
    let Some(seconds) = began() else {
        return;
    };
    let mut boots = known(store);
    if boots.get(boot_id) == Some(&seconds) {
        return;
    }
    boots.insert(boot_id.to_owned(), seconds);
    while boots.len() > KEPT {
        let Some(oldest) = boots
            .iter()
            .min_by_key(|(_, seconds)| **seconds)
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        boots.remove(&oldest);
    }
    let _ = crate::store::write_json(&file(store), &boots);
}

fn utc(seconds: u64) -> String {
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

pub fn lost(store: &Store, job: &Job, boot_id: &str) -> String {
    let Some(before) = job
        .supervisor_boot_id
        .as_deref()
        .filter(|recorded| *recorded != boot_id)
    else {
        return PLAIN.to_owned();
    };
    let state = message(match job.state {
        State::Starting => "was starting",
        State::Suspended => "was suspended",
        State::Stopping => "was stopping",
        _ => "was running",
    });
    let boots = known(store);
    let line = match (boots.get(before), boots.get(boot_id).copied().or_else(began)) {
        (Some(then), Some(now)) => message(
            "lost: the host restarted while the Job {state}; that boot began at {then}, this one at {now}",
        )
        .replace("{then}", &utc(*then))
        .replace("{now}", &utc(now)),
        (None, Some(now)) => {
            message("lost: the host restarted while the Job {state}; this boot began at {now}")
                .replace("{now}", &utc(now))
        }
        _ => message("lost: the host restarted while the Job {state}"),
    };
    line.replace("{state}", &state)
}
