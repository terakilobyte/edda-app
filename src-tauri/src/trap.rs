//! The fuel trap guard: when the commander targets a jump — on a plotted
//! route or freelancing — project the ship's state *after* that jump and
//! warn before they commit to a system they could neither refuel in nor
//! escape from to fuel. The existing low-fuel caution fires on a threshold;
//! this one reasons about the map: in-system refuel options first
//! (scoopable star, scoopable companion, a dockable station), then whether
//! the post-arrival ship could reach any scoopable star at all.
//!
//! Fleet carriers never count as refuel: they jump away, deny docking, and
//! may not sell fuel — the callout mentions one instead of trusting it.

use crate::exchange::SendApi;
use ed_galaxy::fuel::{BoostProfile, FuelModel, REACH_SLACK_LY};
use ed_galaxy::StarClass;
use ed_store::lookup::PadSize;
use serde_json::Value;
use tauri::Manager as _;

/// Resolve an `FSDTarget` event against the ship, the store, and the
/// galaxy index, and return the trap callout if one is due. Anything the
/// guard cannot resolve — no loadout, no fuel reading, a system the index
/// does not know — is silence, never a guess. `warned` keeps one warning
/// per targeted system between jumps.
pub fn on_target(
    app: &tauri::AppHandle,
    conn: &rusqlite::Connection,
    v: &Value,
    warned: &mut Option<i64>,
) -> Option<crate::callouts::Callout> {
    let name = v.get("Name")?.as_str()?;
    let ts = v.get("timestamp").and_then(Value::as_str).unwrap_or("");
    let address = v.get("SystemAddress").and_then(Value::as_i64);
    let class = StarClass::from_journal(v.get("StarClass")?.as_str()?);
    if class == StarClass::Unknown {
        return None;
    }
    if address.is_some() && *warned == address {
        return None;
    }
    let (model, profile, fuel_now, _ship) = crate::routing::ship_fuel(conn)?;
    let (ship_ident, has_scoop) = loadout_ship(conn)?;
    if class.scoopable() && has_scoop {
        return None;
    }
    let here = ed_store::query::location(conn).ok().flatten()?;
    let here_name = here.system_name?;
    if here_name.eq_ignore_ascii_case(name) {
        return None;
    }
    let state = app.state::<crate::state::AppState>();
    let g = resolve_field(&state, &here_name, name)?;
    let cur = g.find(&here_name)?;
    let target = g.find(name)?;
    let (cp, tp) = (g.pos_of(cur), g.pos_of(target));
    let distance_ly =
        ((cp[0] - tp[0]).powi(2) + (cp[1] - tp[1]).powi(2) + (cp[2] - tp[2]).powi(2)).sqrt();
    // An unknown ship needs a Large pad until proven otherwise, for the
    // same reason unknown station pads fail closed below.
    let required_pad = PadSize::for_journal_ship(&ship_ident).unwrap_or(PadSize::Large);
    // One `/v1/stations` call at target time; no answer fails closed.
    let (station_fits, carrier_present) =
        crate::remote_lookup::stations_at_blocking(&state, name, required_pad)
            .unwrap_or((false, false));
    let facts = TargetFacts {
        name: name.to_string(),
        class,
        distance_ly,
        departure_boost: profile.for_class(g.class_of(cur)),
        has_scoop,
        refuels_on_arrival: class.scoopable() || g.scoopable(target),
        station: station_fits,
        carrier: carrier_present,
        injection: crate::routing::injections_status(&state)
            .into_iter()
            .find(|g| g.can_make > 0)
            .map(|g| Injection {
                mult: g.mult,
                grade: g.grade.to_string(),
                can_make: g.can_make,
            }),
    };
    let (text, priority) = if has_scoop {
        assess(&model, &profile, fuel_now, &facts, |radius| {
            nearest_scoopable(&g, target, tp, radius).map(|(d, _)| d)
        })
    } else {
        assess(&model, &profile, fuel_now, &facts, |radius| {
            crate::remote_lookup::nearest_refuel_blocking(
                &state,
                name,
                required_pad,
                f64::from(radius),
            )
            .map(|(d, _)| d)
        })
    }?;
    *warned = address;
    Some(crate::callouts::Callout::new(
        "fuel", ts, priority, true, text,
    ))
}

use crate::status_flags::FSD_SCO_ACTIVE;

/// Live overcharge burn-rate estimate, tonnes per second, from
/// `ReservoirReplenished` bursts: during SCO the main tank drains an
/// order of magnitude faster than normal supercruise, so a drop above
/// 0.3 t inside 30 s is an overcharge signature. Rates are per-ship AND
/// throttle-dependent (measured: Mandalay 0.083 t/s, Caspian 0.555,
/// classic Python 0.83), so the estimator keeps a rolling median of
/// recent bursts and falls back to a measured per-ship prior before the
/// first burst of a session.
#[derive(Clone, Debug, Default)]
pub struct ScoBurn {
    last: Option<(f64, f64)>,
    rates: Vec<f32>,
}

