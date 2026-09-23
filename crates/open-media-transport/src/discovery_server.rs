//! Discovery server: finding sources on networks without multicast
//! (`docs/PROTOCOL.md` §10).
//!
//! libomtnet can replace DNS-SD with a central server. The server is a
//! metadata-only OMT sender, each client a metadata-only OMT receiver, and
//! the messages are ordinary metadata frames holding one `<OMTAddress>`
//! element each (S2, S3). This module has both ends:
//!
//! - [`Client`] connects to `omt://host:port` (default port 6399), registers
//!   the local sources, and keeps a table of the sources the server reports.
//!   It resends its sources on every reconnect and forgets what it learned
//!   when the connection drops (S6).
//! - [`Server`] tracks the sources each connection registers, records them
//!   with the connection's own TCP source address (S4), and sends every add
//!   and remove to all connections, including the one it came from (S5).
//!
//! [`crate::discovery::Discovery::with_server`] puts a [`Client`] behind the
//! usual announce/browse API.
//!
//! One deliberate difference from libomtnet's server: it sends its table to
//! a new connection from inside the accept handler
//! (`server/OMTDiscoveryServer.cs:152-163`, `OMTSend.cs:425-428`), but only
//! to connections already subscribed to metadata (`OMTSend.cs:647-653`). So
//! the table reaches the client only if its subscription has been processed
//! by then — a race. In our captures it always was (the table followed the
//! subscription by 100–200 ms, `docs/evidence/2026-09-23-discovery-server`),
//! but nothing in the code orders the two. Here the table is sent when the
//! connection's `<OMTSubscribe Metadata="true" />` arrives, so a new client
//! always gets it.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::command::{classify, Command, Message, Tally};
use crate::discovery::{self, Source, SourceEvent};
use crate::frame::ExtendedHeader;
use crate::{frame, Deframer, Limits};

/// Port a client uses when the server URL has none (`OMTConstants.cs:34`,
/// `OMTReceive.cs:342-343`), and the upstream server's default
/// (`upstream-OMTDiscoveryServer/Program.cs`).
pub const DEFAULT_PORT: u16 = 6399;

/// Minimum time between connection attempts: libomtnet's receiver rate
/// limit (`OMTReceive.cs:330-331`), which the discovery client inherits.
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// A server write that cannot complete in this time drops that connection,
/// so one stuck client cannot stall the others.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// One `<OMTAddress>` message (S3, `OMTAddress.cs:251-322`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressMessage {
    /// `MACHINE (Name)`.
    pub full_name: String,
    /// The source's TCP port.
    pub port: u16,
    /// `<Removed>True</Removed>`: the source went away.
    pub removed: bool,
    /// Addresses as libomtnet keeps them: IPv4 first, then IPv6, no IPv6
    /// link-local, no duplicates (`OMTAddress.cs:82-126`). IPv4 is held here
    /// as [`IpAddr::V4`] and written as IPv4-mapped IPv6, as libomtnet stores
    /// it.
    pub addresses: Vec<IpAddr>,
}

impl AddressMessage {
    /// A message for `full_name` on `port`.
    pub fn new(full_name: impl Into<String>, port: u16) -> Self {
        AddressMessage {
            full_name: full_name.into(),
            port,
            removed: false,
            addresses: Vec::new(),
        }
    }

    /// Adds an address the way `OMTAddress.AddAddress` does: IPv6 link-local
    /// is refused, duplicates are ignored, IPv4 goes after the other IPv4
    /// addresses and before all IPv6 ones (`OMTAddress.cs:82-126`). Returns
    /// whether it was added.
    pub fn add_address(&mut self, ip: IpAddr) -> bool {
        let ip = canonical(ip);
        if is_ipv6_link_local(&ip) || self.addresses.contains(&ip) {
            return false;
        }
        let at = if ip.is_ipv4() {
            self.addresses.iter().take_while(|a| a.is_ipv4()).count()
        } else {
            self.addresses.len()
        };
        self.addresses.insert(at, ip);
        true
    }

