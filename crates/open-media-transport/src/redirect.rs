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
use crate::receiver::{Event, Receiver, ReceiverConfig};

/// The message for `address`. libomtnet writes it with .NET's
/// `XmlTextWriter` (`OMTRedirect.cs:181-194`), which produced
/// `<OMTRedirect NewAddress="…" />` on one line with one space before `/>`
/// (captured, `docs/evidence/2026-09-23-addressing`). `&`, `<`, `>` and `"`
/// are escaped here; how .NET escapes other characters has not been captured.
pub fn to_xml(address: &str) -> String {
    format!(r#"<OMTRedirect NewAddress="{}" />"#, escape_attr(address))
}

/// The `NewAddress` of a redirect message: `Some("")` for a cancel, `None`
/// when the payload is not a redirect or the attribute cannot be read.
/// libomtnet uses an XML parser; this reads just that one attribute of the
/// root element, in either quote style, and decodes the five predefined
/// entities and numeric character references. A trailing NUL is tolerated.
pub fn parse(payload: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(payload).ok()?;
    let s = s.trim_end_matches('\0');
    let mut attrs = s.strip_prefix("<OMTRedirect")?;
    loop {
        let t = attrs.trim_start();
        if t.len() == attrs.len() || t.is_empty() || t.starts_with(['/', '>']) {
            return None; // no whitespace before the attribute, or end of tag
        }
        let eq = t.find('=')?;
        let name = t[..eq].trim_end();
        if name.contains(['/', '>', '<']) {
            return None;
        }
        let after = t[eq + 1..].trim_start();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let close = after[1..].find(quote)? + 1;
        let value = &after[1..close];
        if name == "NewAddress" {
            return unescape(value);
        }
        attrs = &after[close + 1..];
    }
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let semi = rest[i..].find(';')? + i;
        let ent = &rest[i + 1..semi];
        let ch = match ent {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let n = if let Some(h) = ent.strip_prefix("#x") {
                    u32::from_str_radix(h, 16).ok()?
                } else {
                    ent.strip_prefix('#')?.parse().ok()?
                };
                char::from_u32(n)?
            }
        };
        out.push(ch);
        rest = &rest[semi + 1..];
    }
    if rest.contains('<') {
        return None;
    }
    out.push_str(rest);
    Some(out)
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
                    follow_redirects: false,
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
                            if let Some(a) = parse(x) {
                                on_redirect(a);
                            }
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
            let _ = h.join();
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
        assert_eq!(parse(b"<OMTRedirectNewAddress=\"a\" />"), None);
        assert_eq!(parse(b"<OMTInfo NewAddress=\"a\" />"), None);
    }

    mod end_to_end {
        use crate::receiver::{Event, Receiver, ReceiverConfig};
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

        #[test]
        fn no_redirect_message_on_connect_after_a_clear() {
            use crate::command::{classify, Message};
            let (a, b) = (sender("a"), sender("b"));
            a.set_redirect(Some(&url(&b)));
            a.set_redirect(None);
            let cfg = ReceiverConfig {
                audio: false,
                reconnect: false,
                follow_redirects: false,
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