impl ScoBurn {
    /// Feed one `ReservoirReplenished` reading (event time, `FuelMain`).
    pub fn observe(&mut self, epoch: f64, fuel_main: f64) {
        if let Some((t0, f0)) = self.last {
            let dt = epoch - t0;
            let drop = f0 - fuel_main;
            if dt > 0.0 && dt < 30.0 && drop > 0.3 {
                self.rates.push((drop / dt) as f32);
                if self.rates.len() > 15 {
                    self.rates.remove(0);
                }
            }
        }
        self.last = Some((epoch, fuel_main));
    }

    /// The working burn rate: median of observed bursts, or the ship's
    /// measured prior before any burst has been seen.
    pub fn rate(&self, ship: &str) -> f32 {
        if self.rates.is_empty() {
            return sco_prior(ship);
        }
        let mut sorted = self.rates.clone();
        sorted.sort_by(f32::total_cmp);
        sorted[sorted.len() / 2]
    }
}

/// Cold-start overcharge burn rates by journal ship name, t/s, measured
/// from real journals (median of `ReservoirReplenished` burst intervals).
fn sco_prior(ship: &str) -> f32 {
    match ship.trim().to_ascii_lowercase().as_str() {
        "mandalay" => 0.083,
        "panthermkii" => 0.555,
        "python" => 0.83,
        "python_nx" => 0.28,
        "krait_mkii" => 0.32,
        "anaconda" => 0.36,
        _ => 0.4,
    }
}

/// Journal ISO-8601 timestamp to epoch seconds.
pub fn epoch_secs(ts: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| t.timestamp_millis() as f64 / 1000.0)
}

/// The overcharge strand guard: while SCO burns the tank inside a system
/// the ship cannot refuel in, warn the moment the remaining fuel gets
/// close to what the escape jump itself needs — while aborting still
/// helps. Runs on every Status.json tick, but only does work while the
/// SCO flag is up.
pub fn on_status(
    app: &tauri::AppHandle,
    conn: &rusqlite::Connection,
    status: &Value,
    st: &mut crate::callouts::CalloutState,
) -> Option<crate::callouts::Callout> {
    let flags2 = status.get("Flags2").and_then(Value::as_i64).unwrap_or(0);
    if flags2 & FSD_SCO_ACTIVE == 0 {
        st.sco_strand_warned = false;
        return None;
    }
    if st.sco_strand_warned {
        return None;
    }
    let ts = status
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or("");
    let fuel_now = status.pointer("/Fuel/FuelMain").and_then(Value::as_f64)? as f32;
    let (model, profile, _stale_fuel, _ship) = crate::routing::ship_fuel(conn)?;
    // Far from any strand threshold: skip the map work entirely. The
    // warning zone is around one escape jump's burn, so twice the drive
    // cap of headroom is safely outside it.
    if fuel_now > model.max_fuel_per_jump * 2.0 {
        return None;
    }
    let (ship_ident, has_scoop) = loadout_ship(conn)?;
    let here = ed_store::query::location(conn)
        .ok()
        .flatten()?
        .system_name?;
    let state = app.state::<crate::state::AppState>();
    let g = resolve_field(&state, &here, &here)?;
    let cur = g.find(&here)?;
    // Refuel available right here: SCO cannot strand the ship.
    if has_scoop && g.scoopable(cur) {
        return None;
    }
    let required_pad = PadSize::for_journal_ship(&ship_ident).unwrap_or(PadSize::Large);
    if crate::remote_lookup::stations_at_blocking(&state, &here, required_pad)
        .is_some_and(|(fits, _)| fits)
    {
        return None;
    }
    let pos = g.pos_of(cur);
    let escape_boost = profile.for_class(g.class_of(cur)).max(1.0);
    let radius = (model.range_at(fuel_now) * escape_boost).clamp(1.0, 500.0);
    let nearest = if has_scoop {
        nearest_scoopable(&g, cur, pos, radius)
    } else {
        crate::remote_lookup::nearest_refuel_blocking(
            &state,
            &here,
            required_pad,
            f64::from(radius),
        )
    };
    let text = sco_strand(
        &model,
        fuel_now,
        escape_boost,
        st.sco_burn.rate(&ship_ident),
        &here,
        nearest.as_ref().map(|(d, n)| (*d, n.as_str())),
    )?;
    st.sco_strand_warned = true;
    Some(crate::callouts::Callout::new("fuel", ts, 3, true, text))
}