    /// The XML libomtnet writes: .NET `XmlTextWriter` with
    /// `Formatting.Indented`, two-space indent, no XML declaration and no
    /// trailing newline or NUL (`OMTAddress.cs:251-276`, M2). Line breaks
    /// are `\n`, .NET's `Environment.NewLine` on macOS and Linux; libomtnet on
    /// Windows would write `\r\n` (**inference**, not captured). Text escapes
    /// `&`, `<` and `>`.
    pub fn to_xml(&self) -> String {
        let mut x = String::from("<OMTAddress>\n");
        x.push_str(&format!(
            "  <Name>{}</Name>\n",
            escape_text(&self.full_name)
        ));
        x.push_str(&format!("  <Port>{}</Port>\n", self.port));
        if self.removed {
            x.push_str("  <Removed>True</Removed>\n");
        }
        if self.addresses.is_empty() {
            x.push_str("  <Addresses />\n");
        } else {
            x.push_str("  <Addresses>\n");
            for a in &self.addresses {
                x.push_str(&format!("    <IPAddress>{}</IPAddress>\n", wire_ip(a)));
            }
            x.push_str("  </Addresses>\n");
        }
        x.push_str("</OMTAddress>");
        x
    }

    /// Parses a metadata payload as `OMTAddress.FromXML` does
    /// (`OMTAddress.cs:278-322`): the root element must be `OMTAddress` with
    /// `Name` and `Port` children; `Name` is rebuilt through
    /// `OMTAddress.Create` (machine part trimmed, the last character dropped
    /// as the closing parenthesis, 63-character limit); addresses come from
    /// `Addresses/IPAddress`; `Removed` counts if its text is `true` in any
    /// case. Returns `None` for anything else.
    ///
    /// Two leniencies: trailing NULs are ignored (.NET's `LoadXml` rejects
    /// them), and a port outside 0–65535 is refused (libomtnet keeps any
    /// `int`).
    pub fn from_xml(payload: &[u8]) -> Option<AddressMessage> {
        let end = payload.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        let text = String::from_utf8_lossy(&payload[..end]);
        let doc = roxmltree::Document::parse(&text).ok()?;
        let root = doc.root_element();
        if root.tag_name().name() != "OMTAddress" || root.tag_name().namespace().is_some() {
            return None;
        }
        let name = inner_text(child(root, "Name")?);
        // `int.Parse` allows surrounding white space and a sign.
        let port: i32 = inner_text(child(root, "Port")?).trim().parse().ok()?;
        let port = u16::try_from(port).ok()?;
        let mut m = AddressMessage::new(normalize_full_name(&name)?, port);
        for list in root.children().filter(|n| n.has_tag_name("Addresses")) {
            for ip in list.children().filter(|n| n.has_tag_name("IPAddress")) {
                if let Some(ip) = parse_ip(&inner_text(ip)) {
                    m.add_address(ip);
                }
            }
        }
        m.removed = child(root, "Removed").is_some_and(|n| inner_text(n).to_lowercase() == "true");
        Some(m)
    }
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|n| n.has_tag_name(name))
}

/// .NET `XmlNode.InnerText`: all descendant text, concatenated.
fn inner_text(node: roxmltree::Node) -> String {
    node.descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}

/// `OMTAddress.Create(fullName, port).ToString()` (`OMTAddress.cs:220-232,43-75`).
fn normalize_full_name(full: &str) -> Option<String> {
    if !discovery::is_valid_full_name(full) {
        return None;
    }
    let open = full.find('(')?;
    if open == 0 {
        return None;
    }
    let machine = full[..open].trim();
    let mut name = full[open + 1..].chars();
    // `name.Substring(0, name.Length - 1)`: drops the last character, whatever it is.
    name.next_back()?;
    Some(discovery::full_name(machine, name.as_str()))
}

/// .NET `IPAddress.TryParse` accepts scoped IPv6 (`fe80::1%4`); the scope is
/// dropped here, and such addresses are link-local and refused anyway.
fn parse_ip(s: &str) -> Option<IpAddr> {
    let s = s.trim();
    let s = s
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .unwrap_or(s);
    let s = s.split('%').next().unwrap_or(s);
    s.parse().ok()
}

/// IPv4-mapped IPv6 becomes plain IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// How libomtnet writes an address: IPv4 as IPv4-mapped IPv6
/// (`OMTAddress.cs:84-97`), e.g. `::ffff:127.0.0.1`.
fn wire_ip(ip: &IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped().to_string(),
        IpAddr::V6(v6) => v6.to_string(),
    }
}

