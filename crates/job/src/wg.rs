use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, ToSocketAddrs};
use std::path::Path;

const NLM_F_REQUEST: u16 = 1;
const NLM_F_ACK: u16 = 4;
const NLMSG_ERROR: u16 = 2;
const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const NLA_F_NESTED: u16 = 1 << 15;
const WG_CMD_SET_DEVICE: u8 = 1;
const WG_GENL_VERSION: u8 = 1;
const WGDEVICE_A_IFNAME: u16 = 2;
const WGDEVICE_A_PRIVATE_KEY: u16 = 3;
const WGDEVICE_A_FLAGS: u16 = 5;
const WGDEVICE_A_PEERS: u16 = 8;
const WGDEVICE_F_REPLACE_PEERS: u32 = 1;
const WGPEER_A_PUBLIC_KEY: u16 = 1;
const WGPEER_A_PRESHARED_KEY: u16 = 2;
const WGPEER_A_FLAGS: u16 = 3;
const WGPEER_A_ENDPOINT: u16 = 4;
const WGPEER_A_PERSISTENT_KEEPALIVE_INTERVAL: u16 = 5;
const WGPEER_A_ALLOWEDIPS: u16 = 9;
const WGPEER_F_REPLACE_ALLOWEDIPS: u32 = 2;
const WGALLOWEDIP_A_FAMILY: u16 = 1;
const WGALLOWEDIP_A_IPADDR: u16 = 2;
const WGALLOWEDIP_A_CIDR_MASK: u16 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub public_key: [u8; 32],
    pub preshared_key: Option<[u8; 32]>,
    pub endpoint: Option<String>,
    pub allowed: Vec<(Ipv4Addr, u8)>,
    pub keepalive: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub private_key: [u8; 32],
    pub addresses: Vec<(Ipv4Addr, u8)>,
    pub dns: Vec<Ipv4Addr>,
    pub peers: Vec<Peer>,
}

fn base64_key(text: &str) -> Result<[u8; 32], String> {
    let value = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let bytes = text.trim().trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(33);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= value(c).ok_or("a key is not base64")? << (18 - 6 * i);
        }
        let produced = chunk.len() * 6 / 8;
        out.extend_from_slice(&acc.to_be_bytes()[1..1 + produced]);
    }
    out.try_into()
        .map_err(|_| "a key is not 32 bytes".to_string())
}

fn ipv4_prefix(text: &str) -> Option<(Ipv4Addr, u8)> {
    let (address, prefix) = text.trim().split_once('/').unwrap_or((text.trim(), "32"));
    Some((address.parse().ok()?, prefix.parse().ok()?))
}

