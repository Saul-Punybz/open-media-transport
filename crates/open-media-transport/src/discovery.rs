//! DNS-SD discovery: announcing a source and finding others
//! (`docs/PROTOCOL.md` §7).
//!
//! Uses the pure-Rust [`mdns_sd`] responder. Names follow libomtnet: the
//! instance is `MACHINE (Name)` where `MACHINE` is the OS host name
//! upper-cased (D2, D3), cut to 63 characters by shortening `Name` (D4), with
//! an empty TXT record (D5). Loopback interfaces are not used, so loopback
//! addresses are never advertised to the network.

use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};

/// The DNS-SD service type, with domain (D1).
pub const SERVICE_TYPE: &str = "_omt._tcp.local.";

/// Longest full name libomtnet produces (`OMTAddress.cs:40`).
pub const MAX_FULL_NAME: usize = 63;

/// Errors from the mDNS layer.
pub type Error = mdns_sd::Error;

/// The OS host name, as libomtnet reads it: `gethostname()` on Unix,
/// `ComputerNamePhysicalDnsHostname` on Windows (D3).
pub fn os_host_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".to_owned())
}

/// The machine part of a source name: the host name upper-cased (D3).
pub fn machine_name() -> String {
    os_host_name().to_uppercase()
}

/// `MACHINE (Name)`, shortening `Name` if the whole exceeds 63 characters
/// (`OMTAddress.cs:65-75,201-204`). Lengths are in UTF-16 code units in
/// libomtnet; this counts `char`s, which agrees for ASCII names.
pub fn full_name(machine: &str, name: &str) -> String {
    let full = format!("{machine} ({name})");
    let over = full.chars().count().saturating_sub(MAX_FULL_NAME);
    let name_len = name.chars().count();
    if over == 0 || over >= name_len {
        return full;
    }
    let short: String = name.chars().take(name_len - over).collect();
    format!("{machine} ({})", short.trim())
}

/// Whether a discovered instance name looks like an OMT source: libomtnet
/// accepts it only if it contains both `(` and `)` (D7).
pub fn is_valid_full_name(full_name: &str) -> bool {
    full_name.contains('(') && full_name.contains(')')
}

/// Splits `MACHINE (Name)` into its parts (`OMTAddress.cs:220-249`).
pub fn split_full_name(full_name: &str) -> Option<(&str, &str)> {
    let open = full_name.find('(')?;
    let machine = full_name[..open].trim();
    let name = full_name[open + 1..].strip_suffix(')')?;
    Some((machine, name))
}

/// A source found on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// `MACHINE (Name)`, the name receivers use to select it.
    pub full_name: String,
    /// SRV target host, e.g. `my-mac.local.`.
    pub host: String,
    /// TCP port of the sender.
    pub port: u16,
    /// Addresses for `host`, never empty, best first: routable before
    /// link-local before loopback, IPv4 before IPv6. IPv6 link-local
    /// addresses are left out, as libomtnet does (D10). macOS answers for its
    /// own host name with loopback addresses too, which only work on the same
    /// machine.
    pub addresses: Vec<IpAddr>,
}

/// A change in what is on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceEvent {
    /// A source appeared, or its port or addresses changed.
    Resolved(Source),
    /// A source went away. Holds its full name.
    Removed(String),
}

/// The mDNS responder, shared by announcements and browsing.
pub struct Discovery {
    daemon: ServiceDaemon,
}

impl Discovery {
    /// Starts the responder on every non-loopback interface.
    pub fn new() -> Result<Self, Error> {
        let daemon = ServiceDaemon::new()?;
        // mdns-sd's loopback kinds match 127/8 and ::1 only; the loopback
        // interface's link-local fe80::1 (macOS lo0) would still be announced
        // to the network, so the interfaces are also excluded by name.
        daemon.disable_interface(vec![
            IfKind::LoopbackV4,
            IfKind::LoopbackV6,
            IfKind::Name("lo0".into()),
            IfKind::Name("lo".into()),
        ])?;
        Ok(Discovery { daemon })
    }

    /// Announces a source called `name` on `port` and returns its full name.
    /// It stays announced until [`Discovery::withdraw`] or drop.
    pub fn announce(&self, name: &str, port: u16) -> Result<String, Error> {
        let full = full_name(&machine_name(), name);
        let host = srv_host(&os_host_name());
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &full,
            &host,
            (),
            port,
            None::<std::collections::HashMap<String, String>>,
        )?
        .enable_addr_auto();
        self.daemon.register(info)?;
        Ok(full)
    }

    /// Withdraws a source announced with [`Discovery::announce`].
    pub fn withdraw(&self, full_name: &str) -> Result<(), Error> {
        let fullname = format!("{}.{SERVICE_TYPE}", escape_instance(full_name));
        let status = self.daemon.unregister(&fullname)?;
        let _ = status.recv_timeout(Duration::from_secs(1));
        Ok(())
    }

    /// Starts browsing for sources.
    pub fn browse(&self) -> Result<Browser, Error> {
        Ok(Browser {
            events: self.daemon.browse(SERVICE_TYPE)?,
        })
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        // Sends goodbyes for announced services and stops the daemon thread.
        if let Ok(done) = self.daemon.shutdown() {
            let _ = done.recv_timeout(Duration::from_secs(1));
        }
    }
}

/// A running browse for `_omt._tcp`.
pub struct Browser {
    events: mdns_sd::Receiver<ServiceEvent>,
}

