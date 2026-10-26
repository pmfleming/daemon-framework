// Test-only, transport-neutral qualification support. Include by path from a
// producer's private native fixture; this is deliberately not a runtime API.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub enum Step {
    Read,
    Status,
    Renew,
    Cancel,
    Subscribe,
}
impl Step {
    fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Status => "status",
            Self::Renew => "renew",
            Self::Cancel => "cancel",
            Self::Subscribe => "subscribe",
        }
    }
}

/// One independently owned connection, backed by an existing native client.
/// Steps MUST validate domain responses/events, not just receive any reply.
/// Renew cancels ONLY this adapter's subscription and establishes a fresh one.
/// Status is read-only: unknown outcomes must not dispatch or replay work.
/// Return actual encoded request/response bytes (including LF for JSONL).
/// Count all wire requests/events in composite renewal, including its baseline.
#[derive(Clone, Copy, Default)]
pub struct Traffic {
    pub requests: u64,
    pub events: u64,
    pub request_bytes: u64,
    pub response_bytes: u64,
}
impl Traffic {
    fn add(&mut self, other: Self) -> Result<()> {
        self.requests = self
            .requests
            .checked_add(other.requests)
            .context("request count overflow")?;
        self.events = self
            .events
            .checked_add(other.events)
            .context("event count overflow")?;
        self.request_bytes = self
            .request_bytes
            .checked_add(other.request_bytes)
            .context("request byte count overflow")?;
        self.response_bytes = self
            .response_bytes
            .checked_add(other.response_bytes)
            .context("response byte count overflow")?;
        Ok(())
    }
}
pub trait Adapter {
    fn step(&mut self, step: Step, payload_bytes: usize) -> impl Future<Output = Result<Traffic>>;
}

#[derive(Clone, Copy)]
pub struct Workload {
    pub warmup_cycles: usize,
    pub sample_cycles: usize,
    pub payload_bytes: usize,
    pub step_deadline: Duration,
    pub total_deadline: Duration,
}
impl Workload {
    fn validate(self, owners: usize) -> Result<()> {
        ensure!((1..=64).contains(&owners), "owner count outside 1..=64");
        ensure!(self.sample_cycles > 0, "no measured cycles");
        let cycles = self
            .warmup_cycles
            .checked_add(self.sample_cycles)
            .context("cycle count overflow")?;
        ensure!(
            cycles <= 10_000 && cycles * owners * 6 <= 1_000_000,
            "sample storage bound exceeded"
        );
        ensure!(
            self.payload_bytes <= 16 * 1024 * 1024,
            "payload bound exceeded"
        );
        ensure!(
            !self.step_deadline.is_zero()
                && !self.total_deadline.is_zero()
                && self.step_deadline <= Duration::from_secs(1800)
                && self.total_deadline <= Duration::from_secs(1800),
            "deadlines must be positive and at most 1800 seconds"
        );
        Ok(())
    }
}

fn distribution(samples: &mut [u64]) -> Result<Value> {
    ensure!(!samples.is_empty(), "empty sample distribution");
    samples.sort_unstable();
    // Nearest rank: rank = ceil(p * n), one-based. Never interpolate.
    let rank = |p: usize| (samples.len() * p).div_ceil(100) - 1;
    Ok(json!({"n":samples.len(), "p50_ns":samples[rank(50)],
        "p99_ns":samples[rank(99)], "min_ns":samples[0],
        "max_ns":samples[samples.len()-1]}))
}

