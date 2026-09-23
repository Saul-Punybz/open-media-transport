//! Addressing a sender: full names, `omt://` URLs, and a directory of
//! discovered sources to look names up in (`docs/PROTOCOL.md` §8).
//!
//! libomtnet accepts either a full name `MACHINE (Name)`, matched exactly
//! against what discovery has found (N2), or a URL `omt://host:port`, whose
//! host is resolved with DNS and never through discovery (N3). Which one is
//! decided by a case-insensitive `omt://` prefix (`OMTDiscovery.cs:389-400`).
//! The same strings are what a redirect carries (§9).

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::discovery::{self, Discovery, Source, SourceEvent};

/// The URL scheme prefix, compared case-insensitively
/// (`OMTConstants.cs:72`, `OMTDiscovery.cs:392`).
pub const URL_PREFIX: &str = "omt://";

/// Where a receiver should connect.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Address {
    /// A fixed socket address. Not an OMT address form; nothing is resolved.
    Socket(SocketAddr),
    /// A full name `MACHINE (Name)`, looked up in a [`Directory`] by exact
    /// string equality (N2, `OMTDiscovery.cs:401-415`).
    Name(String),
    /// `omt://host:port`: `host` is resolved with DNS on every connection
    /// attempt (N3, `OMTDiscovery.cs:362-387`, `OMTReceive.cs:328-349`).
    Url {
        /// Host name or IP address, without brackets.
        host: String,
        /// TCP port.
        port: u16,
    },
}

/// Why a string is not an OMT address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressError {
    /// The empty string.
    Empty,
    /// An `omt://` URL without a host.
    MissingHost,
    /// An `omt://` URL without a port, or with one outside 1..=65535.
    /// libomtnet gives up on such a URL (`OMTDiscovery.cs:367-372,394`).
    BadPort,
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AddressError::Empty => "empty OMT address",
            AddressError::MissingHost => "omt:// URL without a host",
            AddressError::BadPort => "omt:// URL needs a port in 1..=65535",
        })
    }
}

impl std::error::Error for AddressError {}

impl Address {
    /// Parses a full name or an `omt://host:port` URL, as libomtnet's
    /// `FindByFullNameOrUrl` tells them apart (`OMTDiscovery.cs:389-400`):
    /// anything that does not start with `omt://` (any case) is a full name.
    /// Names are not trimmed or otherwise changed.
    pub fn parse(s: &str) -> Result<Address, AddressError> {
        if s.is_empty() {
            return Err(AddressError::Empty);
        }
        let is_url = s
            .get(..URL_PREFIX.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(URL_PREFIX));
        if !is_url {
            return Ok(Address::Name(s.to_owned()));
        }
        // Authority only; a path, query or fragment is ignored, as .NET's
        // `Uri.Host`/`Uri.Port` ignore them.
        let rest = &s[URL_PREFIX.len()..];
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (h, after) = v6.split_once(']').ok_or(AddressError::MissingHost)?;
            (h, after.strip_prefix(':').ok_or(AddressError::BadPort)?)
        } else {
            authority.rsplit_once(':').ok_or(AddressError::BadPort)?
        };
        if host.is_empty() {
            return Err(AddressError::MissingHost);
        }
        let port: u16 = port.parse().map_err(|_| AddressError::BadPort)?;
        if port == 0 {
            return Err(AddressError::BadPort);
        }
        Ok(Address::Url {
            host: host.to_owned(),
            port,
        })
    }

    /// Resolves to the socket addresses to try, best first. Names need a
    /// directory and are looked up without waiting; URLs are resolved with
    /// the system resolver.
    pub fn resolve(&self, directory: Option<&Directory>) -> io::Result<Vec<SocketAddr>> {
        let addrs: Vec<SocketAddr> = match self {
            Address::Socket(a) => vec![*a],
            Address::Name(name) => {
                let dir = directory.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "a full name needs a directory")
                })?;
                dir.get(name).map(|s| socket_addrs(&s)).unwrap_or_default()
            }
            Address::Url { host, port } => (host.as_str(), *port).to_socket_addrs()?.collect(),
        };
        if addrs.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{self} not found"),
            ));
        }
        Ok(addrs)
    }
}

