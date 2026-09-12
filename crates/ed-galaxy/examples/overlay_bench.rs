//! Item 47 bench 3b: the delta-apply acceptance program.
//!
//! Measures, against a base index (synthetic by default, `--base` for a
//! real one):
//!   * overlay wire size for a churn week (adds + updates, corridor-
//!     clustered like real EDDN discoveries), raw and as written (zstd);
//!   * `apply_overlays` wall time and peak RSS;
//!   * the crash contract: a `kill -9` mid-apply leaves the base index
//!     live and untouched, and the retried apply succeeds (resume = redo —
//!     staging is cheap, the base is sacred);
//!   * determinism: two applies of the same chain are byte-identical.
//!
//! CSV goes to stdout (`kind,key,value`); progress and verdicts to stderr.
//!
//!   cargo run --release -p ed-galaxy --example overlay_bench -- \
//!       --synth 2500000 --adds 60000 --updates 15000
//!   cargo run --release -p ed-galaxy --example overlay_bench -- \
//!       --base ~/edda-data/galaxy --edgo routing-overlay-46.edgo
//!
//! `--edgo` skips synthesis and benches a real overlay file (stage 1's
//! output) against `--base` — the form the real-week CSV in the ledger
//! comes from.

use ed_galaxy::overlay::{add_at, update_at, AddRecord, Overlay};
use ed_galaxy::{Galaxy, StarClass, StarClassCode as _};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn arg(name: &str) -> Option<String> {
    let mut args = std::env::args();
    while let Some(a) = args.next() {
        if a == name {
            return args.next();
        }
    }
    None
}

#[cfg(unix)]
fn peak_rss_mb() -> f64 {
    // ru_maxrss: bytes on macOS, kilobytes on Linux.
    let mut usage = unsafe { std::mem::zeroed::<libc_rusage>() };
    unsafe { getrusage(0, &mut usage) };
    let raw = usage.ru_maxrss as f64;
    if cfg!(target_os = "macos") {
        raw / 1e6
    } else {
        raw / 1e3
    }
}

/// Windows has no getrusage; the bench still runs there (the RSS rows
/// read 0 — bench on a Unix box for memory numbers).
#[cfg(not(unix))]
fn peak_rss_mb() -> f64 {
    0.0
}

#[cfg(unix)]
#[repr(C)]
struct libc_rusage {
    ru_utime: [u64; 2],
    ru_stime: [u64; 2],
    ru_maxrss: i64,
    _rest: [i64; 13],
}
#[cfg(unix)]
extern "C" {
    fn getrusage(who: i32, usage: *mut libc_rusage) -> i32;
}

#[cfg(unix)]
fn children_peak_rss_mb() -> f64 {
    let mut usage = unsafe { std::mem::zeroed::<libc_rusage>() };
    unsafe { getrusage(-1, &mut usage) }; // RUSAGE_CHILDREN
    usage.ru_maxrss as f64 / if cfg!(target_os = "macos") { 1e6 } else { 1e3 }
}

#[cfg(not(unix))]
fn children_peak_rss_mb() -> f64 {
    0.0
}

/// A synthetic dump big enough to have realistic cell structure.
fn synth_dump(count: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(count * 200);
    out.extend_from_slice(b"[\n");
    for i in 0..count {
        let comma = if i + 1 == count { "" } else { "," };
        // A disk with a dense core: radius grows with sqrt so inner cells
        // are crowded, plus a thin corridor arm the adds will cluster on.
        let angle = (i as f64) * 0.618_034 * std::f64::consts::TAU;
        let radius = 5_000.0 * ((i % 10_000) as f64 / 10_000.0).sqrt();
        let (x, z) = (radius * angle.cos(), radius * angle.sin());
        let y = ((i * 37) % 700) as f64 - 350.0;
        let class = if i % 50 == 0 {
            "Neutron Star"
        } else {
            "G (White-Yellow) Star"
        };
        writeln!(
            out,
            r#"{{"id64":{},"name":"Bench Sector {:05} AA-A d{}","coords":{{"x":{x:.3},"y":{y:.1},"z":{z:.3}}},"bodies":[{{"type":"Star","subType":"{class}","mainStar":true,"distanceToArrival":0}}]}}{comma}"#,
            20_000_000_000u64 + i as u64,
            i % 100_000,
            i % 97,
        )
        .unwrap();
    }
    out.extend_from_slice(b"]\n");
    out
}

