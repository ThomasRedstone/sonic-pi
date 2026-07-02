//! Ports + the daemon handshake. The Ruby daemon prints one whitespace-
//! separated line on stdout; the C++ (`StartBootDaemon`) parses it positionally.

use std::collections::HashMap;

use crate::CoreError;

/// The ports the daemon reports. Names match the Ruby symbols / the C++
/// `SonicPiPortId` enum and cannot change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortId {
    /// Daemon control port (keep-alive, exit).
    Daemon,
    /// Port the GUI/core listens on for messages from Spider.
    GuiListenToSpider,
    /// Port the GUI/core sends to Spider on.
    GuiSendToSpider,
    /// SuperSonic (a.k.a. scsynth) audio-engine port.
    Scsynth,
    /// External OSC cues port (historically "tau"/erlang; now osc-cues).
    TauOscCues,
}

/// Resolved ports + the session token used to authenticate messages.
#[derive(Debug, Clone)]
pub struct Ports {
    map: HashMap<PortId, u16>,
    pub token: i32,
}

impl Ports {
    pub fn get(&self, id: PortId) -> u16 {
        self.map[&id]
    }

    /// Build a Ports table directly (the Rust supervisor allocates its own
    /// ports instead of parsing a daemon handshake).
    pub fn from_parts(
        daemon: u16,
        gui_listen: u16,
        gui_send: u16,
        scsynth: u16,
        osc_cues: u16,
        token: i32,
    ) -> Ports {
        let mut map = HashMap::new();
        map.insert(PortId::Daemon, daemon);
        map.insert(PortId::GuiListenToSpider, gui_listen);
        map.insert(PortId::GuiSendToSpider, gui_send);
        map.insert(PortId::Scsynth, scsynth);
        map.insert(PortId::TauOscCues, osc_cues);
        Ports { map, token }
    }

    /// Parse the daemon's handshake line. Positional, per `StartBootDaemon`:
    ///
    /// ```text
    /// <daemon> <gui_listen_to_spider> <gui_send_to_spider> <scsynth> <tau_osc_cues> <token>
    /// ```
    pub fn parse_daemon_line(line: &str) -> Result<Ports, CoreError> {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 6 {
            return Err(CoreError::Handshake(format!(
                "expected 6 fields, got {}: {line:?}",
                f.len()
            )));
        }
        let port = |i: usize| -> Result<u16, CoreError> {
            f[i].parse::<u16>()
                .map_err(|_| CoreError::Handshake(format!("bad port {:?}", f[i])))
        };

        let mut map = HashMap::new();
        map.insert(PortId::Daemon, port(0)?);
        map.insert(PortId::GuiListenToSpider, port(1)?);
        map.insert(PortId::GuiSendToSpider, port(2)?);
        map.insert(PortId::Scsynth, port(3)?);
        map.insert(PortId::TauOscCues, port(4)?);

        let token = f[5]
            .parse::<i32>()
            .map_err(|_| CoreError::Handshake(format!("bad token {:?}", f[5])))?;

        Ok(Ports { map, token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_handshake_line() {
        let p = Ports::parse_daemon_line("37000 37001 37002 37003 37004 123456789").unwrap();
        assert_eq!(p.get(PortId::Daemon), 37000);
        assert_eq!(p.get(PortId::GuiListenToSpider), 37001);
        assert_eq!(p.get(PortId::GuiSendToSpider), 37002);
        assert_eq!(p.get(PortId::Scsynth), 37003);
        assert_eq!(p.get(PortId::TauOscCues), 37004);
        assert_eq!(p.token, 123456789);
    }

    #[test]
    fn rejects_short_lines() {
        assert!(Ports::parse_daemon_line("1 2 3").is_err());
    }
}
