//! Supervisor integration tests against STUB children: a fake `supersonic`
//! (shell script that publishes the shm readiness file and idles) and a fake
//! Spider (a ruby one-liner that sleeps). Exercises the whole boot →
//! children_running → shutdown_verified lifecycle without the real runtime,
//! plus the boot-timeout failure path. All process spawning is via
//! `std::process::Command` with argument vectors — no shell interpolation.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sonicpi_core::supervisor::Supervisor;
use sonicpi_core::PortId;

fn write_exec(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// Fake app root with a stub engine + stub spider. `publish_shm` controls
/// whether the engine announces readiness.
fn stub_app_root(name: &str, publish_shm: bool) -> PathBuf {
    let root = std::env::temp_dir().join(format!("sonic_oxide_sup_stub_{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("server/native")).unwrap();
    std::fs::create_dir_all(root.join("server/ruby/bin")).unwrap();

    // args: -u <port> [engine opts...]; publish readiness, idle, clean up on TERM.
    let publish = if publish_shm { "touch \"/dev/shm/SuperSonic_$2\"" } else { ":" };
    write_exec(
        &root.join("server/native/supersonic"),
        &format!(
            "#!/bin/sh\ntrap 'rm -f \"/dev/shm/SuperSonic_$2\"; exit 0' TERM\n{publish}\nwhile true; do sleep 1; done\n"
        ),
    );
    // Stub Spider: valid ruby, just idles (killed by shutdown).
    std::fs::write(root.join("server/ruby/bin/spider-server.rb"), "sleep(600)\n").unwrap();
    root
}

/// True when no process command line mentions the given stub marker.
fn no_leftovers(marker: &str) -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", marker])
        .output()
        .map(|o| o.stdout.is_empty())
        .unwrap_or(true)
}

#[test]
fn boots_stub_children_and_shuts_them_down_verified() {
    let root = stub_app_root("ok", true);
    let mut sup = Supervisor::boot(&root).expect("stub boot");

    // Sensible ports + token.
    let scsynth = sup.ports.get(PortId::Scsynth);
    assert!(scsynth > 0);
    assert!(sup.ports.get(PortId::GuiListenToSpider) > 0);
    assert!(sup.ports.token != 0);

    let (spider, engine) = sup.children_running();
    assert!(spider && engine, "both stubs must be running after boot");

    assert!(sup.shutdown_verified(), "stubs must be gone after shutdown");
    let (spider, engine) = sup.children_running();
    assert!(!spider && !engine);

    let _ = std::fs::remove_file(format!("/dev/shm/SuperSonic_{scsynth}"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn boot_times_out_when_the_engine_never_publishes() {
    let root = stub_app_root("timeout", false);
    // Test-only env toggle; no other supervisor boots run concurrently in
    // this test binary (each test uses its own stub root).
    std::env::set_var("SONIC_OXIDE_BOOT_TIMEOUT_SECS", "1");
    let err = Supervisor::boot(&root);
    std::env::remove_var("SONIC_OXIDE_BOOT_TIMEOUT_SECS");
    assert!(err.is_err(), "expected a boot timeout");
    // The half-booted engine stub must have been reaped (no zombies).
    assert!(no_leftovers("sonic_oxide_sup_stub_timeout"), "engine stub survived failed boot");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn shutdown_falls_back_to_sigkill_for_term_ignoring_children() {
    let root = stub_app_root("stubborn", true);
    // Children that ignore TERM entirely: only SIGKILL removes them.
    write_exec(
        &root.join("server/native/supersonic"),
        "#!/bin/sh\ntrap '' TERM\ntouch \"/dev/shm/SuperSonic_$2\"\nwhile true; do sleep 1; done\n",
    );
    std::fs::write(
        root.join("server/ruby/bin/spider-server.rb"),
        "Signal.trap('TERM') {}\nsleep(600)\n",
    )
    .unwrap();

    let mut sup = Supervisor::boot(&root).expect("stub boot");
    let scsynth = sup.ports.get(PortId::Scsynth);
    assert!(
        sup.shutdown_verified(),
        "SIGKILL fallback must reap children that ignore TERM"
    );
    let _ = std::fs::remove_file(format!("/dev/shm/SuperSonic_{scsynth}"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn drop_reaps_children() {
    let root = stub_app_root("drop", true);
    let scsynth;
    {
        let mut sup = Supervisor::boot(&root).expect("stub boot");
        scsynth = sup.ports.get(PortId::Scsynth);
        let (spider, engine) = sup.children_running();
        assert!(spider && engine);
        // sup dropped here without an explicit shutdown.
    }
    // Drop runs shutdown: give the TERM traps a moment, then verify.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(no_leftovers("sonic_oxide_sup_stub_drop"), "children survived Drop");
    let _ = std::fs::remove_file(format!("/dev/shm/SuperSonic_{scsynth}"));
    let _ = std::fs::remove_dir_all(&root);
}