/// A churn week against `base`: `updates` records learn a class (drawn
/// evenly), `adds` new systems land clustered around existing stars —
/// the corridor model, since real discoveries hug the frontier of what
/// is already charted.
fn synth_week(base: &Galaxy, adds: usize, updates: usize, seed: u64) -> Overlay {
    let mut records = Vec::with_capacity(adds + updates);
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let count = base.count as u64;
    let mut used = std::collections::HashSet::new();
    for _ in 0..updates {
        let idx = (next() % count) as u32;
        if !used.insert(idx) {
            continue; // one op per identity per overlay
        }
        let pos = base.pos_of(idx);
        records.push(update_at(pos, StarClass::M.code(), 1));
    }
    for k in 0..adds {
        let anchor = base.pos_of((next() % count) as u32);
        // A neighbour within ~20 ly of a charted star: same or adjacent cell.
        let jitter = |v: f32, r: u64| v + ((r % 4_000) as f32 / 100.0) - 20.0;
        let pos = [
            jitter(anchor[0], next()),
            jitter(anchor[1], next()),
            jitter(anchor[2], next()),
        ];
        records.push(add_at(
            pos,
            AddRecord {
                id64: 30_000_000_000 + k as u64,
                class: if k % 8 == 0 {
                    StarClass::Unknown.code()
                } else {
                    StarClass::K.code()
                },
                flags: 1,
                companion: 0,
                name: format!("Frontier {k:06} ZZ-Z d0"),
            },
        ));
    }
    // The writer refuses duplicate identities; drop any collision the
    // jitter produced (vanishingly rare, but the bench must not flake).
    records
        .sort_by(|a, b| (a.cell, a.pos.map(f32::to_bits)).cmp(&(b.cell, b.pos.map(f32::to_bits))));
    records.dedup_by_key(|r| (r.cell, r.pos.map(f32::to_bits)));
    Overlay {
        base_stars_sha256: ed_galaxy::overlay::stars_sha256(&base.dir).unwrap(),
        created_at: 0,
        records,
    }
}

fn dir_bytes(dir: &Path) -> u64 {
    ["stars.bin", "cells.bin", "names.bin", "byname.bin"]
        .iter()
        .map(|n| std::fs::metadata(dir.join(n)).map(|m| m.len()).unwrap_or(0))
        .sum()
}

