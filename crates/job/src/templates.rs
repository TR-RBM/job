use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::filter::mask;
use crate::store::Store;

pub const SUCCESSFUL_RUNS: usize = 3;

pub fn key_hash(key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn dir(store: &Store, key: &str) -> PathBuf {
    store.root.join("templates").join(key_hash(key))
}

pub fn remember(store: &Store, key: &str, id: u64, lines: &[String]) {
    let dir = dir(store, key);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let unique: HashSet<String> = lines.iter().map(|l| mask(l)).collect();
    let mut text: Vec<String> = unique.into_iter().collect();
    text.sort();
    let _ = fs::write(dir.join(format!("{id}.txt")), text.join("\n"));
    let mut ids: Vec<u64> = fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().to_str()?.strip_suffix(".txt")?.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    ids.sort_unstable();
    let excess = ids.len().saturating_sub(SUCCESSFUL_RUNS);
    for old in &ids[..excess] {
        let _ = fs::remove_file(dir.join(format!("{old}.txt")));
    }
}

pub fn known(store: &Store, key: &str) -> HashSet<String> {
    let Ok(entries) = fs::read_dir(dir(store, key)) else {
        return HashSet::new();
    };
    entries
        .flatten()
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .flat_map(|text| text.lines().map(str::to_string).collect::<Vec<_>>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_three_successful_runs_are_remembered() {
        let root = std::env::temp_dir().join(format!("job-templates-test-{}", std::process::id()));
        let store = Store::open(root).unwrap();
        for id in 1..=4u64 {
            remember(
                &store,
                "make @ /repo",
                id,
                &[format!("line only in run {}", "x".repeat(id as usize))],
            );
        }
        let known = known(&store, "make @ /repo");
        assert!(!known.contains("line only in run x"));
        assert!(known.contains("line only in run xxxx"));
        assert_eq!(known.len(), 3);
    }
}
