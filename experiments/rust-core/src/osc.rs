//! UDP OSC transport — sender + listening server. Mirrors the roles of
//! `OscSender` / `OscServer` in the C++ api (which use kissnet). All traffic is
//! localhost UDP, as in the current build.

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use rosc::{OscMessage, OscPacket};

use crate::CoreError;

/// Fire-and-forget OSC sender to a localhost port.
pub struct UdpOscSender {
    sock: UdpSocket,
}

impl UdpOscSender {
    pub fn to_localhost(port: u16) -> Result<Self, CoreError> {
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        sock.connect((Ipv4Addr::LOCALHOST, port))?;
        Ok(Self { sock })
    }

    pub fn send(&self, m: &OscMessage) -> Result<(), CoreError> {
        let buf = rosc::encoder::encode(&OscPacket::Message(m.clone()))?;
        self.sock.send(&buf)?;
        Ok(())
    }
}

/// A background UDP listener that decodes OSC and hands each message to a
/// callback. Stops and joins its thread on drop.
pub struct OscServer {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    port: u16,
}

impl OscServer {
    /// Bind `port` (use 0 for an ephemeral port) and spawn the receive loop.
    pub fn start<F>(port: u16, mut on_msg: F) -> Result<Self, CoreError>
    where
        F: FnMut(OscMessage) + Send + 'static,
    {
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, port))?;
        let bound = sock.local_addr()?.port();
        // Timeout so the loop can observe the stop flag without a wake packet.
        sock.set_read_timeout(Some(Duration::from_millis(200)))?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 65_535];
            while !stop_thread.load(Ordering::Relaxed) {
                match sock.recv_from(&mut buf) {
                    Ok((n, _)) => {
                        if let Ok((_, packet)) = rosc::decoder::decode_udp(&buf[..n]) {
                            dispatch(packet, &mut on_msg);
                        }
                    }
                    // Read timeout / transient error: just re-check the flag.
                    Err(_) => {}
                }
            }
        });

        Ok(Self { stop, handle: Some(handle), port: bound })
    }

    /// The actually-bound port (useful when started with 0).
    pub fn port(&self) -> u16 {
        self.port
    }
}

fn dispatch<F: FnMut(OscMessage)>(packet: OscPacket, on_msg: &mut F) {
    match packet {
        OscPacket::Message(m) => on_msg(m),
        OscPacket::Bundle(b) => {
            for inner in b.content {
                dispatch(inner, on_msg);
            }
        }
    }
}

impl Drop for OscServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::OscType;
    use std::sync::mpsc;

    #[test]
    fn sends_and_receives_a_message() {
        let (tx, rx) = mpsc::channel();
        let server = OscServer::start(0, move |m| {
            let _ = tx.send(m);
        })
        .unwrap();
        let sender = UdpOscSender::to_localhost(server.port()).unwrap();
        sender
            .send(&OscMessage {
                addr: "/hello".into(),
                args: vec![OscType::Int(7)],
            })
            .unwrap();

        let got = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("message not received");
        assert_eq!(got.addr, "/hello");
        assert!(matches!(got.args[0], OscType::Int(7)));
    }

    #[test]
    fn survives_garbage_and_bundles_and_stays_alive() {
        let (tx, rx) = mpsc::channel();
        let server = OscServer::start(0, move |m| {
            let _ = tx.send(m);
        })
        .unwrap();

        // Raw garbage datagram: must not kill the server thread.
        let raw = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        raw.send_to(b"definitely not osc", ("127.0.0.1", server.port())).unwrap();

        // A real message afterwards still arrives.
        let sender = UdpOscSender::to_localhost(server.port()).unwrap();
        sender.send(&OscMessage { addr: "/after".into(), args: vec![] }).unwrap();
        let got = rx.recv_timeout(Duration::from_secs(2)).expect("server died on garbage");
        assert_eq!(got.addr, "/after");
    }

    #[test]
    fn binding_a_taken_port_errors() {
        let holder = OscServer::start(0, |_| {}).unwrap();
        assert!(OscServer::start(holder.port(), |_| {}).is_err());
    }
}