fn main() {
    // Hidden child mode for the crash test: apply and exit.
    if let Some(base) = arg("--child-apply") {
        let staging = PathBuf::from(arg("--staging").expect("--staging"));
        let edgo = PathBuf::from(arg("--edgo").expect("--edgo"));
        let chain = vec![Overlay::read(&edgo).expect("overlay reads")];
        ed_galaxy::overlay::apply_overlays(Path::new(&base), &chain, &staging)
            .expect("child apply");
        return;
    }

    let scratch = tempfile::tempdir().expect("scratch dir");
    let base_dir = match arg("--base") {
        Some(dir) => PathBuf::from(shellexpand_home(&dir)),
        None => {
            let systems: usize = arg("--synth")
                .and_then(|v| v.parse().ok())
                .unwrap_or(2_500_000);
            eprintln!("building a {systems}-system synthetic base…");
            let dir = scratch.path().join("base");
            let dump = synth_dump(systems);
            let started = Instant::now();
            ed_galaxy::import::import_reader(
                Box::new(std::io::Cursor::new(dump)),
                &dir,
                &mut |_| {},
            )
            .expect("synthetic import");
            eprintln!("  built in {:.1}s", started.elapsed().as_secs_f64());
            dir
        }
    };
    let base = Galaxy::open(&base_dir).expect("opening the base index");
    println!("base,systems,{}", base.count);
    println!("base,bytes,{}", dir_bytes(&base_dir));

    let edgo_path = scratch.path().join("week.edgo");
    match arg("--edgo") {
        Some(real) => {
            std::fs::copy(shellexpand_home(&real), &edgo_path).expect("copying the real overlay");
        }
        None => {
            let adds: usize = arg("--adds").and_then(|v| v.parse().ok()).unwrap_or(60_000);
            let updates: usize = arg("--updates")
                .and_then(|v| v.parse().ok())
                .unwrap_or(15_000);
            let seed: u64 = arg("--seed").and_then(|v| v.parse().ok()).unwrap_or(47);
            eprintln!("synthesizing a churn week: {adds} adds + {updates} updates…");
            let overlay = synth_week(&base, adds, updates, seed);
            println!("overlay,records,{}", overlay.records.len());
            overlay.write(&edgo_path).expect("writing the overlay");
        }
    }
    let overlay = Overlay::read(&edgo_path).expect("overlay round-trip");
    println!(
        "overlay,wire_bytes,{}",
        std::fs::metadata(&edgo_path).unwrap().len()
    );

    // ---- the timed apply ----
    let staging = scratch.path().join("staging");
    let started = Instant::now();
    let stats =
        ed_galaxy::overlay::apply_overlays(&base_dir, std::slice::from_ref(&overlay), &staging)
            .expect("the apply");
    let wall = started.elapsed().as_secs_f64();
    println!("apply,wall_s,{wall:.3}");
    println!("apply,peak_rss_mb,{:.0}", peak_rss_mb());
    println!("apply,systems,{}", stats.systems);
    println!("apply,adds,{}", stats.adds);
    println!("apply,updates,{}", stats.updates);
    println!("apply,out_bytes,{}", dir_bytes(&staging));
    println!(
        "apply,throughput_systems_per_s,{:.0}",
        stats.systems as f64 / wall
    );
    Galaxy::validate_dir(&staging).expect("the merged index validates");
    eprintln!("merged index validates; apply took {wall:.2}s");

    // ---- determinism ----
    let again = scratch.path().join("again");
    ed_galaxy::overlay::apply_overlays(&base_dir, &[overlay], &again).expect("second apply");
    for name in ["stars.bin", "cells.bin", "names.bin", "byname.bin"] {
        assert_eq!(
            std::fs::read(staging.join(name)).unwrap(),
            std::fs::read(again.join(name)).unwrap(),
            "{name} differs between two applies"
        );
    }
    println!("determinism,byte_identical,1");
    eprintln!("two applies byte-identical");

    // ---- the apply's own peak RSS, unpolluted by this process's
    // import/synthesis phases: run it in a clean child and read
    // RUSAGE_CHILDREN after reaping. ----
    let clean_staging = scratch.path().join("clean-child");
    let exe = std::env::current_exe().expect("current exe");
    let status = std::process::Command::new(&exe)
        .arg("--child-apply")
        .arg(&base_dir)
        .arg("--staging")
        .arg(&clean_staging)
        .arg("--edgo")
        .arg(&edgo_path)
        .status()
        .expect("clean child apply");
    assert!(status.success(), "the clean child apply failed");
    let child_rss = children_peak_rss_mb();
    println!("apply,child_peak_rss_mb,{child_rss:.0}");

    // ---- kill -9 mid-apply: the base survives, the retry succeeds ----
    let base_hash_before = ed_galaxy::overlay::stars_sha256(&base_dir).unwrap();
    let crash_staging = scratch.path().join("crash");
    let exe = std::env::current_exe().expect("current exe");
    let mut child = std::process::Command::new(&exe)
        .arg("--child-apply")
        .arg(&base_dir)
        .arg("--staging")
        .arg(&crash_staging)
        .arg("--edgo")
        .arg(&edgo_path)
        .spawn()
        .expect("spawning the child apply");
    // Let it get into the merge, then kill it without ceremony
    // (SIGKILL on Unix, TerminateProcess on Windows — no cleanup runs).
    std::thread::sleep(std::time::Duration::from_millis((wall * 400.0) as u64));
    child.kill().expect("killing the child apply");
    let status = child.wait().expect("child reaped");
    assert!(!status.success(), "the child was supposed to die");
    assert_eq!(
        ed_galaxy::overlay::stars_sha256(&base_dir).unwrap(),
        base_hash_before,
        "kill -9 mid-apply must leave the base index untouched"
    );
    Galaxy::open(&base_dir)
        .expect("the base still opens")
        .validate()
        .expect("and validates");
    println!("crash,base_untouched,1");
    // Resume semantics: redo. The retried apply clears the partial
    // staging itself (apply_overlay_chain does the same in the app).
    if crash_staging.exists() {
        std::fs::remove_dir_all(&crash_staging).unwrap();
    }
    let overlay = Overlay::read(&edgo_path).unwrap();
    ed_galaxy::overlay::apply_overlays(&base_dir, &[overlay], &crash_staging).expect("the retry");
    Galaxy::validate_dir(&crash_staging).expect("the retried index validates");
    println!("crash,retry_ok,1");
    eprintln!("kill -9 verdict: base untouched, retry green");
}

fn shellexpand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_owned(),
    }
}