async fn owner<A: Adapter>(adapter: &mut A, workload: Workload) -> Result<Value> {
    let mut samples = BTreeMap::<&str, Vec<u64>>::new();
    let mut total = Traffic::default();
    // Initial establishment is untimed, but still bounded and included in traffic.
    let mut sequence =
        Vec::with_capacity(1 + 6 * (workload.warmup_cycles + workload.sample_cycles));
    sequence.push((Step::Subscribe, false));
    for cycle in 0..workload.warmup_cycles + workload.sample_cycles {
        for step in [
            Step::Read,
            Step::Status,
            Step::Renew,
            Step::Read,
            Step::Cancel,
            Step::Subscribe,
        ] {
            sequence.push((step, cycle >= workload.warmup_cycles));
        }
    }
    for (step, measured) in sequence {
        let start = Instant::now();
        let traffic = tokio::time::timeout(
            workload.step_deadline,
            adapter.step(step, workload.payload_bytes),
        )
        .await
        .with_context(|| format!("{} deadline; no replay", step.name()))??;
        let elapsed = u64::try_from(start.elapsed().as_nanos()).context("duration overflow")?;
        if measured {
            samples.entry(step.name()).or_default().push(elapsed);
        }
        total.add(traffic)?;
    }
    // Final cancellation is deliberately outside measured distributions.
    let end = tokio::time::timeout(
        workload.step_deadline,
        adapter.step(Step::Cancel, workload.payload_bytes),
    )
    .await
    .context("final cancellation deadline; no replay")??;
    total.add(end)?;
    let latency = samples
        .into_iter()
        .map(|(name, mut values)| Ok((name, distribution(&mut values)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    Ok(
        json!({"latency":latency, "traffic_including_setup_warmup_cleanup":{
        "requests":total.requests, "events":total.events,
        "request_bytes":total.request_bytes, "response_bytes":total.response_bytes}}),
    )
}

/// Owners run concurrently; each owner has exactly one step outstanding. There
/// is no reconnect, retry, effect dispatch or unbounded pending-request queue.
/// On error the caller must close all adapters and finish its private fixture.
pub async fn mixed<A: Adapter>(adapters: &mut [A], workload: Workload) -> Result<Value> {
    workload.validate(adapters.len())?;
    let results = tokio::time::timeout(
        workload.total_deadline,
        futures::future::join_all(adapters.iter_mut().map(|a| owner(a, workload))),
    )
    .await
    .context("mixed workload total deadline; no replay")?
    .into_iter()
    .collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"schema_version":1, "schedule":"subscribe; (read,status,renew,read,cancel,subscribe)*cycles; cancel",
        "owners":results, "warmup_cycles_per_owner":workload.warmup_cycles,
        "sample_cycles_per_owner":workload.sample_cycles,
        "payload_bytes":workload.payload_bytes,
        "max_steps_in_flight_per_owner":1,
        "step_deadline_ms":workload.step_deadline.as_millis(),
        "total_deadline_ms":workload.total_deadline.as_millis(),
        "step_deadline_ns":workload.step_deadline.as_nanos(),
        "total_deadline_ns":workload.total_deadline.as_nanos(),
        "latency_scope":"validated complete adapter step; renewal includes cancel and fresh baseline",
        "percentile":"nearest-rank, per owner and step; warmup/setup/cleanup excluded"}),
    )
}

// Only trusted, non-spawning Git/coreutils commands are admitted here. This is
// NOT a general fixture/process-tree launcher. Disable Git's optional helpers at
// the call site. A fixture must separately own/reap all producer/bridge children.
const IDENTITY_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const IDENTITY_DEADLINE: Duration = Duration::from_secs(10);