fn is_ipv6_link_local(a: &IpAddr) -> bool {
    matches!(a, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80)
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Splits `omt://host:port` into host and port, with [`DEFAULT_PORT`] when
/// the port is missing or 0 (`OMTDiscovery.cs:362-372`, `OMTReceive.cs:342`).
/// IPv6 hosts go in brackets: `omt://[::1]:6399`.
pub fn parse_url(url: &str) -> io::Result<(String, u16)> {
    let bad = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not an omt:// URL: {url}"),
        )
    };
    let rest = url
        .get(..6)
        .filter(|p| p.eq_ignore_ascii_case("omt://"))
        .map(|_| &url[6..])
        .ok_or_else(bad)?;
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or_else(bad)?;
        (h, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err(bad());
    }
    let port = match port {
        None | Some("") => DEFAULT_PORT,
        Some(p) => match p.parse::<u16>().map_err(|_| bad())? {
            0 => DEFAULT_PORT,
            p => p,
        },
    };
    Ok((host.to_owned(), port))
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A connection to a discovery server (S2, S3, S6).
///
/// Like libomtnet's `OMTDiscoveryClient`, it is a metadata-only receiver:
/// one TCP connection that sends `<OMTSubscribe Metadata="true" />` and then
/// the local sources (`OMTReceive.cs:506-512,451-454`,
/// `server/OMTDiscoveryClient.cs:88-118`). It reconnects at most once a
/// second, resolving the host name again each time, as libomtnet does
/// (`OMTReceive.cs:328-347`).
pub struct Client {
    shared: Arc<ClientShared>,
    worker: Option<JoinHandle<()>>,
}

struct ClientShared {
    host: String,
    port: u16,
    state: Mutex<ClientState>,
    wake: Condvar,
}

#[derive(Default)]
struct ClientState {
    closing: bool,
    /// Write half of the live connection.
    stream: Option<TcpStream>,
    /// Sources registered here, resent on every connect.
    local: Vec<AddressMessage>,
    /// Sources the server told us about, by full name.
    learned: BTreeMap<String, Source>,
    subscribers: Vec<flume::Sender<SourceEvent>>,
}

impl Client {
    /// Starts connecting to the server at `url` (`omt://host[:port]`). Returns
    /// at once; the connection is made, and remade, in the background.
    pub fn connect(url: &str) -> io::Result<Client> {
        let (host, port) = parse_url(url)?;
        let shared = Arc::new(ClientShared {
            host,
            port,
            state: Mutex::new(ClientState::default()),
            wake: Condvar::new(),
        });
        let s = shared.clone();
        let worker = std::thread::Builder::new()
            .name("omt-dserver-client".into())
            .spawn(move || s.run())?;
        Ok(Client {
            shared,
            worker: Some(worker),
        })
    }

    /// Registers a local source. It is sent now if connected, and again on
    /// every reconnect. Its address is sent as loopback, as libomtnet does
    /// (`OMTSend.cs:122-125`); the server replaces it with the address it
    /// sees (S4). Returns `false` if `full_name` is already registered
    /// (`OMTDiscovery.cs:304-325`).
    pub fn register(&self, full_name: &str, port: u16) -> bool {
        let mut st = self.shared.state.lock().unwrap();
        if st.local.iter().any(|a| a.full_name == full_name) {
            return false;
        }
        let mut m = AddressMessage::new(full_name, port);
        m.add_address(IpAddr::V4(Ipv4Addr::LOCALHOST));
        send_locked(&mut st, &m);
        st.local.push(m);
        true
    }

    /// Withdraws a source registered with [`Client::register`], telling the
    /// server with `<Removed>True</Removed>` (`OMTDiscovery.cs:347-360`).
    /// Returns `false` if it was not registered.
    pub fn deregister(&self, full_name: &str) -> bool {
        let mut st = self.shared.state.lock().unwrap();
        let Some(i) = st.local.iter().position(|a| a.full_name == full_name) else {
            return false;
        };
        let mut m = st.local.remove(i);
        m.removed = true;
        send_locked(&mut st, &m);
        true
    }

    /// Whether the connection to the server is up.
    pub fn is_connected(&self) -> bool {
        self.shared.state.lock().unwrap().stream.is_some()
    }