/// The pure overcharge verdict. With a reachable refuel target the
/// warning fires when the tank is within ~15 seconds of overcharge (at
/// `rate_t_per_s`) of the escape jump's own burn — never later than a
/// 25 % fuel margin — and says how long the commander has. With nothing
/// reachable it fires immediately.
pub fn sco_strand(
    model: &FuelModel,
    fuel_now: f32,
    escape_boost: f32,
    rate_t_per_s: f32,
    system: &str,
    nearest: Option<(f32, &str)>,
) -> Option<String> {
    let boost = escape_boost.max(1.0);
    let reachable = nearest.filter(|(d, _)| {
        *d <= model.range_at(fuel_now) * boost
            && model.fuel_for(*d, fuel_now, boost) <= model.max_fuel_per_jump
    });
    match reachable {
        Some((d, name)) => {
            let burn = model.fuel_for(d, fuel_now, boost);
            let margin = fuel_now - burn;
            let secs = margin / rate_t_per_s.max(0.01);
            if margin > (burn * 0.25 + 0.1).max(rate_t_per_s * 15.0) {
                return None;
            }
            Some(format!(
                "Warning: about {:.0} seconds of overcharge before you cannot leave {system}. Abort overcharge and target {name} to refuel.",
                secs.max(0.0)
            ))
        }
        None => Some(format!(
            "Warning: overcharge is burning the fuel needed to leave {system}, and nothing to refuel at is in range. Abort overcharge now."
        )),
    }
}

/// Nearest scoopable star (or scoopable-companion system) to `pos`,
/// excluding the system at `exclude`, within `radius` ly.
fn nearest_scoopable(g: &Field, exclude: u32, pos: [f32; 3], radius: f32) -> Option<(f32, String)> {
    let mut nearest: Option<(f32, u32)> = None;
    for (idx, d) in g.within(pos, radius.clamp(1.0, 500.0)) {
        if idx != exclude && g.scoopable(idx) && nearest.is_none_or(|(n, _)| d < n) {
            nearest = Some((d, idx));
        }
    }
    nearest.map(|(d, idx)| (d, g.name_of(idx)))
}

/// Where the guard reads star positions and classes from: the local
/// index when it holds both systems (the bundled bubble, or a full
/// index), else one sphere from the community API around the target
/// (API-only spec, Phase B.2 — "the server API assumes there is no
/// local data, ever"; the bundle is populated-only, and the trap
/// matters most in deep space, exactly where it has no stars).
pub(crate) enum Field {
    Index(std::sync::Arc<ed_galaxy::Galaxy>),
    Sphere(SphereField),
}

impl Field {
    fn find(&self, name: &str) -> Option<u32> {
        match self {
            Field::Index(g) => g.find(name),
            Field::Sphere(f) => f.find(name),
        }
    }
    fn pos_of(&self, idx: u32) -> [f32; 3] {
        match self {
            Field::Index(g) => g.pos_of(idx),
            Field::Sphere(f) => f.systems[idx as usize].pos,
        }
    }
    fn class_of(&self, idx: u32) -> StarClass {
        match self {
            Field::Index(g) => g.class(&g.record(idx)),
            Field::Sphere(f) => f.systems[idx as usize].class,
        }
    }
    /// Scoopable main star — or, from the index, a scoopable companion
    /// within 1,500 ls (the sphere answer carries the main star only).
    fn scoopable(&self, idx: u32) -> bool {
        match self {
            Field::Index(g) => g.scoopable(idx),
            Field::Sphere(f) => f.systems[idx as usize].class.scoopable(),
        }
    }
    fn within(&self, pos: [f32; 3], radius: f32) -> Vec<(u32, f32)> {
        match self {
            Field::Index(g) => g.within(pos, radius),
            Field::Sphere(f) => f.within(pos, radius),
        }
    }
    fn name_of(&self, idx: u32) -> String {
        match self {
            Field::Index(g) => g.name(&g.record(idx)).to_string(),
            Field::Sphere(f) => f.systems[idx as usize].name.clone(),
        }
    }
}

/// The local index if it knows both systems, else the API's sphere.
fn resolve_field(state: &crate::state::AppState, here: &str, target: &str) -> Option<Field> {
    if let Some(g) = state.routing.galaxy(&state.data_dir) {
        if g.find(here).is_some() && g.find(target).is_some() {
            return Some(Field::Index(g));
        }
    }
    SphereField::fetch(state, here, target).map(Field::Sphere)
}

pub(crate) struct SphereSystem {
    pub name: String,
    pub pos: [f32; 3],
    pub class: StarClass,
}

