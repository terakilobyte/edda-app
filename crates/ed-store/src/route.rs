//! The plotted route, enriched.
//!
//! The game writes `NavRoute.json` when a route is plotted: every hop with
//! its star class and position. On its own that answers "is the next star
//! scoopable"; joined to the galaxy tables it answers the questions a pilot
//! actually has before committing: where the fuel runs out, where you can
//! dock, whose space you are crossing, and which arrivals are hazardous.
//!
//! Everything here is deterministic from the snapshot plus the database,
//! so the briefing text is built by a pure function and tested without a
//! game running.

use anyhow::Result;
use ed_domain::star::StarClass;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

/// Somewhere to dock in a hop's system, best first.
#[derive(Debug, Clone, Serialize)]
pub struct Dock {
    pub station: String,
    pub kind: Option<String>,
    pub max_pad: Option<String>,
    pub arrival_ls: Option<f64>,
    pub has_market: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hop {
    pub index: usize,
    pub system: String,
    pub id64: i64,
    pub star_class: String,
    pub scoopable: bool,
    /// Neutron star, white dwarf or black hole: jet cones at the arrival point.
    pub hazard: Option<&'static str>,
    pub distance_ly: f64,
    /// Length of the jump that leaves this system.
    pub next_leg_ly: f64,
    /// For neutron/white-dwarf hops: whether the following jump is longer
    /// than the ship's unboosted range, i.e. supercharging is required.
    /// `None` when the ship's range is unknown.
    pub boost_needed: Option<bool>,
    pub controlling_power: Option<String>,
    pub power_state: Option<String>,
    /// Controlled by a power other than the commander's pledge.
    pub opposing: bool,
    pub security: Option<String>,
    pub population: Option<i64>,
    pub dock: Option<Dock>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteBrief {
    pub plotted: Option<String>,
    pub pledged: Option<String>,
    pub hops: Vec<Hop>,
    pub total_ly: f64,
    /// Index of the first hop (after the start) you cannot scoop at, if any.
    pub first_unscoopable: Option<usize>,
    /// Longest run of consecutive unscoopable arrivals.
    pub longest_dry_run: usize,
}

/// How much the game-route layer should narrate.
///
/// `Full` is the standalone briefing: this route is all the commander
/// has, so fuel and docking are ours to say.
///
/// `Leg` is what is left when something ELSE already owns the leg — a
/// trade follow being the case that named it (maintainer, 2026-09-06). There,
/// the router (the game's plotter or EDDA's planner) owns fuel, and the
/// trade follower owns the pad: it says which station to target for
/// terminal guidance. Repeating a scoopability warning is chatter the
/// commander did not ask for, and repeating a dock is worse than
/// chatter — [`best_dock`] picks the biggest pad in the system, which on
/// a trade run is usually NOT the station being traded at. One owner per
/// fact, the item-46 rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Narration {
    Full,
    Leg,
}

fn parse_hops(raw: &str) -> Vec<(String, i64, [f64; 3], String)> {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    v.get("Route")
        .and_then(Value::as_array)
        .map(|hops| {
            hops.iter()
                .filter_map(|h| {
                    let pos = h.get("StarPos")?.as_array()?;
                    Some((
                        h.get("StarSystem")?.as_str()?.to_string(),
                        h.get("SystemAddress")?.as_i64()?,
                        [
                            pos.first()?.as_f64()?,
                            pos.get(1)?.as_f64()?,
                            pos.get(2)?.as_f64()?,
                        ],
                        h.get("StarClass")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn best_dock(conn: &Connection, id64: i64) -> Option<Dock> {
    conn.query_row(
        "SELECT name, type, pad_large, pad_medium, pad_small, distance_to_arrival, has_market
         FROM sys_stations
         WHERE system_id64 = ?1 AND is_carrier = 0 AND name IS NOT NULL
           AND (pad_large > 0 OR pad_medium > 0 OR pad_small > 0)
           AND type NOT LIKE '%Settlement%'
         ORDER BY COALESCE(pad_large,0) DESC, COALESCE(pad_medium,0) DESC,
                  has_market DESC, distance_to_arrival ASC
         LIMIT 1",
        [id64],
        |r| {
            let (l, m, s): (Option<i64>, Option<i64>, Option<i64>) =
                (r.get(2)?, r.get(3)?, r.get(4)?);
            let pad = if l.unwrap_or(0) > 0 {
                Some("large")
            } else if m.unwrap_or(0) > 0 {
                Some("medium")
            } else if s.unwrap_or(0) > 0 {
                Some("small")
            } else {
                None
            };
            Ok(Dock {
                station: r.get(0)?,
                kind: r.get(1)?,
                max_pad: pad.map(str::to_string),
                arrival_ls: r.get(5)?,
                has_market: r.get::<_, i64>(6)? != 0,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// The route in `NavRoute.json`, unless a later `NavRouteClear` cancelled
/// it. `None` when nothing is plotted.
pub fn current(conn: &Connection) -> Result<Option<RouteBrief>> {
    let jump_range: Option<f64> = conn
        .query_row("SELECT max_jump_range FROM loadout WHERE id = 1", [], |r| {
            r.get(0)
        })
        .optional()?
        .flatten();
    let Some(raw) = crate::session::snapshot_raw(conn, "NavRoute.json")? else {
        return Ok(None);
    };
    let plotted = serde_json::from_str::<Value>(&raw).ok().and_then(|v| {
        v.get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    // A clear after the plot means no route, even though the file lingers.
    let cleared: Option<String> = conn
        .query_row(
            "SELECT ts FROM events WHERE event = 'NavRouteClear' ORDER BY file DESC, offset DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let (Some(c), Some(p)) = (&cleared, &plotted) {
        if c.as_str() >= p.as_str() {
            return Ok(None);
        }
    }
    let hops = parse_hops(&raw);
    if hops.len() < 2 {
        return Ok(None);
    }

    let pledged: Option<String> = crate::session::latest_event_raw(conn, "Powerplay")?
        .and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .and_then(|v| v.get("Power").and_then(Value::as_str).map(str::to_string));

    let mut sys_stmt = conn.prepare(
        "SELECT controlling_power, power_state, security, population FROM sys_systems WHERE id64 = ?1",
    )?;

    let mut out = Vec::with_capacity(hops.len());
    let mut prev: Option<[f64; 3]> = None;
    let mut total = 0.0;
    for (i, (system, id64, pos, class)) in hops.into_iter().enumerate() {
        let distance = prev
            .map(|p| {
                ((pos[0] - p[0]).powi(2) + (pos[1] - p[1]).powi(2) + (pos[2] - p[2]).powi(2)).sqrt()
            })
            .unwrap_or(0.0);
        total += distance;
        prev = Some(pos);
        let (controlling_power, power_state, security, population): (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        ) = sys_stmt
            .query_row([id64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?
            .unwrap_or((None, None, None, None));
        let opposing = match (&controlling_power, &pledged) {
            (Some(c), Some(p)) => !c.eq_ignore_ascii_case(p),
            (Some(_), None) => false,
            _ => false,
        };
        let star = StarClass::from_journal(&class);
        out.push(Hop {
            index: i,
            scoopable: star.scoopable(),
            hazard: star.hazard_label(),
            dock: best_dock(conn, id64),
            system,
            id64,
            star_class: class,
            distance_ly: distance,
            next_leg_ly: 0.0,
            boost_needed: None,
            controlling_power,
            power_state,
            opposing,
            security,
            population,
        });
    }

    for i in 0..out.len() {
        let next_leg = out.get(i + 1).map(|h| h.distance_ly).unwrap_or(0.0);
        out[i].next_leg_ly = next_leg;
        if out[i].hazard.is_some() && next_leg > 0.0 {
            // Neutron ~x4, white dwarf ~x1.5: if the plotted jump exceeds the
            // plain range, the route only works supercharged.
            out[i].boost_needed = jump_range.map(|r| next_leg > r + 0.01);
        }
    }
    let first_unscoopable = out.iter().skip(1).find(|h| !h.scoopable).map(|h| h.index);
    let mut longest = 0;
    let mut run = 0;
    for h in out.iter().skip(1) {
        if h.scoopable {
            run = 0;
        } else {
            run += 1;
            longest = longest.max(run);
        }
    }

    Ok(Some(RouteBrief {
        plotted,
        pledged,
        hops: out,
        total_ly: total,
        first_unscoopable,
        longest_dry_run: longest,
    }))
}

/// The spoken briefing when a route is plotted. Says the things that change
/// a decision -- fuel, hazards, docking, whose space -- and nothing else.
pub fn brief_text(b: &RouteBrief, mode: Narration) -> String {
    let jumps = b.hops.len() - 1;
    let last = &b.hops[jumps];
    let mut parts = vec![format!(
        "Route plotted: {jumps} jump{} to {}, {:.0} light years.",
        if jumps == 1 { "" } else { "s" },
        last.system,
        b.total_ly
    )];

    // Fuel: only worth a sentence when a star cannot be scooped -- and
    // only when fuel is ours to speak for at all.
    if let Some(first) = b.first_unscoopable.filter(|_| mode == Narration::Full) {
        let h = &b.hops[first];
        let before = b.hops[1..first].iter().rev().find(|x| x.scoopable);
        let mut s = format!("Jump {first}, {}, is not scoopable", h.system);
        if b.longest_dry_run > 1 {
            s.push_str(&format!(" and {} in a row are dry", b.longest_dry_run));
        }
        match before {
            Some(sc) => s.push_str(&format!("; top up at {} first.", sc.system)),
            None => s.push_str("; top up before you leave."),
        }
        parts.push(s);
    }

    // Hazards and opposing powers: counts, first name only.
    let hazards: Vec<&Hop> = b
        .hops
        .iter()
        .skip(1)
        .filter(|h| h.hazard.is_some())
        .collect();
    let opposing = b.hops.iter().skip(1).filter(|h| h.opposing).count();
    let mut warn = Vec::new();
    if let Some(h) = hazards.first() {
        warn.push(if hazards.len() == 1 {
            format!("a hazardous arrival at {}", h.system)
        } else {
            format!(
                "{} hazardous arrivals, first at {}",
                hazards.len(),
                h.system
            )
        });
    }
    if opposing > 0 {
        warn.push(format!(
            "{opposing} system{} held by opposing powers",
            if opposing == 1 { "" } else { "s" }
        ));
    }
    if !warn.is_empty() {
        let mut w = warn.join(" and ");
        if let Some(c) = w.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        parts.push(format!("{w}."));
    }
    parts.join(" ")
}

/// What to say as you leave `current` for the next hop: the next star and
/// the one after it, because a scoopable star is only useful if you know
/// the following one is not.
pub fn next_hop_text(b: &RouteBrief, current: &str, mode: Narration) -> Option<String> {
    let idx = b
        .hops
        .iter()
        .position(|h| h.system.eq_ignore_ascii_case(current))?;
    let next = b.hops.get(idx + 1)?;
    let remaining = b.hops.len() - 1 - idx;
    let mut s = format!(
        "Next: {}, class {}{}",
        next.system,
        next.star_class,
        if next.scoopable {
            ", scoopable"
        } else {
            ", not scoopable"
        }
    );
    if let Some(h) = next.hazard {
        s.push_str(&format!(" -- {h}, throttle down"));
        match next.boost_needed {
            Some(true) => s.push_str(&format!(
                "; supercharge required, the jump after is {:.0} light years",
                next.next_leg_ly
            )),
            Some(false) => s.push_str("; no supercharge needed"),
            None => {}
        }
    }
    if let Some(after) = b.hops.get(idx + 2) {
        s.push_str(&format!(
            ". Then {}, class {}{}",
            after.system,
            after.star_class,
            if after.scoopable {
                ""
            } else {
                ", not scoopable"
            }
        ));
    }
    if next.opposing {
        s.push_str(&format!(
            ". {} space{}",
            next.controlling_power.as_deref().unwrap_or("Opposing"),
            next.power_state
                .as_deref()
                .map(|st| format!(", {}", st.to_lowercase()))
                .unwrap_or_default()
        ));
    }
    if let Some(d) = next.dock.as_ref().filter(|_| mode == Narration::Full) {
        s.push_str(&format!(". Docking at {}", d.station));
    }
    s.push_str(&format!(
        ". {remaining} jump{} remaining.",
        if remaining == 1 { "" } else { "s" }
    ));
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop(i: usize, system: &str, class: &str, power: Option<&str>, dock: bool) -> Hop {
        Hop {
            index: i,
            system: system.into(),
            id64: i as i64,
            star_class: class.into(),
            scoopable: StarClass::from_journal(class).scoopable(),
            hazard: StarClass::from_journal(class).hazard_label(),
            distance_ly: if i == 0 { 0.0 } else { 20.0 },
            next_leg_ly: 0.0,
            boost_needed: None,
            controlling_power: power.map(str::to_string),
            power_state: power.map(|_| "Stronghold".to_string()),
            opposing: power.is_some_and(|p| p != "Aisling Duval"),
            security: None,
            population: None,
            dock: dock.then(|| Dock {
                station: format!("{system} Port"),
                kind: Some("Coriolis Starport".into()),
                max_pad: Some("large".into()),
                arrival_ls: Some(100.0),
                has_market: true,
            }),
        }
    }

    fn brief(hops: Vec<Hop>) -> RouteBrief {
        let first_unscoopable = hops.iter().skip(1).find(|h| !h.scoopable).map(|h| h.index);
        let mut longest = 0;
        let mut run = 0;
        for h in hops.iter().skip(1) {
            if h.scoopable {
                run = 0
            } else {
                run += 1;
                longest = longest.max(run)
            }
        }
        RouteBrief {
            plotted: None,
            pledged: Some("Aisling Duval".into()),
            total_ly: 20.0 * (hops.len() - 1) as f64,
            hops,
            first_unscoopable,
            longest_dry_run: longest,
        }
    }

    /// Journal `StarClass` values that start with a scoopable letter but
    /// are not scoopable stars: "AeBe" (Herbig protostar) and "MS" (an
    /// S-type exotic). The router already knows this; the brief must agree.
    #[test]
    fn aebe_and_ms_are_not_scoopable_in_route_briefs() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        let raw = r#"{"timestamp":"2026-08-01T10:00:00Z","event":"NavRoute","Route":[
            {"StarSystem":"Start","SystemAddress":1,"StarPos":[0,0,0],"StarClass":"K"},
            {"StarSystem":"Herbig","SystemAddress":2,"StarPos":[20,0,0],"StarClass":"AeBe"},
            {"StarSystem":"Stype","SystemAddress":3,"StarPos":[40,0,0],"StarClass":"MS"}]}"#;
        conn.execute(
            "INSERT INTO snapshots (name, ts, mtime, raw) VALUES ('NavRoute.json', '2026-08-01T10:00:00Z', 0, ?1)",
            [raw],
        )
        .unwrap();
        let b = current(&conn).unwrap().expect("a plotted route");
        assert!(
            !b.hops[1].scoopable,
            "AeBe is a protostar, not an A-class star"
        );
        assert!(
            !b.hops[2].scoopable,
            "MS is an S-type star, not an M-class star"
        );
        assert_eq!(b.first_unscoopable, Some(1));
        assert_eq!(b.longest_dry_run, 2);
    }

    #[test]
    fn supermassive_black_hole_is_a_hazard_in_route_briefs() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        let raw = r#"{"timestamp":"2026-08-01T10:00:00Z","event":"NavRoute","Route":[
            {"StarSystem":"Start","SystemAddress":1,"StarPos":[0,0,0],"StarClass":"K"},
            {"StarSystem":"Sagittarius A*","SystemAddress":2,"StarPos":[20,0,0],"StarClass":"SupermassiveBlackHole"}]}"#;
        conn.execute(
            "INSERT INTO snapshots (name, ts, mtime, raw) VALUES ('NavRoute.json', '2026-08-01T10:00:00Z', 0, ?1)",
            [raw],
        )
        .unwrap();
        let b = current(&conn).unwrap().expect("a plotted route");
        assert_eq!(b.hops[1].hazard, Some("black hole"));
        let t = brief_text(&b, Narration::Full);
        assert!(t.contains("hazardous arrival at Sagittarius A*"), "{t}");
    }

    #[test]
    fn the_wongi_route_briefs_fuel_hazards_and_docking() {
        let b = brief(vec![
            hop(0, "Wongi", "K", Some("Aisling Duval"), true),
            hop(1, "Crucis Sector PT-R b4-3", "M", None, false),
            hop(2, "LAWD 68", "DA", None, false),
            hop(3, "LAWD 80", "DA", Some("Zemina Torval"), true),
            hop(4, "LHS 3589", "DA", None, false),
        ]);
        let t = brief_text(&b, Narration::Full);
        assert!(
            t.starts_with("Route plotted: 4 jumps to LHS 3589, 80 light years."),
            "{t}"
        );
        assert!(t.contains("Jump 2, LAWD 68, is not scoopable and 3 in a row are dry; top up at Crucis Sector PT-R b4-3 first."), "{t}");
        assert!(
            t.contains(
                "3 hazardous arrivals, first at LAWD 68 and 1 system held by opposing powers."
            ),
            "{t}"
        );
        assert!(t.split(". ").count() <= 4, "briefing stays short: {t}");
    }

    #[test]
    fn next_hop_names_the_one_after_and_the_opposing_power() {
        let b = brief(vec![
            hop(0, "Wongi", "K", Some("Aisling Duval"), true),
            hop(1, "Deciat", "K", Some("Zemina Torval"), true),
            hop(2, "LAWD 68", "DA", None, false),
        ]);
        let t = next_hop_text(&b, "Wongi", Narration::Full).unwrap();
        assert_eq!(t, "Next: Deciat, class K, scoopable. Then LAWD 68, class DA, not scoopable. Zemina Torval space, stronghold. Docking at Deciat Port. 2 jumps remaining.");
        assert!(
            next_hop_text(&b, "LAWD 68", Narration::Full).is_none(),
            "no next hop at the destination"
        );
    }

    /// Maintainer, 2026-09-06, mid trade run: "'One jump is not scoopable' and
    /// 'docking at' is confusing/needless in a trade route follow — the
    /// game or we will calculate for fuel." In `Leg` mode the game-route
    /// layer keeps the skeleton (jumps, stars, hazards, whose space, the
    /// countdown) and drops the two facts another owner already holds.
    /// The dock clause is the sharper one: `best_dock` names the system's
    /// biggest pad, which on a trade run is usually the WRONG station.
    #[test]
    fn a_trade_leg_drops_the_fuel_sentence_and_the_guessed_dock() {
        let b = brief(vec![
            hop(0, "Wongi", "K", Some("Aisling Duval"), true),
            hop(1, "Crucis Sector PT-R b4-3", "M", None, false),
            hop(2, "LAWD 68", "DA", None, false),
            hop(3, "LHS 3589", "DA", Some("Zemina Torval"), true),
        ]);

        let full = brief_text(&b, Narration::Full);
        let leg = brief_text(&b, Narration::Leg);
        assert!(full.contains("is not scoopable"), "{full}");
        assert!(
            !leg.contains("is not scoopable"),
            "fuel belongs to the router: {leg}"
        );
        assert!(
            leg.starts_with("Route plotted: 3 jumps to LHS 3589,"),
            "the skeleton stays: {leg}"
        );
        assert!(
            leg.contains("hazardous arrivals"),
            "hazards are nobody else's job: {leg}"
        );

        let full = next_hop_text(&b, "LAWD 68", Narration::Full).unwrap();
        let leg = next_hop_text(&b, "LAWD 68", Narration::Leg).unwrap();
        assert!(full.contains("Docking at LHS 3589 Port"), "{full}");
        assert!(
            !leg.contains("Docking at"),
            "the trade follower names the pad: {leg}"
        );
        assert_eq!(
            leg,
            "Next: LHS 3589, class DA, not scoopable -- white dwarf, throttle down. Zemina Torval space, stronghold. 1 jump remaining."
        );
    }

    #[test]
    fn supercharge_is_called_when_the_following_jump_exceeds_range() {
        let mut hops = vec![
            hop(0, "A", "K", None, false),
            hop(1, "N1", "N", None, false),
            hop(2, "B", "K", None, false),
        ];
        hops[1].next_leg_ly = 120.0;
        hops[1].boost_needed = Some(true);
        let b = brief(hops);
        let n = next_hop_text(&b, "A", Narration::Full).unwrap();
        assert!(n.contains("neutron star, throttle down; supercharge required, the jump after is 120 light years"), "{n}");
    }

    #[test]
    fn an_all_scoopable_route_says_so() {
        let b = brief(vec![
            hop(0, "A", "K", None, false),
            hop(1, "B", "G", None, false),
        ]);
        let t = brief_text(&b, Narration::Full);
        assert!(t.starts_with("Route plotted: 1 jump to B"), "{t}");
        assert!(!t.contains("scoopable"), "nothing to warn about: {t}");
    }
}