impl FromStr for Address {
    type Err = AddressError;
    fn from_str(s: &str) -> Result<Address, AddressError> {
        Address::parse(s)
    }
}

impl From<SocketAddr> for Address {
    fn from(a: SocketAddr) -> Address {
        Address::Socket(a)
    }
}

impl fmt::Display for Address {
    /// The string form a sender would put in a redirect: the full name, or
    /// `omt://host:port`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Socket(a) => write!(f, "{URL_PREFIX}{a}"),
            Address::Name(n) => f.write_str(n),
            Address::Url { host, port } if host.contains(':') => {
                write!(f, "{URL_PREFIX}[{host}]:{port}")
            }
            Address::Url { host, port } => write!(f, "{URL_PREFIX}{host}:{port}"),
        }
    }
}

fn socket_addrs(s: &Source) -> Vec<SocketAddr> {
    s.addresses
        .iter()
        .map(|ip: &IpAddr| SocketAddr::new(*ip, s.port))
        .collect()
}

#[derive(Default)]
struct Table {
    sources: Mutex<HashMap<String, Source>>,
    changed: Condvar,
}

impl Table {
    fn apply(&self, event: SourceEvent) {
        let mut t = self.sources.lock().unwrap();
        match event {
            SourceEvent::Resolved(s) => {
                t.insert(s.full_name.clone(), s);
            }
            SourceEvent::Removed(name) => {
                t.remove(&name);
            }
        }
        self.changed.notify_all();
    }
}

/// The sources currently on the network, by full name, kept up to date in
/// the background — libomtnet's discovery table (`OMTDiscovery.cs:193-247`).
///
/// A source that goes away is removed; when it comes back, possibly on
/// another port or address, the entry is replaced, so looking the name up
/// again finds where it is now. Share one directory among receivers with an
/// `Arc`; each one built with [`Directory::browse`] runs its own mDNS
/// responder, while [`Directory::with_shared`] uses one shared with others.
pub struct Directory {
    table: Arc<Table>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    // Dropped after the browse thread has been joined.
    _discovery: Option<Arc<Discovery>>,
}

impl Directory {
    /// Starts browsing `_omt._tcp` with DNS-SD (§7).
    pub fn browse() -> Result<Directory, discovery::Error> {
        Directory::with_discovery(Discovery::new()?)
    }

    /// Browses with `discovery`, e.g. one made with
    /// [`Discovery::with_server`] to find sources through a discovery server
    /// (§10) as well as, or instead of, DNS-SD.
    pub fn with_discovery(discovery: Discovery) -> Result<Directory, discovery::Error> {
        Directory::with_shared(Arc::new(discovery))
    }

