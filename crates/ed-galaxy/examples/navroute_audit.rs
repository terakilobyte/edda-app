//! Audit a route's `StarClass` against the routing index: for every hop a
//! `navroute_tap` wrote, the index's main-star class (the dump's scanned
//! arrival star, plus the overlay's teachings) beside the class the game
//! reported on the route. The measurement before NavRoute teaches anything
//! again (2026-09-10: taught blindly, it changed 27 % of touched classes,
//! demoting 6 neutron / white-dwarf primaries and promoting 151 systems to
//! a boost class they do not have).
//!
//!     navroute_audit <index_dir> <hops.tsv>
//!
//! Unique systems by id64 (the last class seen wins; systems whose own hops
//! disagree are counted). Prints how many systems the index lacks (adds),
//! how many it holds as Unknown (teachable), and for the rest a confusion
//! table of route class against index class with the precision of the
//! neutron and white-dwarf claims -- the numbers that decide whether an
//! Unknown may be taught from a route.
use ed_galaxy::{Galaxy, StarClass, StarClassCode};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(args.first().map(String::as_str).expect("usage: navroute_audit <index_dir> <hops.tsv>"));
    let tsv = args.get(1).map(String::as_str).expect("usage: navroute_audit <index_dir> <hops.tsv>");
    let g = Galaxy::open(dir)?;
    // id64 -> (name, route class letter, self-disagreement)
    let mut systems: HashMap<u64, (String, String, bool)> = HashMap::new();
    let (mut lines, mut blank) = (0u64, 0u64);
    for line in std::fs::read_to_string(tsv)?.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 4 {
            continue;
        }
        lines += 1;
        let Ok(addr) = f[1].parse::<u64>() else { continue };
        let class = f[3].trim().to_string();
        if class.is_empty() {
            blank += 1;
            continue;
        }
        match systems.get_mut(&addr) {
            Some(e) => {
                if e.1 != class {
                    e.2 = true;
                    e.1 = class;
                }
            }
            None => {
                systems.insert(addr, (f[2].to_string(), class, false));
            }
        }
    }
    let self_disagree = systems.values().filter(|e| e.2).count();
    let (mut missing, mut unknown, mut known, mut name_clash) = (0u64, 0u64, 0u64, 0u64);
    // (route class name, index class name) -> count
    let mut confusion: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut unknown_by_route: BTreeMap<String, u64> = BTreeMap::new();
    let mut missing_by_route: BTreeMap<String, u64> = BTreeMap::new();
    let mut examples: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for (addr, (name, letter, _)) in &systems {
        let nav = StarClass::from_journal(letter);
        let Some(idx) = g.find(name) else {
            missing += 1;
            *missing_by_route.entry(nav.name().to_string()).or_default() += 1;
            continue;
        };
        if g.record(idx).id64 != *addr {
            name_clash += 1;
            continue;
        }
        let code = g.class_code(idx);
        let have = StarClass::ALL.iter().copied().find(|c| c.code() == code).unwrap_or(StarClass::Unknown);
        if have == StarClass::Unknown {
            unknown += 1;
            *unknown_by_route.entry(nav.name().to_string()).or_default() += 1;
            continue;
        }
        known += 1;
        let key = (nav.name().to_string(), have.name().to_string());
        *confusion.entry(key.clone()).or_default() += 1;
        if nav != have {
            let ex = examples.entry(key).or_default();
            if ex.len() < 3 {
                ex.push(name.clone());
            }
        }
    }
    println!("# navroute_audit {} on {}", tsv, dir.display());
    println!("# hops {lines}, blank class {blank}, unique systems {}, self-disagreeing {self_disagree}", systems.len());
    println!("# not in index (adds) {missing}, name clash {name_clash}, index Unknown (teachable) {unknown}, index known {known}");
    println!("section,route_class,count");
    for (c, n) in &missing_by_route {
        println!("adds,{c},{n}");
    }
    for (c, n) in &unknown_by_route {
        println!("teachable,{c},{n}");
    }
    let mut agree = 0u64;
    println!("route_class,index_class,count,examples");
    for ((nav, have), n) in &confusion {
        if nav == have {
            agree += n;
        }
        let ex = examples.get(&(nav.clone(), have.clone())).map(|v| v.join(" | ")).unwrap_or_default();
        println!("{nav},{have},{n},{ex}");
    }
    println!("# agreement among known: {agree} of {known} ({:.1} %)", if known > 0 { agree as f64 * 100.0 / known as f64 } else { 0.0 });
    for claim in ["Neutron", "WhiteDwarf", "BlackHole"] {
        let total: u64 = confusion.iter().filter(|((n, _), _)| n == claim).map(|(_, c)| c).sum();
        let right = confusion.get(&(claim.to_string(), claim.to_string())).copied().unwrap_or(0);
        let claimed_by_index: u64 = confusion.iter().filter(|((_, h), _)| h == claim).map(|(_, c)| c).sum();
        println!(
            "# {claim}: route claims {total}, index agrees {right} (precision {:.1} %); index has {claimed_by_index}, route says so for {right} (recall {:.1} %)",
            if total > 0 { right as f64 * 100.0 / total as f64 } else { 0.0 },
            if claimed_by_index > 0 { right as f64 * 100.0 / claimed_by_index as f64 } else { 0.0 }
        );
    }
    Ok(())
}
