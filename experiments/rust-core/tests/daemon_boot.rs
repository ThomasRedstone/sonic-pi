//! Integration test for process::Daemon using a stub daemon that prints the
//! same handshake line the real daemon.rb prints, then sleeps.

use std::io::Write;
use std::path::PathBuf;

use sonicpi_core::process::Daemon;
use sonicpi_core::PortId;

#[test]
fn boots_a_stub_daemon_and_parses_handshake() {
    // A stub "daemon" in ruby if present, else sh — prints handshake, sleeps.
    let dir = std::env::temp_dir().join("sonicpi_core_daemon_test");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("stub_daemon.sh");
    let mut f = std::fs::File::create(&script).unwrap();
    writeln!(f, "#!/bin/sh\necho \"29635 29631 29630 29632 29633 660819297\"\nsleep 30").unwrap();
    drop(f);
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();

    let mut d = Daemon::boot(&PathBuf::from("/bin/sh"), &script).expect("boot stub");
    assert_eq!(d.ports.get(PortId::Daemon), 29635);
    assert_eq!(d.ports.get(PortId::GuiListenToSpider), 29631);
    assert_eq!(d.ports.get(PortId::GuiSendToSpider), 29630);
    assert_eq!(d.ports.get(PortId::Scsynth), 29632);
    assert_eq!(d.ports.token, 660819297);
    d.kill(); // must not hang or leave the child running
}

#[test]
fn wait_timeout_reaps_a_self_exiting_daemon_and_times_out_on_a_hung_one() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let dir = std::env::temp_dir().join("sonicpi_core_daemon_test");
    std::fs::create_dir_all(&dir).unwrap();

    // Exits on its own shortly after the handshake — the clean `/daemon/exit`
    // shape. wait_timeout must see it exit well within the window.
    let script = dir.join("exiting_daemon.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\necho \"29635 29631 29630 29632 29633 660819297\"\nsleep 1\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();

    let mut d = Daemon::boot(&PathBuf::from("/bin/sh"), &script).expect("boot stub");
    assert!(d.wait_timeout(Duration::from_secs(5)), "expected clean self-exit");

    // Hung daemon: wait_timeout must give up (and Drop must then reap it).
    let script = dir.join("hung_daemon.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\necho \"29635 29631 29630 29632 29633 660819297\"\nsleep 30\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();

    let mut d = Daemon::boot(&PathBuf::from("/bin/sh"), &script).expect("boot stub");
    assert!(!d.wait_timeout(Duration::from_millis(300)), "hung daemon reported as exited");
    let pid = d.child.id();
    drop(d); // Drop backstop kills + reaps
    let alive = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| !s.contains(") Z "))
        .unwrap_or(false);
    assert!(!alive, "daemon still running after Drop");
}

#[test]
fn boot_fails_cleanly_when_daemon_exits_without_handshake() {
    // Mirrors the real local failure mode: daemon errors out (missing
    // SuperSonic binary) and exits with no stdout line.
    let dir = std::env::temp_dir().join("sonicpi_core_daemon_test");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("failing_daemon.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();

    let err = Daemon::boot(&PathBuf::from("/bin/sh"), &script);
    assert!(err.is_err(), "expected handshake failure, got ports");
}
