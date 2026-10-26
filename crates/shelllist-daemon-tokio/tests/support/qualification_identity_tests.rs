//! Private provenance vectors; no host repository/config mutation or children
//! with descendants. The shell cases use builtins only and retain no output.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "qualification-identity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn git(&self, args: &[&str]) {
        output(
            Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "Private fixture")
                .env("GIT_AUTHOR_EMAIL", "fixture@invalid")
                .env("GIT_COMMITTER_NAME", "Private fixture")
                .env("GIT_COMMITTER_EMAIL", "fixture@invalid"),
        )
        .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn source_identity_requires_complete_clean_or_identified_tracked_inputs() {
    let fixture = Fixture::new();
    fixture.git(&["init", "--quiet", "--template="]);
    std::fs::write(fixture.0.join(".gitignore"), "ignored\n").unwrap();
    std::fs::write(fixture.0.join("tracked"), "first\n").unwrap();
    fixture.git(&["add", ".gitignore", "tracked"]);
    fixture.git(&["commit", "--quiet", "-m", "private baseline"]);
    let clean = source_identity(&fixture.0, true).unwrap();
    assert_eq!(clean["dirty"], false);
    assert_eq!(clean["tracked_diff_sha256"], sha256(b"").unwrap());
    std::fs::write(fixture.0.join("ignored"), "not a source input").unwrap();
    assert_eq!(source_identity(&fixture.0, true).unwrap(), clean);
    std::fs::write(fixture.0.join("new"), "new source\n").unwrap();
    assert!(
        source_identity(&fixture.0, false)
            .unwrap_err()
            .to_string()
            .contains("untracked")
    );
    fixture.git(&["add", "-N", "new"]);
    let intent = source_identity(&fixture.0, false).unwrap();
    assert_eq!(intent["dirty"], true);
    assert_ne!(intent["tracked_diff_sha256"], clean["tracked_diff_sha256"]);
    assert!(source_identity(&fixture.0, true).is_err());
    fixture.git(&["add", "new"]);
    assert_eq!(
        source_identity(&fixture.0, false).unwrap()["tracked_diff_sha256"],
        intent["tracked_diff_sha256"]
    );
    std::fs::write(fixture.0.join("tracked"), "second\n").unwrap();
    let edited = source_identity(&fixture.0, false).unwrap();
    assert_ne!(edited["tracked_diff_sha256"], intent["tracked_diff_sha256"]);
    let subdir = fixture.0.join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    assert!(
        source_identity(&subdir, false)
            .unwrap_err()
            .to_string()
            .contains("repository root")
    );
    fixture.git(&["add", "tracked"]);
    fixture.git(&["commit", "--quiet", "-m", "private next"]);
    let next = source_identity(&fixture.0, true).unwrap();
    assert_ne!(next["commit"], clean["commit"]);
    assert_ne!(next["tree"], clean["tree"]);
    assert_eq!(next["tracked_diff_sha256"], clean["tracked_diff_sha256"]);
}

#[test]
fn binary_hashes_and_unavailable_memory_never_become_zero_evidence() {
    let fixture = Fixture::new();
    let path = fixture.0.join("fixture-binary");
    std::fs::write(&path, "abc").unwrap();
    let first = binary_identity(&path).unwrap();
    assert_eq!(first["sha256"], sha256(b"abc").unwrap());
    std::fs::write(&path, "different").unwrap();
    assert_ne!(binary_identity(&path).unwrap()["sha256"], first["sha256"]);
    assert!(binary_identity(&fixture.0.join("absent")).is_err());
    assert!(hash_field(b"not-a-hash").is_err());
    assert!(parse_memory("Rss: 1 kB garbage").is_err());
    assert!(parse_memory("Rss: 18446744073709551615 kB").is_err());
    assert!(
        parse_memory("Rss: 0 kB\nPss: 0 kB\nPrivate_Clean: 0 kB\nPrivate_Dirty: 0 kB\n").is_ok()
    );
    assert!(memory_snapshot(&[]).is_err());
    // The only missing PID queried is an impossible Linux PID, not a discovered
    // host process. Live snapshots in the socket self-test use our own PID only.
    assert!(memory_snapshot(&[u32::MAX]).is_err());
    assert_eq!(
        start_ticks("123 (a name ) with parens) S 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 42").unwrap(),
        42
    );
    assert!(start_ticks("invalid").is_err());
}

#[test]
fn identity_commands_bound_output_failures_and_blocked_input_and_reap_children() {
    assert!(output(Command::new("sh").args(["-c", "exit 7"])).is_err());
    let oversized = identity_command(
        Command::new("sh").args(["-c", "while :; do printf '0123456789'; done"]),
        &[],
        Duration::from_secs(2),
        64,
    )
    .unwrap_err();
    assert!(oversized.to_string().contains("output bound"));
    let fixture = Fixture::new();
    let pid_file = fixture.0.join("pid");
    // Builtins only: neither child reads stdin. Test both a blocked output
    // reader and early stdout EOF with a still-live child. Both must time out,
    // kill/reap the child and release the bounded input writer, never succeed.
    for script in [
        "printf '%s' \"$$\" > \"$1\"; while :; do :; done",
        "printf '%s' \"$$\" > \"$1\"; exec 1>&-; while :; do :; done",
    ] {
        let error = identity_command(
            Command::new("sh")
                .args(["-c", script, "private-fixture"])
                .arg(&pid_file),
            &vec![b'x'; 1024 * 1024],
            Duration::from_millis(100),
            1024 * 1024,
        )
        .unwrap_err();
        assert!(error.to_string().contains("deadline"));
        let pid = std::fs::read_to_string(&pid_file)
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "owned child must be reaped"
        );
    }
}
