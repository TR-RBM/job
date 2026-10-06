use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::procs;

pub const TOOLS: [&str; 7] = [
    "unshare",
    "nsenter",
    "sleep",
    "ip",
    "tc",
    "nft",
    "slirp4netns",
];
const OUTER_CIDR: &str = "198.18.0.0/24";
const OUTER_RESOLVER: &str = "198.18.0.3";
pub const HOST_LOOPBACK: std::net::Ipv4Addr = std::net::Ipv4Addr::new(198, 18, 0, 2);
const BRIDGE_ADDRESS: &str = "198.19.0.1/24";
const JOB_PREFIX: &str = "198.19.0.";
pub const FIRST_INDEX: u32 = 2;
pub const LAST_INDEX: u32 = 254;
const PACKET: u32 = 1500;
pub const HOST_GATE: &str = "iifname \"br0\" accept;";
pub const TUNNEL_GATE: &str = "iifname \"br0\" oifname \"wg0\" accept;";
const SETTLE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Process {
    pub pid: i32,
    pub start_ticks: u64,
    #[serde(default)]
    pub boot_id: Option<[u8; 16]>,
}

impl Process {
    fn of(pid: i32) -> Option<Process> {
        let boot_id = Some(crate::process::boot_token().ok()?);
        procs::stat_of(pid).map(|s| Process {
            pid,
            start_ticks: s.start_ticks,
            boot_id,
        })
    }

    pub fn alive(self) -> bool {
        self.boot_id.is_some()
            && self.boot_id == crate::process::boot_token().ok()
            && crate::process::Handle::open(self.pid, Some(self.start_ticks))
                .is_ok_and(|handle| handle.alive().unwrap_or(false))
    }