    /// Browses with a [`Discovery`] shared with senders or other directories.
    pub fn with_shared(discovery: Arc<Discovery>) -> Result<Directory, discovery::Error> {
        let browser = discovery.browse()?;
        let table = Arc::new(Table::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (t, s) = (table.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("omt-directory".into())
            .spawn(move || {
                while !s.load(Ordering::SeqCst) {
                    if let Some(e) = browser.recv_timeout(Duration::from_millis(100)) {
                        t.apply(e);
                    }
                }
            })
            .map_err(|e| discovery::Error::Msg(e.to_string()))?;
        Ok(Directory {
            table,
            stop,
            thread: Some(thread),
            _discovery: Some(discovery),
        })
    }

    /// An empty directory fed only through [`Directory::apply`], e.g. from
    /// another discovery mechanism or in tests.
    pub fn manual() -> Directory {
        Directory {
            table: Arc::new(Table::default()),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            _discovery: None,
        }
    }

    /// Records a change.
    pub fn apply(&self, event: SourceEvent) {
        self.table.apply(event);
    }

    /// The source with exactly this full name, if present now.
    pub fn get(&self, full_name: &str) -> Option<Source> {
        self.table.sources.lock().unwrap().get(full_name).cloned()
    }

    /// Waits up to `timeout` for a source with exactly this full name.
    pub fn wait_for(&self, full_name: &str, timeout: Duration) -> Option<Source> {
        let deadline = Instant::now() + timeout;
        let mut t = self.table.sources.lock().unwrap();
        loop {
            if let Some(s) = t.get(full_name) {
                return Some(s.clone());
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            t = self.table.changed.wait_timeout(t, left).unwrap().0;
        }
    }

    /// Every source present now, sorted by full name.
    pub fn sources(&self) -> Vec<Source> {
        let mut v: Vec<Source> = self
            .table
            .sources
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| a.full_name.cmp(&b.full_name));
        v
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_urls() {
        assert_eq!(
            Address::parse("MY-MAC.LOCAL (Cam 1)"),
            Ok(Address::Name("MY-MAC.LOCAL (Cam 1)".into()))
        );
        // Anything that is not a URL is a name, as in libomtnet.
        assert_eq!(
            Address::parse("127.0.0.1:6400"),
            Ok(Address::Name("127.0.0.1:6400".into()))
        );
        assert_eq!(
            Address::parse("omt://my-mac.local:6400"),
            Ok(Address::Url {
                host: "my-mac.local".into(),
                port: 6400
            })
        );
        assert_eq!(
            Address::parse("OMT://10.0.0.2:6401/"),
            Ok(Address::Url {
                host: "10.0.0.2".into(),
                port: 6401
            })
        );
        assert_eq!(
            Address::parse("omt://[::1]:6400"),
            Ok(Address::Url {
                host: "::1".into(),
                port: 6400
            })
        );
        assert_eq!(Address::parse(""), Err(AddressError::Empty));
        assert_eq!(Address::parse("omt://host"), Err(AddressError::BadPort));
        assert_eq!(Address::parse("omt://host:0"), Err(AddressError::BadPort));
        assert_eq!(
            Address::parse("omt://:6400"),
            Err(AddressError::MissingHost)
        );
    }

    #[test]
    fn display_round_trips() {
        for s in ["HOST (A)", "omt://h:1", "omt://[::1]:6400"] {
            assert_eq!(Address::parse(s).unwrap().to_string(), s);
        }
        assert_eq!(
            Address::Socket("127.0.0.1:6400".parse().unwrap()).to_string(),
            "omt://127.0.0.1:6400"
        );
    }

    #[test]
    fn urls_resolve_without_discovery() {
        let a = Address::parse("omt://127.0.0.1:6400").unwrap();
        assert_eq!(
            a.resolve(None).unwrap(),
            ["127.0.0.1:6400".parse::<SocketAddr>().unwrap()]
        );
    }

    #[test]
    fn names_follow_the_directory() {
        let d = Directory::manual();
        let name = Address::parse("HOST (A)").unwrap();
        assert!(name.resolve(None).is_err());
        assert_eq!(
            name.resolve(Some(&d)).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let src = |port| Source {
            full_name: "HOST (A)".into(),
            host: "host-omt.local.".into(),
            port,
            addresses: vec!["127.0.0.1".parse().unwrap()],
        };
        d.apply(SourceEvent::Resolved(src(6400)));
        assert_eq!(name.resolve(Some(&d)).unwrap()[0].port(), 6400);
        d.apply(SourceEvent::Resolved(src(6405)));
        assert_eq!(name.resolve(Some(&d)).unwrap()[0].port(), 6405);
        d.apply(SourceEvent::Removed("HOST (A)".into()));
        assert!(name.resolve(Some(&d)).is_err());
        assert!(d.wait_for("HOST (A)", Duration::from_millis(20)).is_none());
    }
}
