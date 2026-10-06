use std::collections::HashMap;
use std::fs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub pid: i32,
    pub parent: i32,
    pub threads: u64,
    pub start_ticks: u64,
    pub rss_pages: u64,
}

pub fn parse_stat(text: &str) -> Option<Stat> {
    let (head, rest) = text.rsplit_once(')')?;
    let pid = head.split_whitespace().next()?.parse().ok()?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some(Stat {
        pid,
        parent: fields.get(1)?.parse().ok()?,
        threads: fields.get(17)?.parse().ok()?,
        start_ticks: fields.get(19)?.parse().ok()?,
        rss_pages: fields.get(21)?.parse().ok()?,
    })
}

pub fn stat_of(pid: i32) -> Option<Stat> {
    parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

pub fn all() -> Vec<Stat> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter_map(stat_of)
        .collect()
}

pub fn descendants(root: i32, table: &[Stat]) -> Vec<Stat> {
    let mut children: HashMap<i32, Vec<Stat>> = HashMap::new();
    for stat in table {
        children.entry(stat.parent).or_default().push(*stat);
    }
    let mut found = Vec::new();
    let mut frontier = vec![root];
    while let Some(pid) = frontier.pop() {
        for child in children.get(&pid).into_iter().flatten() {
            found.push(*child);
            frontier.push(child.pid);
        }
    }
    found
}

pub fn page_bytes(pages: u64) -> u64 {
    pages * crate::cgroup::page_size()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(pid: i32, parent: i32) -> Stat {
        Stat {
            pid,
            parent,
            threads: 1,
            start_ticks: 0,
            rss_pages: 10,
        }
    }

    #[test]
    fn a_stat_line_with_spaces_in_the_name_parses() {
        let line = "4200 (my prog) S 4199 4200 4200 34816 4270 4194560 1234 0 0 0 1 2 0 0 20 0 3 0 5678 1000 250 18446744073709551615";
        let s = parse_stat(line).unwrap();
        assert_eq!(
            (s.pid, s.parent, s.threads, s.start_ticks, s.rss_pages),
            (4200, 4199, 3, 5678, 250)
        );
    }

    #[test]
    fn descendants_include_grandchildren_and_nothing_else() {
        let table = [
            stat(1, 0),
            stat(10, 1),
            stat(11, 10),
            stat(12, 11),
            stat(20, 1),
        ];
        let mut found: Vec<i32> = descendants(10, &table).iter().map(|s| s.pid).collect();
        found.sort();
        assert_eq!(found, vec![11, 12]);
    }
}
