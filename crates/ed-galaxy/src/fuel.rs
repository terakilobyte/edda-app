//! Fuel and range: the physics the plotter was missing.
//!
//! Jump range depends on mass, and mass includes the fuel aboard, so a full
//! tank jumps shorter than the Loadout's `MaxJumpRange` (measured with just
//! one jump's fuel). Each jump burns fuel by the game's curve, and only
//! scoopable stars (K G B F O A M) refill it. Neutron stars -- the highway --
//! do not, which is why a plan of 70 consecutive supercharged hops is
//! 20,000 ly of wishful thinking.
//!
//! Formula (validated against 393 of the commander's own Mandalay jumps,
//! residual spread 1.070-1.071 before the multiplier is fitted):
//!
//!   r0(mass)  = optimal_mass / mass * (max_fuel / multiplier)^(1/power)
//!   range     = r0 + booster                      (Guardian booster is additive)
//!   fuel(d)   = multiplier * (d_eff * mass / optimal_mass)^power
//!   d_eff     = (d / supercharge) * r0 / (r0 + booster)
//!
//! `multiplier` is 0.012 for standard drives and 0.013 for SCO drives; the
//! exponent depends on drive size. Optimal mass is derived from the ship's
//! own `MaxJumpRange`, so engineering is accounted for exactly.

use crate::StarClass;
use serde::{Deserialize, Serialize};

/// Supercharge multipliers. Standard drives: neutron x4, white dwarf x1.5.
/// The Caspian Explorer's Class 8 Mk II SCO: x6 and x3.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoostProfile {
    pub neutron: f32,
    pub white_dwarf: f32,
}

impl Default for BoostProfile {
    /// A standard drive: the multipliers [`StarClass::boost`] gives.
    fn default() -> Self {
        BoostProfile { neutron: StarClass::Neutron.boost(), white_dwarf: StarClass::WhiteDwarf.boost() }
    }
}

impl BoostProfile {
    pub const MK2_SCO: BoostProfile = BoostProfile { neutron: 6.0, white_dwarf: 3.0 };

    /// FSD integrity lost per supercharge, as a fraction of full health:
    /// one constant for every drive and both star classes.
    ///
    /// Fitted 2026-08-30 from the commander's journal (`tools/fsd_integrity.py`
    /// over `.data/edda.sqlite3`): health is only observed at `Loadout`
    /// (docking, outfitting) and after `RepairAll` / `AfmuRepairs`, so the
    /// fit is loss between consecutive observations divided by the
    /// supercharged `FSDJump`s (`BoostUsed`) between them, with no repair in
    /// the window. 11 usable windows, 14 boosts:
    ///
    /// | drive  | source      | boosts | per boost (median, range)  |
    /// |--------|-------------|--------|----------------------------|
    /// | 5A SCO | neutron     | 5      | 0.69 %  (0.35..1.01)       |
    /// | 5A SCO | white dwarf | 6      | 0.90 %  (0.69..1.98)       |
    /// | 5A     | white dwarf | 2      | 1.04 %  (0.84..1.23)       |
    /// | 6A     | white dwarf | 1      | 0.78 %                     |
    ///
    /// All sources and drives: median 0.84 %, mean 0.92 %. Jump distance
    /// makes no visible difference (288 ly neutron hops and 20 ly white-dwarf
    /// hops cost the same within the spread), and neutron vs white dwarf
    /// cannot be separated at this sample size; both agree with the
    /// community figure of about 1 % per boost, which is what this models.
    /// Boost-free windows lose 0.03 % per plain jump on average, with single
    /// windows losing up to 3 % (heat, SCO overcharge): that noise is the
    /// floor of what any per-boost fit from Loadouts can see. The size 8
    /// SCO Mk II's reputed zero loss is unverified: no boosted jump on that
    /// drive is in the journal yet. A finer fit needs a health reading per
    /// jump, which no journal event provides.
    ///
    /// The size 8 SCO Mk II (Caspian Explorer only) is the documented
    /// exception: per the wiki it does not take the flat 1 % per supercharge.
    /// The journal agrees as far as it can: 111 boosted Mk II jumps on the
    /// 2026-08-28/29 Wongi -> Colonia -> Wongi run, and the RepairAll bills
    /// at each end (58,116 Cr after 67 boosts, 22,552 Cr after 44 -- about
    /// 870 Cr per boost, consistent with heat wear on cheap modules, not a
    /// 67 % dent in a size 8 drive). The direct reading, a Loadout after the
    /// return leg, is still to come; when it does, `tools/fsd_integrity.py`
    /// picks it up.
    pub const INTEGRITY_LOSS_PER_BOOST: f32 = 0.01;
    /// See above: the Mk II SCO's exemption, as documented.
    pub const MK2_SCO_INTEGRITY_LOSS_PER_BOOST: f32 = 0.0;
    /// Plan a repair before the drive's integrity falls below this: the FSD
    /// starts malfunctioning below 80 % (emergency drops, more damage), so
    /// the stop is planned at 81 % -- about 19 boosts from full on a
    /// standard drive.
    pub const FSD_REPAIR_AT_INTEGRITY: f32 = 0.81;

