//! Build the star index from a Spansh dump.
//!
//!     cargo run -p ed-galaxy --example import --release -- .data/dumps/galaxy_populated.json.gz .data/galaxy
//!     curl -sL https://downloads.spansh.co.uk/galaxy.json.gz | gunzip | \
//!         cargo run -p ed-galaxy --example import --release -- - .data/galaxy
//!
//! `-` reads an already-decompressed stream from stdin, so the full 116 GB
//! archive never has to be stored.

use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (src, out) = match args.as_slice() {
        [s, o] => (s.clone(), o.clone()),
        _ => {
            eprintln!("usage: import <dump.json.gz | -> <out_dir>");
            std::process::exit(2);
        }
    };
    let started = std::time::Instant::now();
    let mut progress = |s: &ed_galaxy::import::ImportStats| {
        eprintln!(
            "{:>12} systems  {:>10} lines  {:>6} skipped  {:>8.1} GB read  longest line {:>9} B  {:>6.0} s",
            s.systems, s.lines, s.skipped_lines, s.bytes_in as f64 / 1e9, s.longest_line, started.elapsed().as_secs_f64()
        );
    };
    let stats = if src == "-" {
        ed_galaxy::import::import_reader(Box::new(std::io::stdin().lock()), Path::new(&out), &mut progress)?
    } else {
        ed_galaxy::import::import(Path::new(&src), Path::new(&out), &mut progress)?
    };
    eprintln!("done in {:.0} s: {stats:?}", started.elapsed().as_secs_f64());
    Ok(())
}