    fn end(self) {
        if self.alive()
            && let Ok(handle) = crate::process::Handle::open(self.pid, Some(self.start_ticks))
        {
            let _ = handle.signal(libc::SIGKILL);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub name: String,
    pub holder: Process,
    pub relay: Process,
    pub rate: Option<u64>,
    #[serde(default)]
    pub egress: Option<crate::model::Net>,
}

impl Link {
    fn exit(&self) -> &'static str {
        match self.egress {
            Some(crate::model::Net::WireGuard(_)) => "wg0",
            Some(crate::model::Net::OpenVpn(_)) => "tun0",
            _ => "tap0",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyTarget {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

pub fn proxy_target(url: &str) -> Result<ProxyTarget, String> {
    let shown = crate::netsecret::split(url).0;
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("`{shown}` is not a proxy address"))?;
    let rest = rest.trim_end_matches('/');
    let place = rest.rsplit_once('@').map_or(rest, |(_, place)| place);
    let (host, port) = place
        .rsplit_once(':')
        .ok_or_else(|| format!("the proxy `{shown}` names no port; write {scheme}://HOST:PORT"))?;
    let port = port
        .parse()
        .map_err(|_| format!("the proxy `{shown}` has no port number"))?;
    Ok(ProxyTarget {
        scheme: scheme.to_string(),
        host: host.trim_matches(['[', ']']).to_string(),
        port,
    })
}

fn proxy_address(target: &ProxyTarget) -> io::Result<std::net::Ipv4Addr> {
    use std::net::ToSocketAddrs;
    let address = (target.host.as_str(), target.port)
        .to_socket_addrs()?
        .find_map(|a| match a {
            std::net::SocketAddr::V4(v4) => Some(*v4.ip()),
            std::net::SocketAddr::V6(_) => None,
        })
        .ok_or_else(|| {
            io::Error::other(format!("the proxy {} has no IPv4 address", target.host))
        })?;
    Ok(if address.is_loopback() {
        HOST_LOOPBACK
    } else {
        address
    })
}

pub fn job_view(link: &Link) -> io::Result<(Vec<(String, String)>, String)> {
    match &link.egress {
        Some(crate::model::Net::Proxy(url)) => {
            let target = proxy_target(url).map_err(io::Error::other)?;
            let address = proxy_address(&target)?;
            let scheme = if target.scheme == "socks5" {
                "socks5h"
            } else {
                target.scheme.as_str()
            };
            let seen = format!("{scheme}://{address}:{}", target.port);
            let mut env = vec![
                ("ALL_PROXY".to_string(), seen.clone()),
                ("all_proxy".to_string(), seen.clone()),
            ];
            if scheme.starts_with("http") {
                for name in ["http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY"] {
                    env.push((name.to_string(), seen.clone()));
                }
            }
            Ok((env, "nameserver 127.0.0.1\n".to_string()))
        }
        Some(crate::model::Net::WireGuard(path)) => {
            let config = crate::wg::read(path).map_err(io::Error::other)?;
            let resolver: String = config
                .dns
                .iter()
                .map(|d| format!("nameserver {d}\n"))
                .collect();
            Ok((Vec::new(), resolver))
        }
        _ => Ok((Vec::new(), format!("nameserver {OUTER_RESOLVER}\n"))),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLink {
    pub holder: Process,
    pub index: u32,
}

pub fn missing_tools() -> Vec<&'static str> {
    TOOLS
        .iter()
        .copied()
        .filter(|tool| !on_path(tool))
        .collect()
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(tool).is_file()))
}

fn run(command: &mut Command) -> io::Result<()> {
    let output = command.stdin(Stdio::null()).output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{:?} failed: {}",
            command.get_program(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn inside(holder: Process, script: &str) -> io::Result<()> {
    inside_with(holder, script, None)
}

fn inside_with(holder: Process, script: &str, argument: Option<&Path>) -> io::Result<()> {
    run(Command::new("nsenter")
        .args([
            "-t",
            &holder.pid.to_string(),
            "-U",
            "-n",
            "--preserve-credentials",
        ])
        .args(["--", "sh", "-e", "-c", script, "sh"])
        .args(argument))
}

fn spawn_holder(command: &mut Command) -> io::Result<Process> {
    let own = fs::read_link("/proc/self/ns/net")?;
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let pid = child.id() as i32;
    std::thread::spawn(move || {
        let mut child = child;
        child.wait()
    });
    let deadline = Instant::now() + SETTLE;
    while Instant::now() < deadline {
        let net = fs::read_link(format!("/proc/{pid}/ns/net"));
        if net.as_ref().is_ok_and(|n| *n != own)
            && fs::read_link(format!("/proc/{pid}/exe"))
                .is_ok_and(|exe| exe.file_name().is_some_and(|n| n == "sleep"))
        {
            return Process::of(pid).ok_or_else(|| io::Error::other("the holder ended"));
        }
        std::thread::sleep(POLL);
    }
    Err(io::Error::other(
        "a network namespace did not appear within 5 s",
    ))
}

fn job_address(index: u32) -> String {
    format!("{JOB_PREFIX}{index}")
}

fn class(index: u32) -> String {
    format!("1:{:x}", index + 0x100)
}

fn own_image() -> io::Result<i32> {
    use std::os::unix::io::IntoRawFd;
    static IMAGE: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    if let Some(fd) = IMAGE.get() {
        return Ok(*fd);
    }
    let fd = fs::File::open("/proc/self/exe")?.into_raw_fd();
    unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
    Ok(*IMAGE.get_or_init(|| fd))
}

fn egress_script(link: &Link) -> io::Result<(String, String)> {
    match &link.egress {
        Some(crate::model::Net::Proxy(url)) => {
            let target = proxy_target(url).map_err(io::Error::other)?;
            let address = proxy_address(&target)?;
            Ok((
                format!(
                    "iifname \"br0\" ip daddr {address} tcp dport {} accept;",
                    target.port
                ),
                String::new(),
            ))
        }
        Some(crate::model::Net::WireGuard(path)) => {
            let config = crate::wg::read(path).map_err(io::Error::other)?;
            let mut endpoints = Vec::new();
            for peer in &config.peers {
                endpoints.push(match &peer.endpoint {
                    Some(e) => Some(crate::wg::resolve_endpoint(e).map_err(io::Error::other)?),
                    None => None,
                });
            }
            let helper = format!("/proc/self/fd/{}", own_image()?);
            let listed: Vec<String> = endpoints
                .iter()
                .map(|e| e.map_or_else(|| "-".to_string(), |e| e.to_string()))
                .collect();
            let mut script = format!(
                "ip link add wg0 type wireguard\n{helper} link-wireguard wg0 \"$1\" {}\n",
                listed.join(" ")
            );
            for (address, prefix) in &config.addresses {
                script.push_str(&format!("ip addr add {address}/{prefix} dev wg0\n"));
            }
            script.push_str(&format!(
                "ip link set wg0 mtu 1420 gso_max_size {PACKET} gro_max_size {PACKET} up\n"
            ));
            for endpoint in endpoints.iter().flatten() {
                script.push_str(&format!(
                    "ip route add {}/32 via {HOST_LOOPBACK} dev tap0\n",
                    endpoint.ip()
                ));
            }
            script.push_str("ip route replace default dev wg0\n");
            Ok((TUNNEL_GATE.to_string(), script))
        }
        Some(crate::model::Net::OpenVpn(_)) => Err(io::Error::other(
            "openvpn: not yet; use wireguard:FILE or a proxy",
        )),
        _ => Ok((HOST_GATE.to_string(), String::new())),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub deny: Vec<String>,
    pub allow: Vec<String>,
}

pub fn gate(base: &str, filter: &Filter) -> String {
    let mut rules = String::new();
    for matcher in &filter.deny {
        rules.push_str(&format!(
            "iifname \"br0\" {matcher} reject with icmp type admin-prohibited; "
        ));
    }
    if filter.allow.is_empty() {
        rules.push_str(base);
    }
    for matcher in &filter.allow {
        rules.push_str(&base.replace(" accept;", &format!(" {matcher} accept; ")));
    }
    rules
}

pub fn create(
    name: &str,
    rate: Option<u64>,
    egress: &crate::model::Net,
    log: &Path,
    filter: &Filter,
) -> io::Result<Link> {
    let holder = spawn_holder(Command::new("unshare").args([
        "--user",
        "--map-root-user",
        "--net",
        "sleep",
        "infinity",
    ]))?;
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir)?;
    }
    let relay_log = fs::File::create(log)?;
    let child = Command::new("slirp4netns")
        .args(["--configure", "--mtu=1500"])
        .arg(format!("--cidr={OUTER_CIDR}"))
        .arg(format!("--userns-path=/proc/{}/ns/user", holder.pid))
        .arg(holder.pid.to_string())
        .arg("tap0")
        .stdin(Stdio::null())
        .stdout(relay_log.try_clone()?)
        .stderr(relay_log)
        .process_group(0)
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(e) => {
            holder.end();
            return Err(e);
        }
    };
    let relay = Process::of(child.id() as i32);
    std::thread::spawn(move || {
        let mut child = child;
        child.wait()
    });
    let Some(relay) = relay else {
        holder.end();
        return Err(io::Error::other("slirp4netns ended at once"));
    };
    let link = Link {
        name: name.to_string(),
        holder,
        relay,
        rate,
        egress: Some(egress.clone()),
    };
    let exit = link.exit();
    let (gate, egress_setup) = match egress_script(&link) {
        Ok((base, setup)) => (gate(&base, filter), setup),
        Err(e) => {
            destroy(&link);
            return Err(e);
        }
    };
    let shaping = match rate {
        Some(rate) => format!(
            "for dev in br0 {exit}; do
  tc qdisc replace dev $dev root handle 1: htb
  tc class replace dev $dev parent 1: classid 1:1 htb rate {rate}bit ceil {rate}bit
done"
        ),
        None => String::new(),
    };
    let deadline = Instant::now() + SETTLE;
    let mut configured = false;
    while !configured && Instant::now() < deadline {
        configured = inside(holder, "ip -4 addr show dev tap0 | grep -q inet").is_ok();
        if !configured {
            std::thread::sleep(POLL);
        }
    }
    let script = format!(
        "sysctl -qw net.ipv4.ip_forward=1
ip link set lo up
ip link set tap0 gro_max_size {PACKET} gso_max_size {PACKET}
ip link add br0 type bridge
ip addr add {BRIDGE_ADDRESS} dev br0
ip link set br0 up
{egress_setup}nft -f - <<'RULES'
table ip job {{
  map marks {{ type ipv4_addr : mark; }}
  chain up {{ type filter hook prerouting priority -150; meta mark set ip saddr map @marks; }}
  chain down {{ type filter hook forward priority -150; oifname \"br0\" meta mark set ip daddr map @marks; }}
  chain clamp {{ type filter hook forward priority -140; tcp flags syn tcp option maxseg size set rt mtu; }}
  chain gate {{ type filter hook forward priority 0; policy drop; ct state established,related accept; {gate} iifname \"br0\" reject with icmp type admin-prohibited; }}
  chain out {{ type nat hook postrouting priority 100; oifname \"{exit}\" masquerade; }}
}}
RULES
{shaping}"
    );
    if !configured {
        destroy(&link);
        return Err(io::Error::other(
            "slirp4netns did not configure its interface within 5 s",
        ));
    }
    let configuration = match egress {
        crate::model::Net::WireGuard(path) => Some(path.as_path()),
        _ => None,
    };
    if let Err(e) = inside_with(holder, &script, configuration) {
        destroy(&link);
        return Err(e);
    }
    Ok(link)
}

pub fn alive(link: &Link) -> bool {
    link.holder.alive() && link.relay.alive()
}

pub fn attach(link: &Link, index: u32) -> io::Result<JobLink> {
    let holder = spawn_holder(
        Command::new("nsenter")
            .args([
                "-t",
                &link.holder.pid.to_string(),
                "-U",
                "--preserve-credentials",
                "--",
            ])
            .args(["unshare", "--net", "sleep", "infinity"]),
    )?;
    let job = JobLink { holder, index };
    let address = job_address(index);
    let outer = format!(
        "ip link add q{index} type veth peer name j{index}
ip link set j{index} netns {pid}
ip link set q{index} master br0
ip link set q{index} type bridge_slave isolated on
ip link set q{index} up
nft add element ip job marks {{ {address} : {index} }}
{classes}",
        pid = holder.pid,
        classes = match link.rate {
            Some(rate) => format!(
                "for dev in br0 {exit}; do
  tc class replace dev $dev parent 1:1 classid {class} htb rate 8kbit ceil {rate}bit
  tc qdisc replace dev $dev parent {class} sfq
  tc filter replace dev $dev parent 1: protocol ip prio 1 handle {index} fw flowid {class}
done",
                exit = link.exit(),
                class = class(index),
            ),
            None => String::new(),
        },
    );
    let inner = format!(
        "ip link set lo up
ip link set j{index} name eth0
ip link set eth0 gso_max_size {PACKET} gro_max_size {PACKET}
ip addr add {address}/24 dev eth0
ip link set eth0 up
ip route add default via {bridge}",
        bridge = BRIDGE_ADDRESS.trim_end_matches("/24"),
    );
    if let Err(e) = inside(link.holder, &outer).and_then(|_| inside(holder, &inner)) {
        detach(link, &job);
        return Err(e);
    }
    Ok(job)
}

pub fn isolate_ports(link: &Link) -> io::Result<()> {
    inside(
        link.holder,
        "ip -o link show master br0 | while IFS=': ' read -r number port rest; do
  port=${port%%@*}
  case $port in q[0-9]*)
    ip link set \"$port\" type bridge_slave isolated on
    ip -d link show dev \"$port\" | grep -q 'isolated on'
  ;; esac
done",
    )
}

pub fn reload(
    store: &crate::store::Store,
) -> (
    std::collections::BTreeMap<String, Link>,
    std::collections::BTreeSet<String>,
) {
    let mut unisolated = std::collections::BTreeSet::new();
    let links = store
        .load_links()
        .into_iter()
        .map(|mut link| {
            if crate::netsecret::take(&mut link.egress).is_some()
                && let Err(error) = store.save_link(&link)
            {
                eprintln!(
                    "job daemon: {}",
                    crate::netsecret::message(
                        "cannot save network {name} without its proxy password: {error}",
                        &[("name", link.name.clone()), ("error", error.to_string())],
                    )
                );
            }
            if alive(&link)
                && let Err(error) = isolate_ports(&link)
            {
                eprintln!("job daemon: {}", unisolated_message(&link.name, &error));
                unisolated.insert(link.name.clone());
            }
            (link.name.clone(), link)
        })
        .collect();
    (links, unisolated)
}

fn unisolated_message(name: &str, error: &io::Error) -> String {
    crate::netsecret::message(
        "the jobs already on network {name} could not be kept apart from each other: {error}; no further job joins it until that works",
        &[("name", name.to_string()), ("error", error.to_string())],
    )
}

pub fn admit(
    link: &Link,
    unisolated: &mut std::collections::BTreeSet<String>,
) -> Result<(), String> {
    if !unisolated.contains(&link.name) {
        return Ok(());
    }
    isolate_ports(link).map_err(|error| unisolated_message(&link.name, &error))?;
    unisolated.remove(&link.name);
    Ok(())
}

pub fn set_rates(link: &Link, jobs: &[(u32, u64)]) -> io::Result<()> {
    let Some(total) = link.rate else {
        return Ok(());
    };
    if jobs.is_empty() {
        return Ok(());
    }
    let share = (total / jobs.len() as u64).max(8_000);
    let mut script = format!("for dev in br0 {}; do\n", link.exit());
    for &(index, cap) in jobs {
        let ceil = cap.min(total);
        script.push_str(&format!(
            "  tc class change dev $dev parent 1:1 classid {} htb rate {}bit ceil {}bit\n",
            class(index),
            share.min(ceil),
            ceil
        ));
    }
    script.push_str("done");
    inside(link.holder, &script)
}

pub fn set_total(link: &Link) -> io::Result<()> {
    let Some(rate) = link.rate else {
        return Ok(());
    };
    inside(
        link.holder,
        &format!(
            "for dev in br0 {exit}; do tc class change dev $dev parent 1: classid 1:1 htb rate {rate}bit ceil {rate}bit; done",
            exit = link.exit()
        ),
    )
}

pub fn detach(link: &Link, job: &JobLink) {
    job.holder.end();
    let index = job.index;
    let _ = inside(
        link.holder,
        &format!(
            "nft delete element ip job marks {{ {address} }} || true
ip link del q{index} 2>/dev/null || true
for dev in br0 {exit}; do
  tc filter del dev $dev parent 1: protocol ip prio 1 handle {index} fw 2>/dev/null || true
  tc class del dev $dev classid {class} 2>/dev/null || true
done",
            address = job_address(index),
            class = class(index),
            exit = link.exit(),
        ),
    );
}

pub fn destroy(link: &Link) {
    link.relay.end();
    link.holder.end();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_address_keeps_its_scheme_credentials_host_and_port() {
        let target = proxy_target("socks5://user:secret@proxy.example:1080/").unwrap();
        assert_eq!(target.scheme, "socks5");
        assert_eq!(target.host, "proxy.example");
        assert_eq!(target.port, 1080);
        assert!(
            proxy_target("http://proxy.example")
                .unwrap_err()
                .contains("no port")
        );
    }
}
