use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemInfo {
    pub total: u64,
    pub available: u64,
    pub disk_swap_total: u64,
    pub disk_swap_free: u64,
}

impl MemInfo {
    pub fn room_above_floor(&self) -> u64 {
        self.available.saturating_sub(host_memory_floor(self.total))
            + self
                .disk_swap_free
                .saturating_sub(host_memory_floor(self.disk_swap_total))
    }

    pub fn below_floor(&self) -> bool {
        self.total > 0
            && self.available < host_memory_floor(self.total)
            && self.disk_swap_free <= host_memory_floor(self.disk_swap_total)
    }
}

pub fn parse_swaps(text: &str) -> (u64, u64) {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let name = *fields.first()?;
            let size: u64 = fields.get(2)?.parse().ok()?;
            let used: u64 = fields.get(3)?.parse().ok()?;
            (!name.starts_with("/dev/zram"))
                .then_some((size * 1024, size.saturating_sub(used) * 1024))
        })
        .fold((0, 0), |(total, free), (size, left)| {
            (total + size, free + left)
        })
}

pub fn parse_meminfo(text: &str) -> MemInfo {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| {
                rest.trim()
                    .trim_end_matches("kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .map_or(0, |kib| kib * 1024)
    };
    MemInfo {
        total: field("MemTotal:"),
        available: field("MemAvailable:"),
        ..MemInfo::default()
    }
}

pub fn read_meminfo() -> MemInfo {
    let mut info = fs::read_to_string("/proc/meminfo")
        .map(|t| parse_meminfo(&t))
        .unwrap_or_default();
    let (total, free) = fs::read_to_string("/proc/swaps")
        .map(|t| parse_swaps(&t))
        .unwrap_or_default();
    info.disk_swap_total = total;
    info.disk_swap_free = free;
    info
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pressure {
    pub some_avg10: f64,
    pub full_avg10: f64,
}

pub fn parse_pressure(text: &str) -> Pressure {
    let avg10 = |kind: &str| {
        text.lines()
            .filter(|line| line.starts_with(kind))
            .flat_map(|line| line.split_whitespace())
            .find_map(|field| field.strip_prefix("avg10="))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    Pressure {
        some_avg10: avg10("some"),
        full_avg10: avg10("full"),
    }
}

pub fn read_memory_pressure() -> Pressure {
    fs::read_to_string("/proc/pressure/memory")
        .map(|t| parse_pressure(&t))
        .unwrap_or_default()
}

pub fn user_namespaces() -> bool {
    let read = |path: &str| {
        fs::read_to_string(path)
            .ok()
            .and_then(|t| t.trim().parse::<u64>().ok())
    };
    read("/proc/sys/user/max_user_namespaces").is_some_and(|n| n > 0)
        && read("/proc/sys/kernel/unprivileged_userns_clone").is_none_or(|n| n == 1)
        && read("/proc/sys/kernel/apparmor_restrict_unprivileged_userns").is_none_or(|n| n == 0)
}

fn kernel_value(name: &str) -> String {
    fs::read_to_string(format!("/proc/sys/kernel/{name}"))
        .map(|t| t.trim().to_string())
        .unwrap_or_default()
}

pub fn hostname() -> String {
    kernel_value("hostname")
}

pub fn kernel_release() -> String {
    kernel_value("osrelease")
}

pub fn cores() -> u64 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u64)
}

pub fn pid_max() -> u64 {
    fs::read_to_string("/proc/sys/kernel/pid_max")
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(32768)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub device: (u32, u32),
    pub mount_point: PathBuf,
    pub fs_type: String,
    pub source: String,
}

pub fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let left: Vec<&str> = left.split_whitespace().collect();
            let right: Vec<&str> = right.split_whitespace().collect();
            let (major, minor) = left.get(2)?.split_once(':')?;
            Some(Mount {
                device: (major.parse().ok()?, minor.parse().ok()?),
                mount_point: PathBuf::from(unescape_mount_path(left.get(4)?)),
                fs_type: right.first()?.to_string(),
                source: right.get(1)?.to_string(),
            })
        })
        .collect()
}