/// One `/v1/knowledge/sphere` answer around the target (100 ly, the
/// server's cell), plus the two anchor systems if the sphere did not
/// list them. Distances are exact within the sphere; the guard's
/// "nearest scoopable" is capped at the sphere's radius on this path.
pub(crate) struct SphereField {
    pub systems: Vec<SphereSystem>,
}

/// The server's sphere cell, and the most this path will look.
pub(crate) const SPHERE_RADIUS_LY: f32 = 100.0;
const API_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1_500);

impl SphereField {
    pub(crate) fn from_parts(anchors: Vec<SphereSystem>, sphere: &Value) -> Self {
        let mut systems: Vec<SphereSystem> = Vec::new();
        let items = sphere
            .as_array()
            .cloned()
            .or_else(|| sphere.get("systems").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        for item in &items {
            let Some(name) = item.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(pos) = coords_of(item) else { continue };
            let class = item
                .pointer("/primaryStar/type")
                .and_then(Value::as_str)
                .map(StarClass::from_subtype)
                .unwrap_or(StarClass::Unknown);
            systems.push(SphereSystem {
                name: name.to_string(),
                pos,
                class,
            });
        }
        for anchor in anchors {
            if !systems
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(&anchor.name))
            {
                systems.push(anchor);
            }
        }
        SphereField { systems }
    }

    fn find(&self, name: &str) -> Option<u32> {
        self.systems
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name.trim()))
            .map(|i| i as u32)
    }

    fn within(&self, pos: [f32; 3], radius: f32) -> Vec<(u32, f32)> {
        self.systems
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let d = ((s.pos[0] - pos[0]).powi(2)
                    + (s.pos[1] - pos[1]).powi(2)
                    + (s.pos[2] - pos[2]).powi(2))
                .sqrt();
                (d <= radius).then_some((i as u32, d))
            })
            .collect()
    }

    /// Three short calls at target time: the target's coordinates, the
    /// sphere around them, and the departure system if the sphere did
    /// not include it. Any failure is `None` — silence, never a guess.
    fn fetch(state: &crate::state::AppState, here: &str, target: &str) -> Option<SphereField> {
        let api = crate::exchange::endpoint(state)?;
        let http = state.http.clone();
        let (here, target) = (here.to_string(), target.to_string());
        tauri::async_runtime::block_on(async move {
            let started = std::time::Instant::now();
            let system = |name: String| {
                let http = http.clone();
                let api = api.clone();
                async move {
                    let value: Value = http
                        .get(format!("{api}/v1/knowledge/system"))
                        .query(&[("name", name.as_str())])
                        .timeout(API_TIMEOUT)
                        .send_api()
                        .await
                        .ok()?
                        .error_for_status()
                        .ok()?
                        .json()
                        .await
                        .ok()?;
                    let pos = coords_of(&value)?;
                    let class = value
                        .pointer("/primaryStar/type")
                        .and_then(Value::as_str)
                        .map(StarClass::from_subtype)
                        .unwrap_or(StarClass::Unknown);
                    Some(SphereSystem { name, pos, class })
                }
            };
            let target_system = system(target.clone()).await?;
            let sphere: Value = http
                .get(format!("{api}/v1/knowledge/sphere"))
                .query(&[
                    ("x", target_system.pos[0].to_string()),
                    ("y", target_system.pos[1].to_string()),
                    ("z", target_system.pos[2].to_string()),
                    ("radius", SPHERE_RADIUS_LY.to_string()),
                ])
                .timeout(API_TIMEOUT)
                .send_api()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .json()
                .await
                .ok()?;
            let mut anchors = vec![target_system];
            let listed = |name: &str| {
                sphere.as_array().into_iter().flatten().any(|i| {
                    i.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
                })
            };
            if !here.eq_ignore_ascii_case(&target) && !listed(&here) {
                anchors.push(system(here.clone()).await?);
            }
            let field = SphereField::from_parts(anchors, &sphere);
            tracing::info!(
                systems = field.systems.len(),
                ms = started.elapsed().as_millis() as u64,
                "fuel trap: star field from the community API"
            );
            Some(field)
        })
    }
}

fn coords_of(value: &Value) -> Option<[f32; 3]> {
    let c = value.get("coords")?;
    Some([
        c.get("x")?.as_f64()? as f32,
        c.get("y")?.as_f64()? as f32,
        c.get("z")?.as_f64()? as f32,
    ])
}