struct IdentityChild(std::process::Child);
impl Drop for IdentityChild {
    fn drop(&mut self) {
        // Also runs on output/IO/deadline failure and unwinding, before pipe
        // threads join. Never leave a blocked writer or unreaped direct child.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn identity_command(
    command: &mut Command,
    input: &[u8],
    deadline: Duration,
    limit: usize,
) -> Result<Vec<u8>> {
    ensure!(input.len() <= limit, "identity input bound exceeded");
    std::thread::scope(|scope| {
        let mut child = IdentityChild(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .context("identity command")?,
        );
        let mut stdin = child.0.stdin.take().context("identity stdin")?;
        let stdout = child.0.stdout.take().context("identity stdout")?;
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        scope.spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout.take(limit as u64 + 1).read_to_end(&mut bytes);
            let _ = sent.send(result.map(|_| bytes));
        });
        let writer = scope.spawn(move || stdin.write_all(input));
        let start = Instant::now();
        // The reader sends exactly one completion. Collect its bounded result
        // first, then await process exit under the SAME deadline. EOF alone is
        // not success: a child can close stdout and remain alive or fail later.
        let bytes = received
            .recv_timeout(deadline)
            .context("identity output unavailable (deadline/reader)")?
            .context("identity stdout")?;
        ensure!(bytes.len() <= limit, "identity output bound exceeded");
        loop {
            ensure!(start.elapsed() < deadline, "identity command deadline");
            if let Some(status) = child.0.try_wait()? {
                ensure!(status.success(), "identity command failed");
                writer
                    .join()
                    .expect("identity input thread")
                    .context("identity stdin")?;
                return Ok(bytes);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    })
}

fn output(command: &mut Command) -> Result<Vec<u8>> {
    identity_command(command, &[], IDENTITY_DEADLINE, IDENTITY_OUTPUT_LIMIT)
}
fn sha256(bytes: &[u8]) -> Result<String> {
    hash_field(&identity_command(
        &mut Command::new("sha256sum"),
        bytes,
        IDENTITY_DEADLINE,
        IDENTITY_OUTPUT_LIMIT,
    )?)
}
fn hash_field(bytes: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(bytes)?;
    let hash = text.split_whitespace().next().context("missing hash")?;
    ensure!(
        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid SHA256"
    );
    Ok(hash.into())
}
pub fn binary_identity(path: &Path) -> Result<Value> {
    let path = path.canonicalize()?;
    let bytes = output(Command::new("sha256sum").arg(&path))?;
    Ok(json!({"path":path,"sha256":hash_field(&bytes)?}))
}

/// Final measurements require clean committed inputs. For correctness runs, HEAD
/// plus the binary diff hash identifies tracked edits; untracked inputs are never
/// accepted (stage new inputs with git add -N). Capture every sibling dependency.
pub fn source_identity(root: &Path, require_clean: bool) -> Result<Value> {
    let root = root.canonicalize()?;
    let git = |args: &[&str]| {
        output(
            Command::new("git")
                // Repository config is trusted input, but ambient Git redirects,
                // global config, tracing paths and optional index writes are not.
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .args([
                    "--no-pager",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.untrackedCache=false",
                ])
                .arg("-C")
                .arg(&root)
                .args(args),
        )
    };
    let text = |bytes: Vec<u8>| -> Result<String> { Ok(String::from_utf8(bytes)?.trim().into()) };
    ensure!(
        Path::new(&text(git(&["rev-parse", "--show-toplevel"])?)?).canonicalize()? == root,
        "source identity requires the repository root"
    );
    ensure!(
        git(&["ls-files", "--others", "--exclude-standard"])?.is_empty(),
        "untracked source inputs; stage before building"
    );
    let status = git(&["status", "--porcelain=v1", "--untracked-files=no"])?;
    let dirty = !status.is_empty();
    ensure!(
        !require_clean || !dirty,
        "matched release source must be clean"
    );
    let commit = git(&["rev-parse", "HEAD"])?;
    let tree = git(&["rev-parse", "HEAD^{tree}"])?;
    let diff = git(&[
        "diff",
        "--binary",
        "--no-ext-diff",
        "--no-textconv",
        "HEAD",
        "--",
    ])?;
    // Fence ordinary concurrent edits/commits. As with the matching build tool,
    // callers must freeze inputs; an adversarial edit-and-restore is not covered.
    ensure!(
        commit == git(&["rev-parse", "HEAD"])?
            && status == git(&["status", "--porcelain=v1", "--untracked-files=no"])?
            && diff
                == git(&[
                    "diff",
                    "--binary",
                    "--no-ext-diff",
                    "--no-textconv",
                    "HEAD",
                    "--"
                ])?
            && git(&["ls-files", "--others", "--exclude-standard"])?.is_empty(),
        "source changed during identity capture"
    );
    Ok(
        json!({"path":root, "commit":text(commit)?, "tree":text(tree)?, "dirty":dirty,
        "tracked_diff_sha256":sha256(&diff)?}),
    )
}

fn parse_memory(text: &str) -> Result<BTreeMap<String, u64>> {
    let mut result = BTreeMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(key) = fields.next().map(|s| s.trim_end_matches(':')) else {
            continue;
        };
        if matches!(
            key,
            "Rss" | "Pss" | "Private_Clean" | "Private_Dirty" | "Private_Hugetlb"
        ) {
            let value = fields
                .next()
                .context("missing memory value")?
                .parse::<u64>()?;
            ensure!(fields.next() == Some("kB"), "unexpected smaps unit");
            ensure!(fields.next().is_none(), "trailing smaps fields");
            ensure!(
                result
                    .insert(
                        key.into(),
                        value.checked_mul(1024).context("memory overflow")?
                    )
                    .is_none(),
                "duplicate memory field"
            );
        }
    }
    for key in ["Rss", "Pss", "Private_Clean", "Private_Dirty"] {
        ensure!(
            result.contains_key(key),
            "missing {key}; never substitute zero"
        );
    }
    let private = result["Private_Clean"]
        .checked_add(result["Private_Dirty"])
        .and_then(|n| n.checked_add(*result.get("Private_Hugetlb").unwrap_or(&0)))
        .context("private memory overflow")?;
    result.insert("Private_Total".into(), private);
    Ok(result)
}
fn start_ticks(stat: &str) -> Result<u64> {
    stat.rsplit_once(')')
        .context("invalid proc stat")?
        .1
        .split_whitespace()
        .nth(19)
        .context("missing start ticks")?
        .parse()
        .context("start ticks")
}

