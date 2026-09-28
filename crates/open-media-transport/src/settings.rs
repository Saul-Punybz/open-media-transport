//! libomtnet's `settings.xml`: a machine-wide file that names a discovery
//! server and the ports senders use (`OMTSettings.cs:34-47`).
//!
//! The file lives in `$OMT_STORAGE_PATH`, or else in `~/.OMT` on macOS and
//! Linux and `%ProgramData%\OMT` on Windows, where libomtnet does not look
//! at `OMT_STORAGE_PATH` (`OMTPlatform.cs:72-77`, `mac/MacPlatform.cs:75-84`,
//! `linux/LinuxPlatform.cs:67-71`, `win32/Win32Platform.cs:55-58`). It is an
//! XML document whose root element holds one child element per setting:
//!
//! ```xml
//! <Settings>
//!   <DiscoveryServer>omt://server:6399</DiscoveryServer>
//!   <NetworkPortStart>7000</NetworkPortStart>
//!   <NetworkPortEnd>7100</NetworkPortEnd>
//! </Settings>
//! ```
//!
//! libomtnet reads it in every application: `DiscoveryServer` replaces DNS-SD
//! (`OMTDiscovery.cs:56-60`) and the port range is where senders listen
//! (`OMTSend.cs:100-101`). This crate reads it only when asked, with
//! [`Settings::load`] and [`crate::sender::SenderConfig::from_settings`], so
//! a program's behaviour never changes behind its back; the `omt` tool always
//! does. Nothing here writes the file.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use crate::sender::DEFAULT_PORTS;

/// The settings libomtnet reads, with its defaults for what is missing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// `DiscoveryServer`: an `omt://host:port` URL, or `None` for DNS-SD.
    pub discovery_server: Option<String>,
    /// `NetworkPortStart`, if set to a valid port.
    pub network_port_start: Option<u16>,
    /// `NetworkPortEnd`, if set to a valid port.
    pub network_port_end: Option<u16>,
}

impl Settings {
    /// Where libomtnet looks for `settings.xml` on this system, if a home or
    /// data folder can be found.
    pub fn path() -> Option<PathBuf> {
        storage_path().map(|d| d.join("settings.xml"))
    }

    /// Reads [`Settings::path`]. A missing or unreadable file, or one that is
    /// not well-formed XML, gives the defaults, as in libomtnet
    /// (`OMTSettings.cs:68-92`).
    pub fn load() -> Settings {
        Settings::path()
            .map(|p| Settings::load_from(&p))
            .unwrap_or_default()
    }

    /// Reads a settings file at `path`, with the same fallbacks as
    /// [`Settings::load`].
    pub fn load_from(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .map(|s| Settings::from_xml(&s))
            .unwrap_or_default()
    }

    /// Parses the file's contents. Each setting is the text of the first
    /// child of the root element with that name (`SelectSingleNode`,
    /// `OMTSettings.cs:104-118`); an empty one counts as unset. A port that
    /// .NET's `int.TryParse` would reject, or that is outside 1..=65535, is
    /// unset.
    pub fn from_xml(xml: &str) -> Settings {
        let Ok(doc) = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}')) else {
            return Settings::default();
        };
        let get = |key: &str| {
            doc.root_element()
                .children()
                .find(|n| n.has_tag_name(key))
                .map(|n| {
                    n.descendants()
                        .filter(|d| d.is_text())
                        .filter_map(|d| d.text())
                        .collect::<String>()
                })
                .filter(|s| !s.is_empty())
        };
        let port = |key: &str| {
            get(key)
                .and_then(|s| s.trim().parse::<i32>().ok())
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p != 0)
        };
        Settings {
            discovery_server: get("DiscoveryServer"),
            network_port_start: port("NetworkPortStart"),
            network_port_end: port("NetworkPortEnd"),
        }
    }

    /// The ports senders try: the configured range, with libomtnet's
    /// defaults for a missing end (`OMTConstants.cs:64-65`).
    pub fn ports(&self) -> RangeInclusive<u16> {
        self.network_port_start.unwrap_or(*DEFAULT_PORTS.start())
            ..=self.network_port_end.unwrap_or(*DEFAULT_PORTS.end())
    }
}

fn storage_path() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("ProgramData").map(|d| PathBuf::from(d).join("OMT"));
    }
    if let Some(p) = std::env::var_os("OMT_STORAGE_PATH").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".OMT"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_libomtnet_reads() {
        let s = Settings::from_xml(
            "<?xml version=\"1.0\"?>\n<Settings>\n  <DiscoveryServer>omt://srv:6399</DiscoveryServer>\n  <NetworkPortStart> 7000 </NetworkPortStart>\n  <NetworkPortEnd>7100</NetworkPortEnd>\n  <Other>x</Other>\n</Settings>",
        );
        assert_eq!(s.discovery_server.as_deref(), Some("omt://srv:6399"));
        assert_eq!(s.ports(), 7000..=7100);
    }

    #[test]
    fn missing_or_bad_values_fall_back() {
        assert_eq!(Settings::from_xml("not xml"), Settings::default());
        assert_eq!(Settings::from_xml("<Settings/>").ports(), DEFAULT_PORTS);
        let s = Settings::from_xml(
            "<Settings><DiscoveryServer></DiscoveryServer><NetworkPortStart>x</NetworkPortStart><NetworkPortEnd>70000</NetworkPortEnd></Settings>",
        );
        assert_eq!(s, Settings::default());
        // Any root element name, as `DocumentElement` (`OMTSettings.cs:79`).
        let s = Settings::from_xml("<Root><NetworkPortStart>7000</NetworkPortStart></Root>");
        assert_eq!((*s.ports().start(), *s.ports().end()), (7000, 6600));
    }

    #[test]
    fn loads_from_a_file_and_tolerates_none() {
        let dir = std::env::temp_dir().join(format!("omt-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.xml");
        std::fs::write(
            &file,
            "\u{feff}<Settings><DiscoveryServer>omt://a:1</DiscoveryServer></Settings>",
        )
        .unwrap();
        assert_eq!(
            Settings::load_from(&file).discovery_server.as_deref(),
            Some("omt://a:1")
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(Settings::load_from(&file), Settings::default());
    }
}