fn unescape_mount_path(path: &str) -> String {
    path.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

pub fn mount_for(path: &Path, mounts: Vec<Mount>) -> Option<Mount> {
    mounts
        .into_iter()
        .filter(|m| path.starts_with(&m.mount_point))
        .max_by_key(|m| m.mount_point.as_os_str().len())
}

pub fn mount_of(path: &Path) -> Option<Mount> {
    let path = fs::canonicalize(path).ok()?;
    mount_for(
        &path,
        parse_mountinfo(&fs::read_to_string("/proc/self/mountinfo").ok()?),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    pub size: u64,
    pub free: u64,
}

pub fn statvfs_space(path: &Path) -> Option<Space> {
    let c_path = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let block = stat.f_frsize as u64;
    Some(Space {
        size: stat.f_blocks as u64 * block,
        free: stat.f_bavail as u64 * block,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BtrfsAllocation {
    pub device_size: u64,
    pub data_total: u64,
    pub data_used: u64,
    pub disk_allocated: u64,
    pub largest_profile_ratio: u64,
}

pub fn btrfs_min_free(a: BtrfsAllocation) -> u64 {
    let unallocated = a.device_size.saturating_sub(a.disk_allocated);
    a.data_total.saturating_sub(a.data_used) + unallocated / a.largest_profile_ratio.max(1)
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub fn read_btrfs_allocation(source: &str) -> Option<BtrfsAllocation> {
    let device_name = Path::new(source).file_name()?.to_owned();
    for entry in fs::read_dir("/sys/fs/btrfs").ok()?.flatten() {
        let fs_dir = entry.path();
        let device_dir = fs_dir.join("devices").join(&device_name);
        if !device_dir.exists() {
            continue;
        }
        let device_size = read_u64(&device_dir.join("size"))? * 512;
        let allocation = fs_dir.join("allocation");
        let mut disk_allocated = 0;
        let mut largest_profile_ratio = 1;
        for kind in ["data", "metadata", "system"] {
            let total = read_u64(&allocation.join(kind).join("total_bytes"))?;
            let disk_total = read_u64(&allocation.join(kind).join("disk_total"))?;
            disk_allocated += disk_total;
            if let Some(ratio) = disk_total.checked_div(total) {
                largest_profile_ratio = largest_profile_ratio.max(ratio);
            }
        }
        return Some(BtrfsAllocation {
            device_size,
            data_total: read_u64(&allocation.join("data").join("total_bytes"))?,
            data_used: read_u64(&allocation.join("data").join("bytes_used"))?,
            disk_allocated,
            largest_profile_ratio,
        });
    }
    None
}

pub fn space_of(path: &Path) -> Option<(Mount, Space)> {
    let mount = mount_of(path)?;
    let mut space = statvfs_space(&mount.mount_point)?;
    if mount.fs_type == "btrfs"
        && let Some(allocation) = read_btrfs_allocation(&mount.source)
    {
        space.free = space.free.min(btrfs_min_free(allocation));
    }
    Some((mount, space))
}

pub fn filesystem_floor(size: u64) -> u64 {
    const FLOOR_SHARE_PERCENT: u64 = 5;
    size / 100 * FLOOR_SHARE_PERCENT
}

pub fn host_memory_floor(total: u64) -> u64 {
    const HOST_FLOOR_PERCENT: u64 = 12;
    total / 100 * HOST_FLOOR_PERCENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_gives_total_and_available_in_bytes() {
        let text = "MemTotal:       49308784 kB\nMemFree:  1 kB\nMemAvailable:   45893988 kB\n";
        assert_eq!(
            parse_meminfo(text),
            MemInfo {
                total: 49308784 * 1024,
                available: 45893988 * 1024,
                ..MemInfo::default()
            }
        );
    }

    #[test]
    fn pressure_gives_some_and_full_avg10() {
        let text = "some avg10=1.50 avg60=0.00 avg300=0.00 total=201\nfull avg10=0.25 avg60=0.00 avg300=0.00 total=199\n";
        assert_eq!(
            parse_pressure(text),
            Pressure {
                some_avg10: 1.5,
                full_avg10: 0.25
            }
        );
    }

    #[test]
    fn mountinfo_lines_give_device_mount_point_and_type() {
        let text = "31 2 0:28 /@ / rw,noatime - btrfs /dev/vda2 rw,compress=zstd:1\n41 29 0:38 / /run/user/1000 rw - tmpfs tmpfs rw\n";
        let mounts = parse_mountinfo(text);
        assert_eq!(mounts[0].device, (0, 28));
        assert_eq!(mounts[0].mount_point, PathBuf::from("/"));
        assert_eq!(mounts[0].fs_type, "btrfs");
        assert_eq!(mounts[0].source, "/dev/vda2");
        assert_eq!(mounts[1].mount_point, PathBuf::from("/run/user/1000"));
    }

    #[test]
    fn a_path_belongs_to_its_longest_mount_point() {
        let text = "31 2 0:28 /@ / rw - btrfs /dev/vda2 rw\n41 29 0:38 / /run/user/1000 rw - tmpfs tmpfs rw\n";
        let mount = mount_for(Path::new("/run/user/1000/x"), parse_mountinfo(text)).unwrap();
        assert_eq!(mount.fs_type, "tmpfs");
        let mount = mount_for(Path::new("/home/agent"), parse_mountinfo(text)).unwrap();
        assert_eq!(mount.fs_type, "btrfs");
    }

    #[test]
    fn btrfs_free_counts_unallocated_space_at_the_most_wasteful_profile() {
        let allocation = BtrfsAllocation {
            device_size: 136_901_013_504,
            data_total: 73_532_375_040,
            data_used: 65_188_401_152,
            disk_allocated: 73_532_375_040 + 6_442_450_944 + 16_777_216,
            largest_profile_ratio: 2,
        };
        assert_eq!(
            btrfs_min_free(allocation),
            8_343_973_888 + 56_909_410_304 / 2
        );
    }

    #[test]
    fn the_filesystem_floor_is_five_percent_of_its_size() {
        assert_eq!(filesystem_floor(136_901_013_504), 1_369_010_135 * 5);
        assert_eq!(filesystem_floor(10 << 20), (10 << 20) / 100 * 5);
    }

    #[test]
    fn the_host_floor_is_twelve_percent_of_memory() {
        assert_eq!(host_memory_floor(100 << 20), (100 << 20) / 100 * 12);
    }

    #[test]
    fn swap_on_disk_counts_and_zram_does_not() {
        let text = "Filename Type Size Used Priority\n/dev/zram0 partition 1000 400 100\n/dev/dm-1 partition 2000 500 -1\n";
        assert_eq!(parse_swaps(text), (2000 * 1024, 1500 * 1024));
    }

    #[test]
    fn a_host_is_short_only_when_memory_and_disk_swap_are_both_below_their_floors() {
        let gib = 1u64 << 30;
        let desktop = MemInfo {
            total: 64 * gib,
            available: 7 * gib,
            disk_swap_total: 72 * gib,
            disk_swap_free: 40 * gib,
        };
        assert!(!desktop.below_floor());
        assert!(desktop.room_above_floor() > 30 * gib);
        let without_swap = MemInfo {
            disk_swap_total: 0,
            disk_swap_free: 0,
            ..desktop
        };
        assert!(without_swap.below_floor());
        assert_eq!(without_swap.room_above_floor(), 0);
    }
}