pub fn parse(text: &str) -> Result<Config, String> {
    let mut private_key = None;
    let mut addresses = Vec::new();
    let mut dns = Vec::new();
    let mut peers: Vec<Peer> = Vec::new();
    let mut section = "";
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = if line.eq_ignore_ascii_case("[peer]") {
                peers.push(Peer {
                    public_key: [0; 32],
                    preshared_key: None,
                    endpoint: None,
                    allowed: Vec::new(),
                    keepalive: None,
                });
                "peer"
            } else {
                "interface"
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        let list = || value.split(',').map(str::trim);
        match (section, key.as_str()) {
            ("interface", "privatekey") => private_key = Some(base64_key(value)?),
            ("interface", "address") => addresses.extend(list().filter_map(ipv4_prefix)),
            ("interface", "dns") => dns.extend(list().filter_map(|d| d.parse::<Ipv4Addr>().ok())),
            ("peer", _) => {
                let peer = peers.last_mut().ok_or("a peer line outside [Peer]")?;
                match key.as_str() {
                    "publickey" => peer.public_key = base64_key(value)?,
                    "presharedkey" => peer.preshared_key = Some(base64_key(value)?),
                    "endpoint" => peer.endpoint = Some(value.to_string()),
                    "allowedips" => peer.allowed.extend(list().filter_map(ipv4_prefix)),
                    "persistentkeepalive" => peer.keepalive = value.parse().ok(),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    let config = Config {
        private_key: private_key.ok_or("the configuration has no PrivateKey")?,
        addresses,
        dns,
        peers,
    };
    if config.addresses.is_empty() {
        return Err("the configuration has no IPv4 Address".to_string());
    }
    if config.peers.iter().all(|p| p.endpoint.is_none()) {
        return Err("the configuration has no peer with an Endpoint".to_string());
    }
    Ok(config)
}

pub fn read(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn resolve_endpoint(endpoint: &str) -> Result<SocketAddrV4, String> {
    endpoint
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve the endpoint {endpoint}: {e}"))?
        .find_map(|a| match a {
            std::net::SocketAddr::V4(v4) => Some(v4),
            std::net::SocketAddr::V6(_) => None,
        })
        .ok_or_else(|| format!("the endpoint {endpoint} has no IPv4 address"))
}

fn attribute(buffer: &mut Vec<u8>, kind: u16, payload: &[u8]) {
    let length = 4 + payload.len();
    buffer.extend_from_slice(&(length as u16).to_ne_bytes());
    buffer.extend_from_slice(&kind.to_ne_bytes());
    buffer.extend_from_slice(payload);
    while !buffer.len().is_multiple_of(4) {
        buffer.push(0);
    }
}

fn nested(buffer: &mut Vec<u8>, kind: u16, build: impl FnOnce(&mut Vec<u8>)) {
    let start = buffer.len();
    buffer.extend_from_slice(&[0, 0]);
    buffer.extend_from_slice(&(kind | NLA_F_NESTED).to_ne_bytes());
    build(buffer);
    let length = (buffer.len() - start) as u16;
    buffer[start..start + 2].copy_from_slice(&length.to_ne_bytes());
}

fn message(
    family: u16,
    command: u8,
    version: u8,
    flags: u16,
    seq: u32,
    attributes: &[u8],
) -> Vec<u8> {
    let length = 16 + 4 + attributes.len();
    let mut out = Vec::with_capacity(length);
    out.extend_from_slice(&(length as u32).to_ne_bytes());
    out.extend_from_slice(&family.to_ne_bytes());
    out.extend_from_slice(&flags.to_ne_bytes());
    out.extend_from_slice(&seq.to_ne_bytes());
    out.extend_from_slice(&0u32.to_ne_bytes());
    out.extend_from_slice(&[command, version, 0, 0]);
    out.extend_from_slice(attributes);
    out
}

struct Socket(i32);

impl Drop for Socket {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

impl Socket {
    fn open() -> io::Result<Socket> {
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_GENERIC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Socket(fd))
    }

    fn exchange(&self, request: &[u8]) -> io::Result<Vec<u8>> {
        let sent = unsafe { libc::send(self.0, request.as_ptr().cast(), request.len(), 0) };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut reply = vec![0u8; 65536];
        let received = unsafe { libc::recv(self.0, reply.as_mut_ptr().cast(), reply.len(), 0) };
        if received < 0 {
            return Err(io::Error::last_os_error());
        }
        reply.truncate(received as usize);
        Ok(reply)
    }
}

fn error_of(reply: &[u8]) -> Option<i32> {
    let kind = u16::from_ne_bytes([*reply.get(4)?, *reply.get(5)?]);
    (kind == NLMSG_ERROR).then(|| i32::from_ne_bytes(reply[16..20].try_into().unwrap_or([0; 4])))
}

fn family_id(socket: &Socket) -> io::Result<u16> {
    let mut attributes = Vec::new();
    attribute(&mut attributes, CTRL_ATTR_FAMILY_NAME, b"wireguard\0");
    let reply = socket.exchange(&message(
        GENL_ID_CTRL,
        CTRL_CMD_GETFAMILY,
        1,
        NLM_F_REQUEST,
        1,
        &attributes,
    ))?;
    if let Some(code) = error_of(&reply).filter(|c| *c != 0) {
        return Err(io::Error::from_raw_os_error(-code));
    }
    let mut at = 20;
    while at + 4 <= reply.len() {
        let length = u16::from_ne_bytes([reply[at], reply[at + 1]]) as usize;
        let kind = u16::from_ne_bytes([reply[at + 2], reply[at + 3]]);
        if kind == CTRL_ATTR_FAMILY_ID && at + 6 <= reply.len() {
            return Ok(u16::from_ne_bytes([reply[at + 4], reply[at + 5]]));
        }
        at += (length.max(4) + 3) & !3;
    }
    Err(io::Error::other("the kernel has no wireguard family"))
}

fn sockaddr(address: SocketAddrV4) -> [u8; 16] {
    let mut raw = [0u8; 16];
    raw[0..2].copy_from_slice(&(libc::AF_INET as u16).to_ne_bytes());
    raw[2..4].copy_from_slice(&address.port().to_be_bytes());
    raw[4..8].copy_from_slice(&address.ip().octets());
    raw
}

pub fn configure(
    interface: &str,
    config: &Config,
    endpoints: &[Option<SocketAddrV4>],
) -> io::Result<()> {
    let socket = Socket::open()?;
    let family = family_id(&socket)?;
    let mut attributes = Vec::new();
    let mut name = interface.as_bytes().to_vec();
    name.push(0);
    attribute(&mut attributes, WGDEVICE_A_IFNAME, &name);
    attribute(&mut attributes, WGDEVICE_A_PRIVATE_KEY, &config.private_key);
    attribute(
        &mut attributes,
        WGDEVICE_A_FLAGS,
        &WGDEVICE_F_REPLACE_PEERS.to_ne_bytes(),
    );
    nested(&mut attributes, WGDEVICE_A_PEERS, |peers| {
        for (index, peer) in config.peers.iter().enumerate() {
            nested(peers, 0, |p| {
                attribute(p, WGPEER_A_PUBLIC_KEY, &peer.public_key);
                if let Some(key) = &peer.preshared_key {
                    attribute(p, WGPEER_A_PRESHARED_KEY, key);
                }
                attribute(
                    p,
                    WGPEER_A_FLAGS,
                    &WGPEER_F_REPLACE_ALLOWEDIPS.to_ne_bytes(),
                );
                if let Some(Some(endpoint)) = endpoints.get(index) {
                    attribute(p, WGPEER_A_ENDPOINT, &sockaddr(*endpoint));
                }
                if let Some(seconds) = peer.keepalive {
                    attribute(
                        p,
                        WGPEER_A_PERSISTENT_KEEPALIVE_INTERVAL,
                        &seconds.to_ne_bytes(),
                    );
                }
                nested(p, WGPEER_A_ALLOWEDIPS, |list| {
                    for (address, prefix) in &peer.allowed {
                        nested(list, 0, |a| {
                            attribute(
                                a,
                                WGALLOWEDIP_A_FAMILY,
                                &(libc::AF_INET as u16).to_ne_bytes(),
                            );
                            attribute(a, WGALLOWEDIP_A_IPADDR, &address.octets());
                            attribute(a, WGALLOWEDIP_A_CIDR_MASK, &[*prefix]);
                        });
                    }
                });
            });
        }
    });
    let reply = socket.exchange(&message(
        family,
        WG_CMD_SET_DEVICE,
        WG_GENL_VERSION,
        NLM_F_REQUEST | NLM_F_ACK,
        2,
        &attributes,
    ))?;
    match error_of(&reply) {
        Some(0) => Ok(()),
        Some(code) => Err(io::Error::from_raw_os_error(-code)),
        None => Err(io::Error::other(
            "the kernel gave no answer to the WireGuard settings",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "[Interface]
# Device: Example
PrivateKey = AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
Address = 10.64.1.2/32,fc00:bbbb::2/128
DNS = 10.64.0.1

[Peer]
PublicKey = //////////////////////////////////////////8=
AllowedIPs = 0.0.0.0/0,::0/0
Endpoint = 185.213.154.68:51820
";

    #[test]
    fn a_wireguard_configuration_is_read_as_wg_quick_writes_it() {
        let config = parse(EXAMPLE).unwrap();
        assert_eq!(config.addresses, vec![(Ipv4Addr::new(10, 64, 1, 2), 32)]);
        assert_eq!(config.dns, vec![Ipv4Addr::new(10, 64, 0, 1)]);
        assert_eq!(config.peers.len(), 1);
        assert_eq!(config.peers[0].allowed, vec![(Ipv4Addr::UNSPECIFIED, 0)]);
        assert_eq!(
            config.peers[0].endpoint.as_deref(),
            Some("185.213.154.68:51820")
        );
        assert_eq!(config.private_key, [0; 32]);
        assert_eq!(config.peers[0].public_key[0], 0xff);
        assert_eq!(config.peers[0].public_key[31], 0xff);
    }

    #[test]
    fn a_configuration_without_a_key_or_an_endpoint_is_refused() {
        assert!(parse("[Interface]\nAddress = 10.0.0.2/32\n").is_err());
        let no_endpoint = EXAMPLE.replace("Endpoint = 185.213.154.68:51820\n", "");
        assert!(parse(&no_endpoint).unwrap_err().contains("Endpoint"));
    }
}