    /// Sources the server has reported and not removed, with at least one
    /// usable address.
    pub fn sources(&self) -> Vec<Source> {
        let st = self.shared.state.lock().unwrap();
        st.learned
            .values()
            .filter(|s| !s.addresses.is_empty())
            .cloned()
            .collect()
    }

    /// A feed of changes, starting with a [`SourceEvent::Resolved`] for each
    /// source already known.
    pub(crate) fn subscribe(&self) -> flume::Receiver<SourceEvent> {
        let (tx, rx) = flume::unbounded();
        let mut st = self.shared.state.lock().unwrap();
        for s in st.learned.values().filter(|s| !s.addresses.is_empty()) {
            let _ = tx.send(SourceEvent::Resolved(s.clone()));
        }
        st.subscribers.push(tx);
        rx
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        {
            let mut st = self.shared.state.lock().unwrap();
            st.closing = true;
            if let Some(s) = &st.stream {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
        self.shared.wake.notify_all();
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

/// Sends `m` if connected; a failed write closes the connection so the
/// worker reconnects.
fn send_locked(st: &mut ClientState, m: &AddressMessage) {
    if let Some(s) = &st.stream {
        let mut out = Vec::new();
        frame::write_metadata(0, m.to_xml().as_bytes(), &mut out);
        if (&*s).write_all(&out).is_err() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

fn notify(st: &mut ClientState, e: SourceEvent) {
    st.subscribers.retain(|tx| tx.send(e.clone()).is_ok());
}

impl ClientShared {
    fn run(&self) {
        let mut last_attempt: Option<Instant> = None;
        loop {
            {
                let mut st = self.state.lock().unwrap();
                if let Some(t) = last_attempt {
                    let due = t + RETRY_INTERVAL;
                    while !st.closing {
                        let Some(left) = due.checked_duration_since(Instant::now()) else {
                            break;
                        };
                        st = self.wake.wait_timeout(st, left).unwrap().0;
                    }
                }
                if st.closing {
                    return;
                }
            }
            last_attempt = Some(Instant::now());
            let Ok(stream) = connect_any(&self.host, self.port) else {
                continue;
            };
            let Ok(writer) = stream.try_clone() else {
                continue;
            };
            {
                let mut st = self.state.lock().unwrap();
                if st.closing {
                    let _ = stream.shutdown(Shutdown::Both);
                    return;
                }
                // §4.3 metadata-only sequence, then every local source (S6).
                let mut out = Vec::new();
                frame::write_metadata(0, Command::SubscribeMetadata.as_bytes(), &mut out);
                for m in &st.local {
                    frame::write_metadata(0, m.to_xml().as_bytes(), &mut out);
                }
                if (&writer).write_all(&out).is_err() {
                    continue;
                }
                st.stream = Some(writer);
            }
            self.read(stream);
            let mut st = self.state.lock().unwrap();
            st.stream = None;
            // S6: server-learned sources go with the connection
            // (`OMTDiscovery.cs:427-446`).
            let gone = std::mem::take(&mut st.learned);
            for (name, s) in gone {
                if !s.addresses.is_empty() {
                    notify(&mut st, SourceEvent::Removed(name));
                }
            }
        }
    }

    fn read(&self, mut stream: TcpStream) {
        // A metadata connection's receive buffer is 1 MiB (`OMTChannel.cs:111-114`).
        let mut deframer = Deframer::new(Limits::AUDIO_OR_METADATA);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match stream.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            };
            deframer.push(&buf[..n]);
            loop {
                match deframer.next_frame() {
                    Ok(Some(f)) if f.ext == ExtendedHeader::None => {
                        // Commands such as the tally the server sends on
                        // connect are consumed by the channel in libomtnet
                        // and never reach the discovery client.
                        if let Message::Application(x) | Message::SenderInfo(x) = classify(&f.data)
                        {
                            if let Some(m) = AddressMessage::from_xml(x) {
                                self.apply(m);
                            }
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => {
                        let _ = stream.shutdown(Shutdown::Both);
                        return;
                    }
                }
            }
        }
    }

    /// `server/OMTDiscoveryClient.cs:146-162`: a removal drops the entry by
    /// full name; an add creates it or merges port and addresses into it
    /// (`OMTDiscovery.cs:193-247`).
    fn apply(&self, m: AddressMessage) {
        let mut st = self.state.lock().unwrap();
        if m.removed {
            if let Some(s) = st.learned.remove(&m.full_name) {
                if !s.addresses.is_empty() {
                    notify(&mut st, SourceEvent::Removed(m.full_name));
                }
            }
            return;
        }
        let entry = st
            .learned
            .entry(m.full_name.clone())
            .or_insert_with(|| Source {
                full_name: m.full_name.clone(),
                host: String::new(),
                port: m.port,
                addresses: Vec::new(),
            });
        let mut changed = entry.port != m.port || entry.addresses.is_empty();
        entry.port = m.port;
        for a in &m.addresses {
            if !entry.addresses.contains(a) {
                entry.addresses.push(*a);
                changed = true;
            }
        }
        if entry.addresses.is_empty() {
            return; // nothing to connect to yet (`OMTDiscovery.cs:126-128`)
        }
        entry.addresses.sort_by_key(discovery::address_preference);
        // The server gives no host name; the best address stands in for it.
        entry.host = entry.addresses[0].to_string();
        if changed {
            let s = entry.clone();
            notify(&mut st, SourceEvent::Resolved(s));
        }
    }
}

fn connect_any(host: &str, port: u16) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("no address for {host}"));
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                s.set_nodelay(true)?; // T3
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Something the server did, for logging. libomtnet's server prints the
/// same four kinds of line (`server/OMTDiscoveryServer.cs:123,134,157,170`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerEvent {
    /// A client connected.
    Connected(SocketAddr),
    /// A client disconnected; its sources have been removed.
    Disconnected(SocketAddr),
    /// A source was added, by the client at the given address.
    Added(SocketAddr, AddressMessage),
    /// A source was removed, by or on behalf of the client at the given address.
    Removed(SocketAddr, AddressMessage),
}

/// A source the server knows about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerEntry {
    /// The source, with the registering connection's address (S4).
    pub address: AddressMessage,
    /// The connection that registered it.
    pub from: SocketAddr,
}

/// A discovery server (S2, S4, S5).
pub struct Server {
    port: u16,
    shared: Arc<ServerShared>,
    accept: Option<JoinHandle<()>>,
    events: Mutex<mpsc::Receiver<ServerEvent>>,
}

struct ServerShared {
    table: Mutex<Table>,
    readers: Mutex<Vec<JoinHandle<()>>>,
    events: mpsc::SyncSender<ServerEvent>,
    closing: AtomicBool,
}

#[derive(Default)]
struct Table {
    peers: Vec<Arc<Peer>>,
    entries: Vec<Entry>,
}

struct Entry {
    address: AddressMessage,
    peer: u64,
    from: SocketAddr,
}

struct Peer {
    id: u64,
    addr: SocketAddr,
    stream: TcpStream,
    metadata: AtomicBool,
}

impl Peer {
    /// Writes one metadata frame. Called with the table locked, so frames
    /// never interleave. A failed or timed-out write closes the connection.
    fn send(&self, bytes: &[u8]) {
        if (&self.stream).write_all(bytes).is_err() {
            let _ = self.stream.shutdown(Shutdown::Both);
        }
    }
}

impl Server {
    /// Listens on `port` (0 for any free port) on a dual-stack socket, as
    /// the upstream server does (`upstream-OMTDiscoveryServer/Program.cs`,
    /// `OMTSend.cs:64-76`).
    pub fn bind(port: u16) -> io::Result<Server> {
        let listener = crate::sender::bind_dual_stack(port)?;
        let port = listener.local_addr()?.port();
        let (tx, rx) = mpsc::sync_channel(1024);
        let shared = Arc::new(ServerShared {
            table: Mutex::new(Table::default()),
            readers: Mutex::new(Vec::new()),
            events: tx,
            closing: AtomicBool::new(false),
        });
        let s = shared.clone();
        let accept = std::thread::Builder::new()
            .name("omt-dserver-accept".into())
            .spawn(move || s.accept_loop(listener))?;
        Ok(Server {
            port,
            shared,
            accept: Some(accept),
            events: Mutex::new(rx),
        })
    }

    /// The TCP port clients connect to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Open client connections.
    pub fn connections(&self) -> usize {
        self.shared.table.lock().unwrap().peers.len()
    }

    /// The sources currently registered.
    pub fn entries(&self) -> Vec<ServerEntry> {
        let t = self.shared.table.lock().unwrap();
        t.entries
            .iter()
            .map(|e| ServerEntry {
                address: e.address.clone(),
                from: e.from,
            })
            .collect()
    }

    /// Waits up to `timeout` for the next event. Events are dropped, not
    /// queued without bound, when nobody reads them.
    pub fn recv_event(&self, timeout: Duration) -> Option<ServerEvent> {
        self.events.lock().unwrap().recv_timeout(timeout).ok()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::SeqCst);
        // Wake the blocking accept.
        for ip in [
            IpAddr::from([127, 0, 0, 1]),
            IpAddr::from([0u16, 0, 0, 0, 0, 0, 0, 1]),
        ] {
            if TcpStream::connect_timeout(
                &SocketAddr::new(ip, self.port),
                Duration::from_millis(200),
            )
            .is_ok()
            {
                break;
            }
        }
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
        for p in &self.shared.table.lock().unwrap().peers {
            let _ = p.stream.shutdown(Shutdown::Both);
        }
        let readers = std::mem::take(&mut *self.shared.readers.lock().unwrap());
        for h in readers {
            let _ = h.join();
        }
    }
}

impl ServerShared {
    fn event(&self, e: ServerEvent) {
        let _ = self.events.try_send(e);
    }

    fn accept_loop(self: Arc<Self>, listener: std::net::TcpListener) {
        let mut next_id = 0u64;
        for conn in listener.incoming() {
            if self.closing.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = conn else { continue };
            next_id += 1;
            let _ = self.start_peer(stream, next_id);
            // Forget readers that have finished.
            self.readers.lock().unwrap().retain(|h| !h.is_finished());
        }
    }

    fn start_peer(self: &Arc<Self>, stream: TcpStream, id: u64) -> io::Result<()> {
        stream.set_nodelay(true)?; // T3
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        let addr = stream.peer_addr()?;
        let peer = Arc::new(Peer {
            id,
            addr,
            stream: stream.try_clone()?,
            metadata: AtomicBool::new(false),
        });
        {
            let mut t = self.table.lock().unwrap();
            // libomtnet's sender sends its (always empty) tally to every new
            // connection, the discovery server's included (`OMTSend.cs:371`).
            let mut out = Vec::new();
            frame::write_metadata(0, Command::Tally(Tally::default()).as_bytes(), &mut out);
            peer.send(&out);
            t.peers.push(peer.clone());
        }
        self.event(ServerEvent::Connected(addr));
        let s = self.clone();
        let reader = std::thread::Builder::new()
            .name(format!("omt-dserver-r{id}"))
            .spawn(move || s.read_loop(stream, peer))?;
        self.readers.lock().unwrap().push(reader);
        Ok(())
    }

    fn read_loop(&self, mut stream: TcpStream, peer: Arc<Peer>) {
        let mut deframer = Deframer::new(Limits::AUDIO_OR_METADATA);
        let mut buf = vec![0u8; 64 * 1024];
        'read: loop {
            let n = match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            deframer.push(&buf[..n]);
            loop {
                match deframer.next_frame() {
                    Ok(Some(f)) if f.ext == ExtendedHeader::None => self.handle(&peer, &f.data),
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break 'read,
                }
            }
        }
        let _ = stream.shutdown(Shutdown::Both);
        self.disconnected(&peer);
    }

    fn handle(&self, peer: &Peer, data: &[u8]) {
        match classify(data) {
            Message::Command(Command::SubscribeMetadata) => {
                let t = self.table.lock().unwrap();
                if !peer.metadata.swap(true, Ordering::SeqCst) {
                    // The full table for a new client (S5), sent once it can
                    // receive it; see the module documentation.
                    for e in &t.entries {
                        peer.send(&address_frame(&e.address));
                    }
                }
            }
            Message::Application(x) | Message::SenderInfo(x) => {
                if let Some(m) = AddressMessage::from_xml(x) {
                    self.update(peer, m);
                }
            }
            _ => {}
        }
    }

    /// `server/OMTDiscoveryServer.cs:192-211`: an unknown name and port is
    /// added with the connection's address in place of the client's (S4);
    /// a removal of a known one removes it, whichever connection sends it;
    /// anything else is ignored. Changes go to every metadata connection.
    fn update(&self, peer: &Peer, mut m: AddressMessage) {
        let mut t = self.table.lock().unwrap();
        let known = t
            .entries
            .iter()
            .position(|e| e.address.full_name == m.full_name && e.address.port == m.port);
        match (known, m.removed) {
            (None, false) => {
                m.addresses.clear();
                m.add_address(peer.addr.ip());
                broadcast(&t, &m);
                t.entries.push(Entry {
                    address: m.clone(),
                    peer: peer.id,
                    from: peer.addr,
                });
                drop(t);
                self.event(ServerEvent::Added(peer.addr, m));
            }
            (Some(i), true) => {
                let mut e = t.entries.remove(i);
                e.address.removed = true;
                broadcast(&t, &e.address);
                drop(t);
                self.event(ServerEvent::Removed(peer.addr, e.address));
            }
            _ => {}
        }
    }

    /// `server/OMTDiscoveryServer.cs:93-111,165-176`: the connection's
    /// entries are removed and the removals sent to everyone left.
    fn disconnected(&self, peer: &Peer) {
        let mut removed = Vec::new();
        {
            let mut t = self.table.lock().unwrap();
            t.peers.retain(|p| p.id != peer.id);
            let (gone, kept): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut t.entries)
                .into_iter()
                .partition(|e| e.peer == peer.id);
            t.entries = kept;
            for mut e in gone {
                e.address.removed = true;
                broadcast(&t, &e.address);
                removed.push(e.address);
            }
        }
        for m in removed {
            self.event(ServerEvent::Removed(peer.addr, m));
        }
        self.event(ServerEvent::Disconnected(peer.addr));
    }
}