/// Explicit caller-owned live PIDs only, held unreaped by the fixture. No global
/// process discovery. Caller documents included/excluded producer/driver/bridge
/// scope and takes paired snapshots at the same workload phase on each path.
/// Failure, disappearance or PID identity change is NOT a zero-memory sample.
pub fn memory_snapshot(pids: &[u32]) -> Result<Value> {
    ensure!(!pids.is_empty(), "empty memory scope");
    let mut seen = std::collections::BTreeSet::new();
    let mut totals = BTreeMap::<String, u64>::new();
    let mut processes = Vec::new();
    for &pid in pids {
        ensure!(seen.insert(pid), "duplicate PID");
        let root = format!("/proc/{pid}");
        let before = start_ticks(&std::fs::read_to_string(format!("{root}/stat"))?)?;
        let executable = std::fs::read_link(format!("{root}/exe"))?;
        let values = parse_memory(&std::fs::read_to_string(format!("{root}/smaps_rollup"))?)?;
        let after = start_ticks(&std::fs::read_to_string(format!("{root}/stat"))?)?;
        ensure!(before == after, "PID changed during memory snapshot");
        ensure!(
            executable == std::fs::read_link(format!("{root}/exe"))?,
            "executable changed during memory snapshot"
        );
        for (key, value) in &values {
            let sum = totals.entry(key.clone()).or_default();
            *sum = sum.checked_add(*value).context("scope memory overflow")?;
        }
        processes
            .push(json!({"pid":pid,"start_ticks":before,"executable":executable,"bytes":values}));
    }
    Ok(json!({"processes":processes,"total_bytes":totals,
        "scope":"explicit fixture-owned PID list; point-in-time, not peak",
        "warning":"summed RSS double-counts shared pages; PSS depends on concurrent sharers"}))
}

#[cfg(test)]
#[path = "qualification_identity_tests.rs"]
mod identity_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn percentiles_memory_and_bounds_are_explicit() {
        assert_eq!(distribution(&mut [1, 2, 3, 4]).unwrap()["p50_ns"], 2);
        let mut values = (1..=100).rev().collect::<Vec<_>>();
        assert_eq!(distribution(&mut values).unwrap()["p99_ns"], 99);
        assert!(distribution(&mut []).is_err());
        let memory = parse_memory("Rss: 10 kB\nPss: 8 kB\nPrivate_Clean: 2 kB\nPrivate_Dirty: 3 kB\nPrivate_Hugetlb: 1 kB\n").unwrap();
        assert_eq!(memory["Private_Total"], 6144);
        assert!(parse_memory("Rss: 1 kB").is_err());
        assert!(parse_memory("Rss: 1 MB").is_err());
        assert!(parse_memory("Rss: 1 kB\nRss: 2 kB").is_err());
        let mut traffic = Traffic {
            requests: u64::MAX,
            ..Default::default()
        };
        assert!(
            traffic
                .add(Traffic {
                    requests: 1,
                    ..Default::default()
                })
                .is_err()
        );
        assert_eq!(
            sha256(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let workload = Workload {
            warmup_cycles: 0,
            sample_cycles: 1,
            payload_bytes: 16,
            step_deadline: Duration::from_secs(1),
            total_deadline: Duration::from_secs(5),
        };
        assert!(workload.validate(0).is_err());
        assert!(workload.validate(65).is_err());
        assert!(
            Workload {
                payload_bytes: 16 * 1024 * 1024 + 1,
                ..workload
            }
            .validate(1)
            .is_err()
        );
        assert!(
            Workload {
                step_deadline: Duration::ZERO,
                ..workload
            }
            .validate(1)
            .is_err()
        );
        assert!(
            Workload {
                total_deadline: Duration::MAX,
                ..workload
            }
            .validate(1)
            .is_err()
        );
        assert!(
            Workload {
                step_deadline: Duration::from_secs(1801),
                ..workload
            }
            .validate(1)
            .is_err()
        );
        assert!(
            Workload {
                sample_cycles: usize::MAX,
                ..workload
            }
            .validate(1)
            .is_err()
        );
        assert!(
            Workload {
                sample_cycles: 0,
                ..workload
            }
            .validate(1)
            .is_err()
        );
        assert!(
            Workload {
                sample_cycles: 10_000,
                ..workload
            }
            .validate(64)
            .is_err()
        );
    }
}