/// The flown ship's journal identifier and whether a fuel scoop is
/// fitted, from the latest Loadout.
pub(crate) fn loadout_ship(conn: &rusqlite::Connection) -> Option<(String, bool)> {
    let raw: String = conn
        .query_row(
            "SELECT raw FROM events WHERE event = 'Loadout' ORDER BY file DESC, offset DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok()?;
    let loadout: Value = serde_json::from_str(&raw).ok()?;
    let ship = loadout.get("Ship")?.as_str()?.to_string();
    let has_scoop = loadout
        .get("Modules")
        .and_then(Value::as_array)
        .is_some_and(|modules| {
            modules.iter().any(|module| {
                module
                    .get("Item")
                    .and_then(Value::as_str)
                    .is_some_and(|item| item.to_ascii_lowercase().contains("fuelscoop"))
            })
        });
    Some((ship, has_scoop))
}

/// What the guard resolved about the targeted system.
pub struct TargetFacts {
    pub name: String,
    /// The target's main-star class, from the FSDTarget event itself.
    pub class: StarClass,
    /// Straight-line length of the targeted jump, ly.
    pub distance_ly: f32,
    /// Boost available for THIS jump: the departure system's star class
    /// through the fitted drive's profile (1.0 when unboosted).
    pub departure_boost: f32,
    /// A fuel scoop is fitted. Without one, scoopable stars mean nothing:
    /// only a station can refuel the ship, in-system or as the escape.
    pub has_scoop: bool,
    /// The target refuels a scoop-fitted ship on arrival: scoopable main
    /// star, or a scoopable companion within 1,500 ls (index flag).
    pub refuels_on_arrival: bool,
    /// A known non-carrier station in the target system with a pad this
    /// ship fits on. An outpost a Cutter cannot dock at refuels nothing.
    pub station: bool,
    /// A known fleet carrier in the target system.
    pub carrier: bool,
    /// The best FSD injection the commander could synthesise right now.
    pub injection: Option<Injection>,
}

/// One synthesisable FSD injection: its range multiplier, its grade name,
/// and how many the materials aboard cover.
pub struct Injection {
    pub mult: f32,
    pub grade: String,
    pub can_make: u32,
}

/// Fuel left after the targeted jump, or `None` when the jump itself
/// cannot happen (out of range, over the drive's fuel cap) — that is the
/// route checker's problem, not a trap.
pub fn fuel_after_jump(
    model: &FuelModel,
    fuel_now: f32,
    distance_ly: f32,
    departure_boost: f32,
) -> Option<f32> {
    let boost = departure_boost.max(1.0);
    if distance_ly > model.range_at(fuel_now) * boost + 1e-3 {
        return None;
    }
    let burn = model.fuel_for(distance_ly, fuel_now, boost);
    if burn > model.max_fuel_per_jump + 1e-3 || burn >= fuel_now {
        return None;
    }
    Some(fuel_now - burn)
}

/// The longest jump the ship could make OUT of the target with
/// `fuel_after` tonnes aboard: mass-scaled range times the boost the
/// target's own star grants this drive (a neutron trap is not a trap if
/// you can supercharge your way out of it).
pub fn escape_range_ly(
    model: &FuelModel,
    profile: &BoostProfile,
    target_class: StarClass,
    fuel_after: f32,
) -> f32 {
    model.range_at(fuel_after) * profile.for_class(target_class).max(1.0)
}

/// The verdict. `fuel_within` answers "nearest refuel option within
/// `radius` ly of the target, excluding the target itself" — for a
/// scoop-fitted ship the caller backs it with the galaxy index's
/// scoopable stars, for a scoopless one with station-bearing systems;
/// tests back it with arithmetic.
pub fn assess(
    model: &FuelModel,
    profile: &BoostProfile,
    fuel_now: f32,
    facts: &TargetFacts,
    fuel_within: impl FnOnce(f32) -> Option<f32>,
) -> Option<(String, u8)> {
    if (facts.has_scoop && facts.refuels_on_arrival) || facts.station {
        return None;
    }
    let fuel_after = fuel_after_jump(model, fuel_now, facts.distance_ly, facts.departure_boost)?;
    let star_boost = profile.for_class(facts.class).max(1.0);
    let injection_mult = facts.injection.as_ref().map_or(1.0, |i| i.mult);
    // Synthesis does not stack with a supercharge; the escape jump takes
    // the better of the two.
    let escape_plain =
        (escape_range_ly(model, profile, facts.class, fuel_after) - REACH_SLACK_LY).max(0.0);
    let escape_best =
        (model.range_at(fuel_after) * star_boost.max(injection_mult) - REACH_SLACK_LY).max(0.0);
    let nearest = fuel_within(escape_best);
    if nearest.is_some_and(|d| d <= escape_plain) {
        return None;
    }
    // Reachable only by burning materials: a softer warning that names
    // the grade, so the commander decides with open eyes.
    if let (Some(d), Some(injection)) = (nearest, facts.injection.as_ref()) {
        if d <= escape_best && injection.mult > star_boost {
            return Some((
                format!(
                    "Caution: {} has nothing to refuel at, and the only way back to fuel from there would be a {} FSD injection. Materials aboard for {}.",
                    facts.name, injection.grade, injection.can_make
                ),
                2,
            ));
        }
    }
    let mut text = if facts.has_scoop {
        format!(
            "Warning: {} would be a fuel trap. Nothing to refuel at there, and after that jump no scoopable star would be in range. Choose a fuel stop first.",
            facts.name
        )
    } else {
        format!(
            "Warning: {} would be a fuel trap. No fuel scoop is fitted, there is no station there, and no station would be in range after that jump. Dock and refuel first.",
            facts.name
        )
    };
    if let Some(injection) = &facts.injection {
        text.push_str(&format!(
            " Not even a {} FSD injection would reach fuel from there.",
            injection.grade
        ));
    }
    if facts.carrier {
        text.push_str(
            " The only known refuel option there is a fleet carrier; verify it sells fuel before committing.",
        );
    }
    Some((text, 3))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The API path: a sphere answer (EDSM shape with coords, as the
    /// server writes it) becomes a star field the guard can search, and
    /// the two anchors are added when the sphere did not list them.
    #[test]
    fn a_sphere_answer_is_a_star_field() {
        let sphere = serde_json::json!([
            {"name": "Deep A", "id64": 1, "coords": {"x": 0.0, "y": 0.0, "z": 0.0}, "distance": 0.0, "primaryStar": {"type": "L (Brown dwarf) Star"}},
            {"name": "Deep B", "id64": 2, "coords": {"x": 30.0, "y": 0.0, "z": 0.0}, "distance": 30.0, "primaryStar": {"type": "K (Yellow-Orange) Star"}},
            {"name": "Deep C", "id64": 3, "coords": {"x": 0.0, "y": 60.0, "z": 0.0}, "distance": 60.0, "primaryStar": {"type": "Neutron Star"}}
        ]);
        let anchors = vec![
            SphereSystem {
                name: "Deep A".into(),
                pos: [0.0, 0.0, 0.0],
                class: StarClass::Unknown,
            },
            SphereSystem {
                name: "Origin".into(),
                pos: [-40.0, 0.0, 0.0],
                class: StarClass::Unknown,
            },
        ];
        let field = Field::Sphere(SphereField::from_parts(anchors, &sphere));
        let a = field
            .find("deep a")
            .expect("the sphere's own entry wins over the anchor");
        assert_eq!(
            field.class_of(a),
            StarClass::from_subtype("L (Brown dwarf) Star")
        );
        assert!(
            !field.scoopable(a),
            "brown dwarfs are not scoopable (KGBFOAM only)"
        );
        let b = field.find("Deep B").unwrap();
        assert!(field.scoopable(b));
        assert_eq!(
            field.find("Origin").map(|i| field.pos_of(i)),
            Some([-40.0, 0.0, 0.0])
        );
        assert_eq!(
            field.within([0.0, 0.0, 0.0], 35.0).len(),
            2,
            "A itself and B"
        );
        let nearest =
            nearest_scoopable(&field, a, [0.0, 0.0, 0.0], 100.0).expect("B is the nearest scoop");
        assert_eq!(nearest.1, "Deep B");
        assert!((nearest.0 - 30.0).abs() < 1e-4);
        assert!(
            nearest_scoopable(&field, a, [0.0, 0.0, 0.0], 20.0).is_none(),
            "nothing scoopable within 20 ly"
        );
    }

    /// A mid-size explorer: 200 t hull, 32 t tank, size 5 drive capped at
    /// 5 t/jump, 30 ly on the Loadout figure.
    fn model(cargo: f32) -> FuelModel {
        FuelModel::from_loadout(200.0, 32.0, 5.0, 5, false, false, 30.0, 0.0, cargo)
    }

    fn facts(class: StarClass) -> TargetFacts {
        TargetFacts {
            name: "Oevasy SG-Y d0".into(),
            class,
            distance_ly: 20.0,
            departure_boost: 1.0,
            has_scoop: true,
            refuels_on_arrival: false,
            station: false,
            carrier: false,
            injection: None,
        }
    }

    #[test]
    fn warns_when_nothing_scoopable_is_reachable() {
        let m = model(0.0);
        let (text, priority) = assess(
            &m,
            &BoostProfile::default(),
            20.0,
            &facts(StarClass::T),
            |_| None,
        )
        .expect("a dead-end system warns");
        assert_eq!(priority, 3, "a hard trap is critical");
        assert!(text.contains("Oevasy SG-Y d0"), "{text}");
        assert!(text.to_lowercase().contains("fuel"), "{text}");
    }

    #[test]
    fn silent_when_a_scoopable_star_is_within_escape_range() {
        let m = model(0.0);
        let verdict = assess(
            &m,
            &BoostProfile::default(),
            20.0,
            &facts(StarClass::T),
            |radius| Some(radius - 1.0),
        );
        assert_eq!(verdict, None);
    }

    /// The same tank and the same scoop star: an empty hold escapes, a
    /// laden one is trapped. Escape range is a function of mass.
    #[test]
    fn cargo_turns_an_escape_into_a_trap() {
        let empty = model(0.0);
        let laden = model(100.0);
        let profile = BoostProfile::default();
        let mut f = facts(StarClass::T);
        // Short enough that the laden ship can still make the jump; the
        // trap question is about what happens after arrival.
        f.distance_ly = 10.0;
        let after_empty = fuel_after_jump(&empty, 20.0, f.distance_ly, 1.0).unwrap();
        let after_laden = fuel_after_jump(&laden, 20.0, f.distance_ly, 1.0).unwrap();
        let reach_empty = escape_range_ly(&empty, &profile, f.class, after_empty);
        let reach_laden = escape_range_ly(&laden, &profile, f.class, after_laden);
        assert!(
            reach_empty > reach_laden + 1.0,
            "cargo must cost range: {reach_empty} vs {reach_laden}"
        );
        let scoop_at = (reach_empty + reach_laden) / 2.0;
        let sees = |radius: f32| (scoop_at <= radius).then_some(scoop_at);
        assert_eq!(
            assess(&empty, &profile, 20.0, &f, sees),
            None,
            "empty hold reaches the scoop"
        );
        assert!(
            assess(&laden, &profile, 20.0, &f, sees).is_some(),
            "laden hold does not"
        );
    }

    /// A neutron target credits the escape with the SHIP'S boost for that
    /// class — ×4 standard, ×6 on the size 8 SCO Mk II — never a constant.
    #[test]
    fn escape_boost_depends_on_star_class_and_fitted_drive() {
        let m = model(0.0);
        let f = facts(StarClass::Neutron);
        let after = fuel_after_jump(&m, 20.0, f.distance_ly, 1.0).unwrap();
        let standard = escape_range_ly(&m, &BoostProfile::default(), StarClass::Neutron, after);
        let mk2 = escape_range_ly(&m, &BoostProfile::MK2_SCO, StarClass::Neutron, after);
        assert!(
            (mk2 / standard - 1.5).abs() < 1e-3,
            "x6 vs x4: {mk2} vs {standard}"
        );
        let scoop_at = standard * 1.25; // between x4 and x6 reach
        let sees = |radius: f32| (scoop_at <= radius).then_some(scoop_at);
        assert!(
            assess(&m, &BoostProfile::default(), 20.0, &f, sees).is_some(),
            "x4 drive is trapped"
        );
        assert_eq!(
            assess(&m, &BoostProfile::MK2_SCO, 20.0, &f, sees),
            None,
            "x6 drive escapes"
        );
    }

    #[test]
    fn a_known_station_suppresses_the_warning() {
        let m = model(0.0);
        let mut f = facts(StarClass::T);
        f.station = true;
        assert_eq!(
            assess(&m, &BoostProfile::default(), 20.0, &f, |_| None),
            None
        );
    }

    #[test]
    fn a_carrier_never_suppresses_but_is_mentioned() {
        let m = model(0.0);
        let mut f = facts(StarClass::T);
        f.carrier = true;
        let (text, _) = assess(&m, &BoostProfile::default(), 20.0, &f, |_| None)
            .expect("a carrier is not a refuel guarantee");
        assert!(text.to_lowercase().contains("carrier"), "{text}");
    }

    #[test]
    fn a_scoopable_arrival_is_silent() {
        let m = model(0.0);
        let mut f = facts(StarClass::T);
        f.refuels_on_arrival = true;
        assert_eq!(
            assess(&m, &BoostProfile::default(), 20.0, &f, |_| None),
            None
        );
    }

    /// Without a fuel scoop, a scoopable star refuels nothing: only a
    /// station counts, in-system or within escape range.
    #[test]
    fn no_scoop_means_only_stations_count() {
        let m = model(0.0);
        let mut f = facts(StarClass::G);
        f.has_scoop = false;
        f.refuels_on_arrival = true; // a lovely G star the ship cannot drink from
        let (text, _) = assess(&m, &BoostProfile::default(), 20.0, &f, |_| None)
            .expect("scoopless ships are trapped by scoopable stars too");
        assert!(text.contains("No fuel scoop is fitted"), "{text}");
        f.station = true;
        assert_eq!(
            assess(&m, &BoostProfile::default(), 20.0, &f, |_| None),
            None
        );
    }

    /// The overcharge strand guard: silent with a healthy margin, warns
    /// by name as the tank approaches the escape burn, warns immediately
    /// when no refuel is reachable at all.
    #[test]
    fn sco_strand_warns_at_the_escape_margin() {
        let m = model(0.0);
        let nearest = Some((20.0, "Fuelum"));
        let burn = m.fuel_for(20.0, 3.0, 1.0);
        assert_eq!(
            sco_strand(&m, burn * 2.0, 1.0, 0.05, "Nowhere XY-Z c0", nearest),
            None,
            "double the escape burn is a healthy margin"
        );
        let text = sco_strand(&m, burn * 1.1, 1.0, 0.05, "Nowhere XY-Z c0", nearest)
            .expect("inside the margin warns");
        assert!(
            text.contains("Fuelum") && text.contains("Abort overcharge"),
            "{text}"
        );
        assert!(text.contains("seconds of overcharge"), "{text}");
        let text = sco_strand(&m, burn * 1.1, 1.0, 0.05, "Nowhere XY-Z c0", None)
            .expect("no reachable fuel warns immediately");
        assert!(text.contains("Abort overcharge now"), "{text}");
    }

    /// The burn-rate estimator: priors before any burst, rolling median
    /// after, and slow normal-supercruise drain never pollutes it.
    #[test]
    fn sco_burn_medians_bursts_and_falls_back_to_priors() {
        let mut b = ScoBurn::default();
        assert!(
            (b.rate("mandalay") - 0.083).abs() < 1e-4,
            "prior before any burst"
        );
        assert!(
            (b.rate("shiny_new_ship") - 0.4).abs() < 1e-4,
            "unknown ship default"
        );
        // Normal supercruise drain: 0.1 t over 60 s — ignored twice over.
        b.observe(0.0, 32.0);
        b.observe(60.0, 31.9);
        assert!(
            (b.rate("mandalay") - 0.083).abs() < 1e-4,
            "slow drain is not a burst"
        );
        // Overcharge bursts: ~1.1 t every 2 s.
        b.observe(62.0, 30.8);
        b.observe(64.0, 29.7);
        b.observe(66.0, 28.6);
        assert!(
            (b.rate("mandalay") - 0.55).abs() < 0.01,
            "median of measured bursts wins"
        );
    }

    /// A nearest option beyond the ship's present reach counts as
    /// unreachable, not as a comfort.
    #[test]
    fn sco_strand_ignores_fuel_beyond_reach() {
        let m = model(0.0);
        let far = Some((m.range_at(5.0) * 3.0, "Too Far"));
        let text = sco_strand(&m, 5.0, 1.0, 0.05, "Nowhere XY-Z c0", far).expect("warns");
        assert!(!text.contains("Too Far"), "{text}");
    }

    /// Reachable only by burning materials: a softer caution that names
    /// the grade and the count, not a hard trap.
    #[test]
    fn an_injection_escape_softens_the_warning() {
        let m = model(0.0);
        let mut f = facts(StarClass::T);
        f.injection = Some(Injection {
            mult: 2.0,
            grade: "premium".into(),
            can_make: 2,
        });
        let after = fuel_after_jump(&m, 20.0, f.distance_ly, 1.0).unwrap();
        let scoop_at = m.range_at(after) * 1.5; // beyond plain reach, inside x2
        let sees = |radius: f32| (scoop_at <= radius).then_some(scoop_at);
        let (text, priority) =
            assess(&m, &BoostProfile::default(), 20.0, &f, sees).expect("a caution is due");
        assert_eq!(priority, 2, "injection escape is a caution, not a critical");
        assert!(text.contains("premium") && text.contains("2"), "{text}");
    }

    /// When not even the best injection reaches fuel, the hard warning
    /// says so instead of leaving false hope.
    #[test]
    fn an_insufficient_injection_stays_a_hard_trap() {
        let m = model(0.0);
        let mut f = facts(StarClass::T);
        f.injection = Some(Injection {
            mult: 2.0,
            grade: "premium".into(),
            can_make: 1,
        });
        let (text, priority) =
            assess(&m, &BoostProfile::default(), 20.0, &f, |_| None).expect("still a trap");
        assert_eq!(priority, 3);
        assert!(text.contains("Not even a premium"), "{text}");
    }

    /// A jump the ship cannot make at all is the route checker's problem;
    /// the trap guard stays quiet rather than warning about a jump that
    /// will never happen.
    #[test]
    fn an_impossible_jump_is_not_a_trap() {
        let m = model(0.0);
        let mut f = facts(StarClass::T);
        f.distance_ly = 500.0;
        assert_eq!(
            assess(&m, &BoostProfile::default(), 20.0, &f, |_| None),
            None
        );
    }
}