    /// Integrity lost per supercharge by this profile's drive.
    pub fn integrity_loss_per_boost(&self) -> f32 {
        if self.neutron >= Self::MK2_SCO.neutron {
            Self::MK2_SCO_INTEGRITY_LOSS_PER_BOOST
        } else {
            Self::INTEGRITY_LOSS_PER_BOOST
        }
    }

    /// This ship's multiplier at `class`; the ship-independent default is
    /// [`StarClass::boost`].
    pub fn for_class(&self, class: StarClass) -> f32 {
        match class {
            StarClass::Neutron => self.neutron,
            StarClass::WhiteDwarf => self.white_dwarf,
            _ => 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FuelModel {
    /// Hull + modules, no fuel, no cargo (Loadout `UnladenMass`).
    pub unladen_mass: f32,
    /// Main tank (Loadout `FuelCapacity.Main`).
    pub capacity: f32,
    /// Most fuel one jump can burn (drive cap).
    pub max_fuel_per_jump: f32,
    /// Fuel-curve exponent by drive size: 2.00 (size 2) .. 2.90 (size 8).
    pub power: f32,
    /// 0.012 standard, 0.013 SCO.
    pub multiplier: f32,
    /// Effective optimal mass, engineering included (derived from MaxJumpRange).
    pub optimal_mass: f32,
    /// Fuel scoop rate, t/s (Loadout scoop module; 0 = unknown, time
    /// estimates then fall back to a flat per-stop cost). Item 22.
    pub scoop_rate: f32,
    /// Phantom tonnes added when pricing reach: the plan-time safety
    /// margin. Item 31 shipped the h0 era -- default 0.0: the ladder,
    /// repeats gate, and two independent Spansh replays measured the
    /// old 10 t clamp costing 3,000-5,400 fitted seconds across the
    /// matrix to guard against a fuel model that is gram-exact
    /// (+0.0 t over 513 t vs Spansh, within 1 t over a real 62-jump
    /// flight). The app's safe-margin toggle sets 2.0 (2x measured
    /// model error, the repeats gate's alternate winner).
    /// ED_FUEL_HEADROOM still overrides everything for the harness.
    pub headroom_t: f32,
    /// Guardian FSD booster, ly (0 if none).
    pub booster: f32,
    /// Tonnes to keep in hand; jumps that would go below it are refused.
    pub reserve: f32,
    /// Cargo aboard, tonnes.
    pub cargo: f32,
}

/// Planning headroom, in tonnes. Fuel is mass: a tank a few tonnes above
/// the schedule (a skipped scoop, a burn in supercruise) shortens the real
/// reach by a light year or two, and a hop planned at the very edge then
/// fails in the map. So every hop is planned as if the tank were this much
/// heavier than scheduled -- capped at a full tank, which cannot get any
/// heavier, so the natural 430-440 ly highway hops from a full tank are
/// not thrown away (a percentage margin cost 16 jumps on Colonia -> Wongi).
/// Scaled to the tank: 8 % of capacity, never less than 2 t or more than
/// 10 t. A 128 t Explorer gets 10 t (about one skipped scoop); a 32 t ship
/// 2.6 t, so a light hull is not planned at full-tank mass on every hop.
pub const FUEL_HEADROOM_FRACTION: f32 = 0.08;
pub const FUEL_HEADROOM_MIN_T: f32 = 2.0;
pub const FUEL_HEADROOM_MAX_T: f32 = 10.0;

pub fn fuel_headroom_override() -> Option<f32> {
    static OVERRIDE: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *OVERRIDE.get_or_init(|| std::env::var("ED_FUEL_HEADROOM").ok().and_then(|v| v.parse::<f32>().ok()))
}

/// The PRE-item-31 default margin (8% of tank, clamped 2-10 t), kept
/// for reference and the harness; no production path calls it since
/// the h0 era shipped.
pub fn fuel_headroom_for(capacity: f32) -> f32 {
    // The override is read once: this sits under `reach`, on every
    // expansion of every search, and `std::env::var` takes a process-wide
    // lock -- parallel searches (the long-range variants, the mid-range
    // exact + neutron-first pair) serialised on it, an exact plot going
    // from 44 ms alone to 124 ms beside one other search.
    static OVERRIDE: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    if let Some(v) = *OVERRIDE.get_or_init(|| std::env::var("ED_FUEL_HEADROOM").ok().and_then(|v| v.parse::<f32>().ok())) {
        return v;
    }
    (capacity * FUEL_HEADROOM_FRACTION).clamp(FUEL_HEADROOM_MIN_T, FUEL_HEADROOM_MAX_T)
}

/// Light-years kept in hand on every hop for rounding between the model and the game.
pub const REACH_SLACK_LY: f32 = 0.3;

/// Kept for callers that still think in fractions: no percentage margin.
pub fn range_margin() -> f32 {
    0.0
}

impl FuelModel {
    /// The distance the planner allows for a jump with `fuel` aboard and
    /// `boost` from the departure star: the physical range less the margin.
    pub fn reach(&self, fuel: f32, boost: f32) -> f32 {
        let heavy = (fuel + fuel_headroom_override().unwrap_or(self.headroom_t)).min(self.capacity.max(fuel));
        (self.range_at(heavy) * boost.max(1.0) - REACH_SLACK_LY).max(0.0)
    }

    /// Build from the Loadout. `max_jump_range` is the Loadout figure
    /// (unladen mass plus one jump's fuel, booster included).
    #[allow(clippy::too_many_arguments)]
    pub fn from_loadout(
        unladen_mass: f32,
        capacity: f32,
        max_fuel_per_jump: f32,
        drive_size: u8,
        sco: bool,
        mk2: bool,
        max_jump_range: f32,
        booster: f32,
        cargo: f32,
    ) -> Self {
        let (power, multiplier) = drive_curve(drive_size, sco, mk2);
        let k = (max_fuel_per_jump / multiplier).powf(1.0 / power);
        let optimal_mass = (max_jump_range - booster).max(1.0) * (unladen_mass + max_fuel_per_jump) / k;
        FuelModel { unladen_mass, capacity, max_fuel_per_jump, power, multiplier, optimal_mass, headroom_t: 0.0,
            scoop_rate: 0.0, booster, reserve: 0.0, cargo }
    }

    pub fn mass(&self, fuel: f32) -> f32 {
        self.unladen_mass + self.cargo + fuel.max(0.0)
    }

    fn r0(&self, fuel: f32) -> f32 {
        self.optimal_mass / self.mass(fuel) * (self.max_fuel_per_jump / self.multiplier).powf(1.0 / self.power)
    }

    /// Unboosted range with `fuel` tonnes aboard.
    pub fn range_at(&self, fuel: f32) -> f32 {
        self.r0(fuel) + self.booster
    }

    /// Fuel burned by a jump of `dist` ly with `fuel` aboard, supercharged
    /// by `boost` (1.0 = none).
    pub fn fuel_for(&self, dist: f32, fuel: f32, boost: f32) -> f32 {
        let r0 = self.r0(fuel);
        let eff = (dist / boost.max(1.0)) * r0 / (r0 + self.booster);
        self.multiplier * (eff * self.mass(fuel) / self.optimal_mass).powf(self.power)
    }

    /// Can the ship jump `dist` (supercharged by `boost`) with `fuel`
    /// aboard, keeping the reserve? Returns fuel left on arrival.
    pub fn jump(&self, dist: f32, fuel: f32, boost: f32) -> Option<f32> {
        if dist > self.reach(fuel, boost) + 1e-3 {
            return None;
        }
        let burn = self.fuel_for(dist, fuel, boost);
        if burn > self.max_fuel_per_jump + 1e-3 {
            return None;
        }
        let left = fuel - burn;
        (left >= self.reserve - 1e-3).then_some(left.max(0.0))
    }
}

/// (exponent, multiplier) for a drive. Standard: 0.012 and the size table;
/// SCO: 0.013; the Class 8 Mk II SCO is its own curve (2.5025, 0.011) with
/// a 6.8 t cap -- the "special fuel modifier" on that drive.
pub fn drive_curve(size: u8, sco: bool, mk2: bool) -> (f32, f32) {
    if mk2 {
        return (2.5025, 0.011);
    }
    (fuel_power(size), if sco { 0.013 } else { 0.012 })
}

/// Max fuel per jump for the Class 8 Mk II SCO (Coriolis/Spansh data).
pub const MK2_SCO_MAX_FUEL: f32 = 6.8;

/// Fuel-curve exponent by drive size (public formula).
pub fn fuel_power(size: u8) -> f32 {
    match size {
        2 => 2.00,
        3 => 2.15,
        4 => 2.30,
        5 => 2.45,
        6 => 2.60,
        7 => 2.75,
        8 => 2.90,
        _ => 2.45,
    }
}

/// Base max fuel per jump by drive size and rating (A..E), standard drives.
/// SCO drives burn ~4 % more (measured: 5.20 / 8.27 / 13.10 t on 5A / 6A /
/// 7A SCO). Size 8 is extrapolated; prefer the ship's observed maximum.
pub fn base_max_fuel(size: u8, rating: char) -> Option<f32> {
    let row: [f32; 5] = match size {
        2 => [0.9, 0.8, 0.6, 0.6, 0.6],
        3 => [1.8, 1.5, 1.2, 1.2, 1.2],
        4 => [3.0, 2.5, 2.0, 2.0, 2.0],
        5 => [5.0, 4.1, 3.3, 3.3, 3.3],
        6 => [8.0, 6.6, 5.3, 5.3, 5.3],
        7 => [12.8, 10.6, 8.5, 8.5, 8.5],
        8 => [20.0, 16.5, 13.3, 13.3, 13.3],
        _ => return None,
    };
    let i = match rating.to_ascii_uppercase() {
        'A' => 0,
        'B' => 1,
        'C' => 2,
        'D' => 3,
        'E' => 4,
        _ => return None,
    };
    Some(row[i])
}

/// Guardian FSD booster range bonus by module size.
pub fn guardian_booster_ly(size: u8) -> f32 {
    match size {
        1 => 4.0,
        2 => 6.0,
        3 => 7.75,
        4 => 9.25,
        5 => 10.5,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    /// Every drive loses the community 1 % per supercharge except the size 8
    /// SCO Mk II, which is documented not to; a repair is due at 81 %, above
    /// the 80 % where the drive starts malfunctioning.
    #[test]
    fn integrity_loss_is_one_percent_except_on_the_mk2_sco() {
        assert_eq!(BoostProfile::default().integrity_loss_per_boost(), 0.01);
        assert_eq!(BoostProfile::MK2_SCO.integrity_loss_per_boost(), 0.0);
        assert!(BoostProfile::FSD_REPAIR_AT_INTEGRITY > 0.80);
        // 19 boosts from full stay above the repair line; the 20th crosses it.
        let after = |n: u32| 1.0 - n as f32 * BoostProfile::default().integrity_loss_per_boost();
        assert!(after(19) >= BoostProfile::FSD_REPAIR_AT_INTEGRITY);
        assert!(after(20) < BoostProfile::FSD_REPAIR_AT_INTEGRITY);
    }

    use super::*;

    /// The commander's Mandalay as fitted: 5A SCO, G5 range + Mass Manager,
    /// size-5 Guardian booster. Loadout: MaxJumpRange 77.9, unladen 319 t,
    /// 32 t tank. Spansh's model for it: optimal mass 1894, 5.2 t cap.
    fn mandalay() -> FuelModel {
        FuelModel::from_loadout(319.0, 32.0, 5.2, 5, true, false, 77.9, 10.5, 0.0)
    }

    #[test]
    fn optimal_mass_is_recovered_from_the_loadout_range() {
        let m = mandalay();
        assert!((m.optimal_mass - 1894.0).abs() < 40.0, "optimal mass {}", m.optimal_mass);
        assert!((m.range_at(5.2) - 77.9).abs() < 0.05);
        // Full tank: ~72 ly, not 78.
        let full = m.range_at(32.0);
        assert!(full > 70.0 && full < 74.0, "full-tank range {full}");
    }

    #[test]
    fn fuel_matches_a_real_jump() {
        // Journal (FSDJump, 2026-08-26): 70.112 ly on a full 32 t tank
        // burned 4.898 t. Model lands within a few percent.
        let m = mandalay();
        let burn = m.fuel_for(70.112, 32.0, 1.0);
        assert!((burn - 4.90).abs() < 0.3, "burn {burn}");
        // A neutron hop of 4x burns like a max-range jump, never more than the cap.
        let r = m.range_at(32.0);
        assert!((m.fuel_for(r * 4.0, 32.0, 4.0) - m.max_fuel_per_jump).abs() < 0.05);
    }

    #[test]
    fn jumps_are_refused_beyond_range_or_fuel() {
        let mut m = mandalay();
        m.reserve = 1.0;
        assert!(m.jump(200.0, 32.0, 1.0).is_none(), "beyond range");
        assert!(m.jump(70.0, 32.0, 4.0).is_some());
        assert!(m.jump(m.range_at(3.0), 3.0, 1.0).is_none(), "would dip under the reserve");
    }
}
