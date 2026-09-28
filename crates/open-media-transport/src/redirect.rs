//! Redirect: a sender telling its receivers to use another source instead
//! (`docs/PROTOCOL.md` §9, "virtual source").
//!
//! The message is a metadata frame `<OMTRedirect NewAddress="…" />`, no NUL
//! (M2), recognised by its `<OMTRedirect` prefix and parsed as XML for the
//! `NewAddress` attribute (`OMTChannel.cs:394-398`, `OMTRedirect.cs:165-180`).
//! An empty address cancels the redirect. The sending side lives in
//! [`crate::sender::Sender::set_redirect`], the following side in
//! [`crate::receiver`].

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::address::{Address, Directory};
use crate::command::{classify, Message, Quality, Tally};
use crate::frame::ExtendedHeader;
use crate::receiver::{Event, Receiver, ReceiverConfig, RedirectPolicy};

/// The message for `address`. libomtnet writes it with .NET's
/// `XmlTextWriter` (`OMTRedirect.cs:181-194`), which produced
/// `<OMTRedirect NewAddress="…" />` on one line with one space before `/>`
/// (captured, `docs/evidence/2026-09-23-addressing`). `&`, `<`, `>` and `"`
/// are escaped here; how .NET escapes other characters has not been captured.
pub fn to_xml(address: &str) -> String {
    format!(r#"<OMTRedirect NewAddress="{}" />"#, escape_attr(address))
}

/// The `NewAddress` of a redirect message: `Some("")` for a cancel, `None`
/// when the payload is not a redirect or cannot be read. Like libomtnet,
/// which loads the message into an `XmlDocument` and reads the root
/// element's `NewAddress` attribute (`OMTRedirect.cs:165-180`,
/// `OMTMetadata.cs:63-76`), the whole message must be well-formed XML: a
/// duplicate attribute or a reference to a character XML forbids makes it
/// unreadable. A trailing NUL is tolerated.
///
/// An unreadable redirect is not ignored by libomtnet: `FromXML` returns
/// null and `RedirectChanged` is still raised (`OMTChannel.cs:394-398`),
/// which on a side connection cancels the redirect. [`heard`] gives that
/// meaning.
pub fn parse(payload: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(payload).ok()?;
    let s = s.trim_end_matches('\0');
    if !s.starts_with("<OMTRedirect") {
        return None;
    }
    let doc = roxmltree::Document::parse(s).ok()?;
    doc.root_element()
        .attributes()
        .find(|a| a.name() == "NewAddress" && a.namespace().is_none())
        .map(|a| a.value().to_owned())
}

/// What a receiver makes of a redirect message it recognised by its prefix
/// (`Message::Redirect`): its address, or `""`, a cancel, if the message
/// cannot be read, as in libomtnet (see [`parse`]).
pub fn heard(payload: &[u8]) -> String {
    parse(payload).unwrap_or_default()
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A metadata-only connection that reports every redirect message it hears:
/// libomtnet's "side connection", an `OMTReceive` for metadata only with
/// `redirectMetadataOnly` set (`OMTRedirect.cs:84-108`). It reconnects, and
/// re-resolves a name, like any receiver.
pub(crate) struct Watcher {
    address: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    pub(crate) fn start(
        address: Address,
        directory: Option<Arc<Directory>>,
        mut on_redirect: impl FnMut(String) + Send + 'static,
    ) -> io::Result<Watcher> {
        let text = address.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name("omt-redirect-watch".into())
            .spawn(move || {
                let config = ReceiverConfig {
                    video: false,
                    audio: false,
                    preview: false,
                    quality: Quality::Default,
                    tally: Tally::default(),
                    reconnect: true,
                    redirects: RedirectPolicy::Never,
                };
                let Ok(rx) = Receiver::start(address, config, directory, false) else {
                    return;
                };
                while !s.load(Ordering::SeqCst) {
                    if let Some(Event::Frame(_, f)) = rx.recv_timeout(Duration::from_millis(100)) {
                        if f.ext != ExtendedHeader::None {
                            continue;
                        }
                        if let Message::Redirect(x) = classify(&f.data) {
                            on_redirect(heard(x));
                        }
                    }
                }
            })?;
        Ok(Watcher {
            address: text,
            stop,
            thread: Some(thread),
        })
    }

    /// The address watched, as given.
    pub(crate) fn address(&self) -> &str {
        &self.address
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.thread.take() {
            crate::sender::join_bounded(h, std::time::Instant::now() + crate::sender::DROP_TIMEOUT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_form() {
        assert_eq!(
            to_xml("HOST (Cam 2)"),
            r#"<OMTRedirect NewAddress="HOST (Cam 2)" />"#
        );
        assert_eq!(to_xml(""), r#"<OMTRedirect NewAddress="" />"#);
        assert_eq!(
            to_xml(r#"A&B "x" <y>"#),
            r#"<OMTRedirect NewAddress="A&amp;B &quot;x&quot; &lt;y&gt;" />"#
        );
    }

    #[test]
    fn parses_what_it_writes() {
        for a in [
            "HOST (Cam 2)",
            "",
            "omt://h:6400",
            r#"A&B "x" <y>"#,
            "Cámara ñ",
        ] {
            assert_eq!(parse(to_xml(a).as_bytes()).as_deref(), Some(a), "{a}");
        }
    }

    #[test]
    fn parse_variants() {
        assert_eq!(
            parse(b"<OMTRedirect NewAddress='X (Y)'/>").as_deref(),
            Some("X (Y)")
        );
        assert_eq!(
            parse(b"<OMTRedirect Other=\"1\"\n  NewAddress = \"a&#x41;&#66;\" />\0").as_deref(),
            Some("aAB")
        );
        assert_eq!(parse(b"<OMTRedirect />"), None);
        assert_eq!(parse(b"<OMTRedirect NewAddress=\"a&bogus;\" />"), None);
        // Bug hunt #7: not well-formed, so unreadable, as for `XmlDocument`.
        assert_eq!(
            parse(b"<OMTRedirect NewAddress=\"a\" NewAddress=\"b\" />"),
            None
        );
        assert_eq!(parse(b"<OMTRedirect NewAddress=\"a&#0;\" />"), None);
        assert_eq!(parse(b"<OMTRedirect NewAddress=\"a\">"), None);
        // An unreadable redirect is heard as a cancel.
        assert_eq!(
            heard(b"<OMTRedirect NewAddress=\"a\" NewAddress=\"b\" />"),
            ""
        );
        assert_eq!(heard(b"<OMTRedirect NewAddress=\"X (Y)\" />"), "X (Y)");
        assert_eq!(parse(b"<OMTRedirectNewAddress=\"a\" />"), None);
        assert_eq!(parse(b"<OMTInfo NewAddress=\"a\" />"), None);
    }

    mod end_to_end {
        use crate::receiver::{Event, Receiver, ReceiverConfig, RedirectPolicy};
        use crate::sender::{Sender, SenderConfig};
        use std::net::SocketAddr;
        use std::time::{Duration, Instant};

        fn sender(name: &str) -> Sender {
            Sender::new(SenderConfig {
                announce: false,
                ports: 18400..=18600,
                ..SenderConfig::new(name)
            })
            .unwrap()
        }

        fn url(s: &Sender) -> String {
            format!("omt://127.0.0.1:{}", s.port())
        }

        fn receiver(s: &Sender) -> Receiver {
            let cfg = ReceiverConfig {
                audio: false,
                ..ReceiverConfig::default()
            };
            Receiver::connect(SocketAddr::from(([127, 0, 0, 1], s.port())), cfg).unwrap()
        }

        fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !cond() {
                assert!(Instant::now() < deadline, "timed out: {what}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn on(rx: &Receiver, s: &Sender) -> bool {
            rx.peer_addr().map(|a| a.port()) == Some(s.port()) && s.video_receivers() == 1
        }

        fn redirects(rx: &Receiver) -> Vec<Option<String>> {
            let mut v = Vec::new();
            while let Some(e) = rx.recv_timeout(Duration::from_millis(50)) {
                if let Event::Redirect(r) = e {
                    v.push(r);
                }
            }
            v
        }

        #[test]
        fn follows_moves_and_returns() {
            let (a, b, c) = (sender("a"), sender("b"), sender("c"));
            let rx = receiver(&a);
            wait_for("on a", || on(&rx, &a));

            a.set_redirect(Some(&url(&b)));
            assert_eq!(a.redirect(), Some(url(&b)));
            wait_for("on b", || on(&rx, &b) && a.video_receivers() == 0);
            // X2: a keeps a metadata-only side connection from the receiver.
            wait_for("side connection", || a.connections() == 1);

            a.set_redirect(Some(&url(&c)));
            wait_for("on c", || on(&rx, &c) && b.video_receivers() == 0);

            a.set_redirect(None);
            assert_eq!(a.redirect(), None);
            wait_for("back on a", || on(&rx, &a) && c.video_receivers() == 0);
            assert_eq!(
                redirects(&rx),
                [Some(url(&b)), Some(url(&c)), None],
                "redirect events"
            );
            assert_eq!(rx.redirect(), None);
            let stats = rx.stats();
            assert_eq!((stats.redirects, stats.reconnects), (3, 0));
        }

        #[test]
        fn a_new_receiver_is_redirected_on_connect() {
            let (a, b) = (sender("a"), sender("b"));
            a.set_redirect(Some(&url(&b)));
            let rx = receiver(&a);
            wait_for("on b", || on(&rx, &b));
            assert_eq!(rx.redirect(), Some(url(&b)));
        }

        #[test]
        fn redirect_to_self_is_none() {
            let a = sender("a");
            a.set_redirect(Some(a.url()));
            assert_eq!(a.redirect(), None);
            let own = crate::discovery::full_name(&crate::discovery::machine_name(), "a");
            a.set_redirect(Some(&own));
            assert_eq!(a.redirect(), None);
        }

        #[test]
        fn chains_are_forwarded_by_the_first_sender() {
            // X3: a -> b, then b -> c. a forwards c; when b clears, a goes back to b.
            let (a, b, c) = (sender("a"), sender("b"), sender("c"));
            let rx = receiver(&a);
            wait_for("on a", || on(&rx, &a));
            a.set_redirect(Some(&url(&b)));
            wait_for("on b", || on(&rx, &b));
            wait_for("a watches b", || b.connections() == 2);

            b.set_redirect(Some(&url(&c)));
            wait_for("a forwards c", || a.redirect() == Some(url(&c)));
            wait_for("on c", || on(&rx, &c));

            b.set_redirect(None);
            wait_for("a forwards b again", || a.redirect() == Some(url(&b)));
            wait_for("back on b", || on(&rx, &b));
        }

        /// This machine's address on its LAN, if it has one: another host,
        /// as far as a receiver that reached a sender on loopback can tell.
        fn lan_ip() -> Option<std::net::IpAddr> {
            let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
            // No packet is sent: connecting a UDP socket only picks a route.
            s.connect("192.0.2.1:9").ok()?;
            let ip = s.local_addr().ok()?.ip();
            (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
        }

        fn receiver_with(s: &Sender, redirects: RedirectPolicy) -> Receiver {
            let cfg = ReceiverConfig {
                audio: false,
                redirects,
                ..ReceiverConfig::default()
            };
            Receiver::connect(SocketAddr::from(([127, 0, 0, 1], s.port())), cfg).unwrap()
        }

        #[test]
        fn same_host_refuses_a_redirect_to_another_host() {
            // Security review PoC B: a redirect could send a receiver anywhere.
            let Some(lan) = lan_ip() else {
                eprintln!("no LAN address; skipped");
                return;
            };
            let (a, b) = (sender("a"), sender("b"));
            let elsewhere = format!("omt://{lan}:{}", b.port());
            let rx = receiver_with(&a, RedirectPolicy::SameHost);
            wait_for("on a", || on(&rx, &a));
            a.set_redirect(Some(&elsewhere));
            std::thread::sleep(Duration::from_millis(1500));
            assert_eq!(b.video_receivers(), 0, "not followed");
            assert!(on(&rx, &a), "still on a");
            assert_eq!(rx.stats().reconnects, 0, "never left a");
            let closed = std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(50)))
                .any(|e| matches!(e, Event::Closed(..)));
            assert!(!closed, "the connections to a stayed open");

            // The same redirect with `Any` is followed, as by libomtnet.
            let any = receiver_with(&a, RedirectPolicy::Any);
            wait_for("on b", || b.video_receivers() == 1);
            assert_eq!(any.redirect(), Some(elsewhere));
        }

        #[test]
        fn an_unreadable_redirect_on_the_side_connection_cancels() {
            // Bug hunt #7: libomtnet reads a malformed redirect as null and
            // still raises RedirectChanged, which returns the receiver.
            let (a, b) = (sender("a"), sender("b"));
            let rx = receiver(&a);
            wait_for("on a", || on(&rx, &a));
            a.set_redirect(Some(&url(&b)));
            wait_for("on b", || on(&rx, &b));
            wait_for("side connection", || a.connections() == 1);
            a.send_metadata(br#"<OMTRedirect NewAddress="x" NewAddress="y" />"#, 0);
            wait_for("back on a", || on(&rx, &a));
            assert_eq!(rx.redirect(), None);
        }

        #[test]
        fn a_redirect_that_is_not_an_address_leaves_the_receiver_disconnected() {
            // Bug hunt #8: it used to fall back to the original silently.
            let a = sender("a");
            let rx = receiver(&a);
            wait_for("on a", || on(&rx, &a));
            a.set_redirect(Some("omt://127.0.0.1"));
            wait_for("disconnected", || !rx.is_connected());
            std::thread::sleep(Duration::from_millis(1500));
            assert!(!rx.is_connected());
            assert_eq!(a.video_receivers(), 0);
        }

        #[test]
        fn a_sender_watches_a_named_target_with_its_own_discovery() {
            // Bug hunt #9: the watcher looked names up with an mDNS responder
            // of its own, so a target known only to the sender's discovery
            // server was never watched and its redirect never forwarded.
            use crate::address::{Address, Directory};
            use crate::discovery::Discovery;
            use crate::discovery_server::Server;
            use std::sync::Arc;
            let server = Server::bind(0).unwrap();
            let d = Arc::new(
                Discovery::with_server(&format!("omt://127.0.0.1:{}", server.port()), false)
                    .unwrap(),
            );
            let announced = |name: &str| {
                Sender::new(SenderConfig {
                    discovery: Some(d.clone()),
                    ports: 18400..=18600,
                    ..SenderConfig::new(name)
                })
                .unwrap()
            };
            let (a, b, c) = (announced("w-a"), announced("w-b"), sender("w-c"));
            let dir = Arc::new(Directory::with_shared(d.clone()).unwrap());
            let b_name = b.full_name().unwrap().to_owned();
            assert!(dir.wait_for(&b_name, Duration::from_secs(5)).is_some());
            let cfg = ReceiverConfig {
                audio: false,
                ..ReceiverConfig::default()
            };
            let a_url = Address::parse(&url(&a)).unwrap();
            let rx = Receiver::connect_address(a_url, cfg, Some(dir)).unwrap();
            wait_for("on a", || on(&rx, &a));
            a.set_redirect(Some(&b_name));
            wait_for("on b", || on(&rx, &b));
            b.set_redirect(Some(&url(&c)));
            wait_for("a forwards c", || a.redirect() == Some(url(&c)));
            wait_for("on c", || on(&rx, &c));
        }

        #[test]
        fn no_redirect_message_on_connect_after_a_clear() {
            use crate::command::{classify, Message};
            let (a, b) = (sender("a"), sender("b"));
            a.set_redirect(Some(&url(&b)));
            a.set_redirect(None);
            let cfg = ReceiverConfig {
                audio: false,
                reconnect: false,
                redirects: RedirectPolicy::Never,
                ..ReceiverConfig::default()
            };
            let rx = Receiver::connect(SocketAddr::from(([127, 0, 0, 1], a.port())), cfg).unwrap();
            let mut kinds = Vec::new();
            while let Some(e) = rx.recv_timeout(Duration::from_millis(300)) {
                if let Event::Frame(_, f) = e {
                    kinds.push(matches!(classify(&f.data), Message::Redirect(_)));
                }
            }
            assert!(!kinds.is_empty() && !kinds.contains(&true), "{kinds:?}");
        }
    }
}