impl Browser {
    /// Waits up to `timeout` for the next change. Instances without `(` and
    /// `)` in their name are skipped, as libomtnet skips them (D7).
    pub fn recv_timeout(&self, timeout: Duration) -> Option<SourceEvent> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(std::time::Instant::now())?;
            let event = self.events.recv_timeout(left).ok()?;
            let mapped = match event {
                ServiceEvent::ServiceResolved(s) => {
                    let full_name = instance_name(&s.fullname);
                    if !is_valid_full_name(&full_name) {
                        continue;
                    }
                    // IPv6 link-local addresses need an interface to be
                    // usable and libomtnet ignores them (D10,
                    // `OMTAddress.cs:98-101`); so do we. mdns-sd reports a
                    // service as soon as it has any address, so an event may
                    // carry none that is usable yet: wait for the next one.
                    let mut addresses: Vec<IpAddr> = s
                        .addresses
                        .iter()
                        .map(|a| a.to_ip_addr())
                        .filter(|a| !is_ipv6_link_local(a))
                        .collect();
                    if addresses.is_empty() {
                        continue;
                    }
                    addresses.sort_by_key(address_preference);
                    SourceEvent::Resolved(Source {
                        full_name,
                        host: s.host.clone(),
                        port: s.port,
                        addresses,
                    })
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    SourceEvent::Removed(instance_name(&fullname))
                }
                _ => continue,
            };
            return Some(mapped);
        }
    }
}

fn is_ipv6_link_local(a: &IpAddr) -> bool {
    matches!(a, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80)
}

fn address_preference(a: &IpAddr) -> (u8, bool, IpAddr) {
    let class = match a {
        _ if a.is_loopback() => 2,
        IpAddr::V4(v4) if v4.is_link_local() => 1,
        _ => 0,
    };
    (class, a.is_ipv6(), *a)
}

/// SRV target for our announcements: `<os host>-omt.local.`.
///
/// libomtnet hands registration to the OS responder, which points the SRV
/// record at the machine's own mDNS name. `mdns-sd` is a second responder and
/// must publish A/AAAA records for whatever host it names; naming the OS host
/// makes it probe against the OS responder, lose, and rename itself (seen on
/// macOS as `<host>-2.local.`, `docs/evidence/2026-09-21-our-discovery`). A
/// name of our own avoids the conflict. Receivers only use the instance name
/// to choose a source, then resolve whatever host the SRV record gives.
fn srv_host(os_host: &str) -> String {
    let base = os_host.trim_end_matches('.');
    let base = base.strip_suffix(".local").unwrap_or(base);
    format!("{base}-omt.local.")
}

fn escape_instance(name: &str) -> String {
    name.replace('\\', "\\\\").replace('.', "\\.")
}

/// Instance part of `Instance._omt._tcp.local.`, with DNS escapes (`\.`,
/// `\\`, `\DDD`) undone as libomtnet does (`OMTAddress.cs:149-188`).
fn instance_name(fullname: &str) -> String {
    let raw = fullname
        .strip_suffix(SERVICE_TYPE)
        .map(|s| s.strip_suffix('.').unwrap_or(s))
        .unwrap_or(fullname);
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let digits: String = std::iter::from_fn(|| chars.next_if(|d| d.is_ascii_digit()))
            .take(3)
            .collect();
        if digits.len() == 3 {
            if let Some(ch) = digits.parse::<u32>().ok().and_then(char::from_u32) {
                out.push(ch);
            }
        } else if digits.is_empty() {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push_str(&digits);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_name_format_and_limit() {
        assert_eq!(full_name("HOST", "Cam 1"), "HOST (Cam 1)");
        let long = "x".repeat(80);
        let f = full_name("SAULS-MACBOOK-PRO.LOCAL", &long);
        assert_eq!(f.chars().count(), MAX_FULL_NAME);
        assert!(f.starts_with("SAULS-MACBOOK-PRO.LOCAL (xxx") && f.ends_with(')'));
        // Name too short to absorb the excess: left as is, like libomtnet.
        let host = "H".repeat(70);
        assert_eq!(full_name(&host, "a"), format!("{host} (a)"));
    }

    #[test]
    fn validity_and_split() {
        assert!(is_valid_full_name("SAULS-MACBOOK-PRO.LOCAL (Cam 1.5)"));
        assert!(!is_valid_full_name("Printer"));
        assert_eq!(
            split_full_name("SAULS-MACBOOK-PRO.LOCAL (Cam 1.5)"),
            Some(("SAULS-MACBOOK-PRO.LOCAL", "Cam 1.5"))
        );
    }

    #[test]
    fn instance_names_are_unescaped() {
        // As shown by dns-sd for libomtnet's announcement (evidence/2026-09-21-libomtnet-loopback).
        assert_eq!(
            instance_name(r"SAULS-MACBOOK-PRO\.LOCAL\032(Cam\0321\.5)._omt._tcp.local."),
            "SAULS-MACBOOK-PRO.LOCAL (Cam 1.5)"
        );
        assert_eq!(instance_name("HOST (A)._omt._tcp.local."), "HOST (A)");
        assert_eq!(instance_name(r"a\\b (c)._omt._tcp.local."), r"a\b (c)");
    }

    #[test]
    fn address_order() {
        let mut a: Vec<IpAddr> = [
            "::1",
            "fe80::1",
            "127.0.0.1",
            "172.16.80.59",
            "fe80::4a6:3a1c:277d:23f0",
            "2001:db8::1",
        ]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
        a.retain(|x| !is_ipv6_link_local(x));
        a.sort_by_key(address_preference);
        let s: Vec<String> = a.iter().map(|x| x.to_string()).collect();
        assert_eq!(s, ["172.16.80.59", "2001:db8::1", "127.0.0.1", "::1"]);
    }

    #[test]
    fn srv_host_forms() {
        assert_eq!(
            srv_host("Sauls-MacBook-Pro.local"),
            "Sauls-MacBook-Pro-omt.local."
        );
        assert_eq!(srv_host("pi5"), "pi5-omt.local.");
    }
}
