use std::net::Ipv4Addr;

use super::message;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub address: Ipv4Addr,
    pub prefix: u8,
    pub ports: Option<(u16, u16)>,
    pub protocol: Option<&'static str>,
}

fn refused(key: &str, text: &str) -> String {
    message(key, &[("rule", text.to_string())])
}

pub fn address(text: &str) -> Result<Ipv4Addr, String> {
    if text.contains(':') {
        return Err(message(
            "`{address}` is an IPv6 address; these networks carry IPv4 only",
            &[("address", text.to_string())],
        ));
    }
    text.parse().map_err(|_| {
        message(
            "`{address}` is not an IPv4 address",
            &[("address", text.to_string())],
        )
    })
}

impl Rule {
    pub fn parse(text: &str) -> Result<Rule, String> {
        let (rest, protocol) = match text.rsplit_once('/') {
            Some((rest, "tcp")) => (rest, Some("tcp")),
            Some((rest, "udp")) => (rest, Some("udp")),
            _ => (text, None),
        };
        if rest.starts_with('[') || rest.matches(':').count() > 1 {
            return Err(refused(
                "`{rule}` names an IPv6 range; these networks carry IPv4 only, so write IPv4 ranges",
                text,
            ));
        }
        let (range, ports) = match rest.split_once(':') {
            Some((range, ports)) => {
                let (first, last) = ports.split_once('-').unwrap_or((ports, ports));
                let number = |port: &str| {
                    port.parse::<u16>()
                        .ok()
                        .filter(|port| *port > 0)
                        .ok_or_else(|| {
                            refused(
                                "`{rule}`: write a port as a number from 1 to 65535, or FIRST-LAST",
                                text,
                            )
                        })
                };
                let (first, last) = (number(first)?, number(last)?);
                if first > last {
                    return Err(refused(
                        "`{rule}`: write a port as a number from 1 to 65535, or FIRST-LAST",
                        text,
                    ));
                }
                (range, Some((first, last)))
            }
            None => (rest, None),
        };
        let (host, prefix) = match range.split_once('/') {
            Some((host, prefix)) => (
                host,
                prefix
                    .parse::<u8>()
                    .ok()
                    .filter(|prefix| *prefix <= 32)
                    .ok_or_else(|| {
                        refused(
                            "`{rule}`: write ADDRESS[/PREFIX][:PORT[-PORT]][/tcp|/udp] with an IPv4 address",
                            text,
                        )
                    })?,
            ),
            None => (range, 32),
        };
        let parsed: Ipv4Addr = host.parse().map_err(|_| {
            refused(
                "`{rule}`: write ADDRESS[/PREFIX][:PORT[-PORT]][/tcp|/udp] with an IPv4 address",
                text,
            )
        })?;
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - u32::from(prefix))
        };
        Ok(Rule {
            address: Ipv4Addr::from(u32::from(parsed) & mask),
            prefix,
            ports,
            protocol,
        })
    }

    pub fn canonical(&self) -> String {
        let mut text = format!("{}/{}", self.address, self.prefix);
        match self.ports {
            Some((first, last)) if first == last => text.push_str(&format!(":{first}")),
            Some((first, last)) => text.push_str(&format!(":{first}-{last}")),
            None => {}
        }
        if let Some(protocol) = self.protocol {
            text.push_str(&format!("/{protocol}"));
        }
        text
    }

    pub fn matcher(&self) -> String {
        let seen = if self.address.is_loopback() {
            format!("{}", crate::link::HOST_LOOPBACK)
        } else if self.prefix == 32 {
            format!("{}", self.address)
        } else {
            format!("{}/{}", self.address, self.prefix)
        };
        let ports = self.ports.map(|(first, last)| {
            if first == last {
                first.to_string()
            } else {
                format!("{first}-{last}")
            }
        });
        match (self.protocol, ports) {
            (Some(protocol), Some(ports)) => format!("ip daddr {seen} {protocol} dport {ports}"),
            (Some(protocol), None) => format!("ip daddr {seen} meta l4proto {protocol}"),
            (None, Some(ports)) => {
                format!("ip daddr {seen} meta l4proto {{ tcp, udp }} th dport {ports}")
            }
            (None, None) => format!("ip daddr {seen}"),
        }
    }
}
