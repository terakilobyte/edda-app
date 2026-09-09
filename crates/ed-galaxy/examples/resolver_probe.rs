//! Replays the client's routing-index resolution against a data dir and
//! prints every step — built to chase the 2026-09-03 prod launch that
//! seeded the bundled bubble despite a valid galaxy-51 on disk.
//!
//!     cargo run -p ed-galaxy --example resolver_probe -- <data_dir>

fn main() {
    let data_dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let sync = data_dir.join("routing-sync.json");
    match std::fs::read(&sync) {
        Ok(bytes) => {
            println!("read {}: {} bytes", sync.display(), bytes.len());
            match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(v) => {
                    let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("<missing>").to_string();
                    println!("parsed: version={version:?} systems={:?}", v.get("systems"));
                    let dir = data_dir.join(format!("galaxy-{version}"));
                    for p in ed_galaxy::Galaxy::paths(&dir) {
                        println!("  {} is_file={}", p.display(), p.is_file());
                    }
                    println!("Galaxy::exists({}) = {}", dir.display(), ed_galaxy::Galaxy::exists(&dir));
                    match ed_galaxy::Galaxy::open(&dir) {
                        Ok(g) => println!("Galaxy::open OK, systems={}", g.count),
                        Err(e) => println!("Galaxy::open FAILED: {e:#}"),
                    }
                }
                Err(e) => println!("serde parse FAILED: {e}"),
            }
        }
        Err(e) => println!("read {} FAILED: {e}", sync.display()),
    }
}