fn address_frame(m: &AddressMessage) -> Vec<u8> {
    let mut out = Vec::new();
    frame::write_metadata(0, m.to_xml().as_bytes(), &mut out);
    out
}

/// To every connection subscribed to metadata (`OMTSend.cs:637-658`).
fn broadcast(t: &Table, m: &AddressMessage) {
    let bytes = address_frame(m);
    for p in &t.peers {
        if p.metadata.load(Ordering::SeqCst) {
            p.send(&bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBOMTNET_XML: &str = "<OMTAddress>\n  <Name>HOST (Cam 1)</Name>\n  <Port>6400</Port>\n  <Addresses>\n    <IPAddress>::ffff:127.0.0.1</IPAddress>\n  </Addresses>\n</OMTAddress>";

    #[test]
    fn xml_round_trip() {
        let mut m = AddressMessage::new("HOST (Cam 1)", 6400);
        m.add_address("127.0.0.1".parse().unwrap());
        assert_eq!(m.to_xml(), LIBOMTNET_XML);
        assert_eq!(
            AddressMessage::from_xml(LIBOMTNET_XML.as_bytes()),
            Some(m.clone())
        );

        m.removed = true;
        m.addresses.clear();
        let x = m.to_xml();
        assert_eq!(
            x,
            "<OMTAddress>\n  <Name>HOST (Cam 1)</Name>\n  <Port>6400</Port>\n  <Removed>True</Removed>\n  <Addresses />\n</OMTAddress>"
        );
        assert_eq!(AddressMessage::from_xml(x.as_bytes()), Some(m));
    }

    #[test]
    fn parsing_follows_libomtnet() {
        // Upstream PROTOCOL.md's <Address> element is not what the code reads.
        let doc = "<OMTAddress><Name> M  (a&amp;b)</Name><Port> 7 </Port><Removed>TRUE</Removed>\
                   <Addresses><Address>1.2.3.4</Address><IPAddress>fe80::1</IPAddress>\
                   <IPAddress>2001:db8::1</IPAddress><IPAddress>10.0.0.1</IPAddress></Addresses></OMTAddress>\0";
        let m = AddressMessage::from_xml(doc.as_bytes()).unwrap();
        assert_eq!(m.full_name, "M (a&b)");
        assert_eq!(m.port, 7);
        assert!(m.removed);
        let want: Vec<IpAddr> = vec!["10.0.0.1".parse().unwrap(), "2001:db8::1".parse().unwrap()];
        assert_eq!(m.addresses, want);
        assert!(m.to_xml().contains("<Name>M (a&amp;b)</Name>"));

        for bad in [
            "<Other><Name>A (b)</Name><Port>1</Port></Other>",
            "<OMTAddress><Name>no parens</Name><Port>1</Port></OMTAddress>",
            "<OMTAddress><Name>(b)</Name><Port>1</Port></OMTAddress>",
            "<OMTAddress><Name>A (b)</Name></OMTAddress>",
            "<OMTAddress><Name>A (b)</Name><Port>x</Port></OMTAddress>",
            "not xml",
        ] {
            assert_eq!(AddressMessage::from_xml(bad.as_bytes()), None, "{bad}");
        }
    }

    #[test]
    fn address_order_like_libomtnet() {
        let mut m = AddressMessage::new("A (b)", 1);
        for a in [
            "2001:db8::1",
            "10.0.0.1",
            "::ffff:10.0.0.2",
            "10.0.0.1",
            "fe80::2",
        ] {
            m.add_address(a.parse().unwrap());
        }
        let got: Vec<String> = m.addresses.iter().map(wire_ip).collect();
        assert_eq!(got, ["::ffff:10.0.0.1", "::ffff:10.0.0.2", "2001:db8::1"]);
    }

    #[test]
    fn urls() {
        assert_eq!(parse_url("omt://srv:1234").unwrap(), ("srv".into(), 1234));
        assert_eq!(
            parse_url("OMT://srv").unwrap(),
            ("srv".into(), DEFAULT_PORT)
        );
        assert_eq!(parse_url("omt://[::1]:7/").unwrap(), ("::1".into(), 7));
        assert!(parse_url("srv:1234").is_err());
        assert!(parse_url("omt://:5").is_err());
    }

    fn wait_for(
        browser: &flume::Receiver<SourceEvent>,
        pred: impl Fn(&SourceEvent) -> bool,
    ) -> SourceEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let e = browser.recv_timeout(left).expect("event within 5 s");
            if pred(&e) {
                return e;
            }
        }
    }

    #[test]
    fn clients_find_each_other_through_the_server() {
        let server = Server::bind(0).unwrap();
        let url = format!("omt://127.0.0.1:{}", server.port());
        let a = Client::connect(&url).unwrap();
        a.register("HOST (a)", 6401);
        let b = Client::connect(&url).unwrap();
        let feed = b.subscribe();

        // b, connecting later, gets a's source with the address the server saw (S4).
        let e = wait_for(
            &feed,
            |e| matches!(e, SourceEvent::Resolved(s) if s.full_name == "HOST (a)"),
        );
        let SourceEvent::Resolved(s) = e else {
            unreachable!()
        };
        assert_eq!(s.port, 6401);
        assert_eq!(s.addresses, vec![IpAddr::from([127, 0, 0, 1])]);

        // The origin hears its own registration back (S5).
        let own = a.subscribe();
        wait_for(
            &own,
            |e| matches!(e, SourceEvent::Resolved(s) if s.full_name == "HOST (a)"),
        );

        a.deregister("HOST (a)");
        wait_for(&feed, |e| *e == SourceEvent::Removed("HOST (a)".into()));

        // A client's sources go when it disconnects (S5).
        a.register("HOST (a2)", 6402);
        wait_for(
            &feed,
            |e| matches!(e, SourceEvent::Resolved(s) if s.full_name == "HOST (a2)"),
        );
        drop(a);
        wait_for(&feed, |e| *e == SourceEvent::Removed("HOST (a2)".into()));
        assert!(server.entries().is_empty());
    }

    #[test]
    fn client_reregisters_after_server_restart_and_forgets_on_disconnect() {
        let server = Server::bind(0).unwrap();
        let port = server.port();
        let url = format!("omt://127.0.0.1:{port}");
        let a = Client::connect(&url).unwrap();
        a.register("HOST (r)", 6403);
        let feed = a.subscribe();
        wait_for(&feed, |e| matches!(e, SourceEvent::Resolved(_)));

        drop(server);
        // S6: learned sources are forgotten with the connection.
        wait_for(&feed, |e| *e == SourceEvent::Removed("HOST (r)".into()));
        assert!(a.sources().is_empty());

        // S6: on reconnect the client sends its sources again.
        let server = Server::bind(port).unwrap();
        wait_for(
            &feed,
            |e| matches!(e, SourceEvent::Resolved(s) if s.full_name == "HOST (r)"),
        );
        assert_eq!(server.entries().len(), 1);
    }
}
