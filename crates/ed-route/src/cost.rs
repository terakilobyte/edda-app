//! What a route actually costs in wall-clock time.
//!
//! Route planners conventionally minimise *jumps*, which is the wrong
//! objective: a run that earns 30M credits in 20 minutes beats one that
//! earns 40M in 45. Everything here exists so routes can be scored in
//! credits (or merits) **per hour** rather than per loop.
//!
//! # Where these numbers come from
//!
//! `JUMP_SECONDS` is measured from the commander's own journal: 1,222
//! `StartJump` → `FSDJump` pairs, median 18.0s, p90 19.0s. That is a real,
//! tight constant.
//!
//! Docking and undocking are also measured from this journal: 293
//! `DockingRequested` → `Docked` pairs have a 61s median (p90 97s), and 280
//! `Undocked` → `StartJump` pairs have a 55s median (p90 106s). Their rounded
//! 60s constants below are therefore grounded in the commander's play.
//!
//! Supercruise is the fitted shape (2026-09-09): sampled only inside
//! recorded trade-follow windows, where the journal interval is travel and
//! not the commander's other business, it is ≈150 s flat plus ≈0.5 s per
//! 1,000 ls. Raw `SupercruiseEntry` → `Docked` intervals outside those
//! windows correlate with distance at R² ≈ 0.001, which is why the earlier
//! logarithmic guess could not be checked against them.
//!
//! The practical consequence: use these to *compare* routes, and present
//! a duration as measured only when the commander's own sampled constants
//! priced it (`Timing::measured`).

use serde::{Deserialize, Serialize};

/// Charge, witchspace and arrival for one hyperspace jump.
/// Measured: n=1222, median 18.0s, p90 19.0s.
pub const JUMP_SECONDS: f64 = 18.0;

/// Supercruise, arrival to docking request, as the fitted shape (maintainer,
/// 2026-09-09: "take the fitted shape … and assume I'm an average
/// pilot"): the assistant's two-month journal fit inside trade-follow windows
/// is ≈ 148 s flat plus ≈ 0.5 s per 1,000 ls — the cruise is dominated
/// by alignment, spool-up, the approach and drop-out, and distance
/// barely moves it inside the bubble's arrival distances. Replaces the
/// documented `45 + 22·ln(1+ls)` guess, which priced the first measured
/// loop 174 s / 234 s against 98 s / 139 s flown.
pub const SUPERCRUISE_BASE_SECONDS: f64 = 150.0;
pub const SUPERCRUISE_PER_KLS: f64 = 0.5;

/// Measured: n=293, median 61s, p90 97s; rounded to 60s.
pub const DOCKING_SECONDS: f64 = 60.0;

/// Estimated: the journal records transactions but not market-screen close.
/// 30 s until the first measured loop (maintainer, 2026-09-09: "market: add
/// 5s"; the Panther Mk II's stop was 41 s).
pub const MARKET_SECONDS: f64 = 35.0;

/// Multiplier on the whole supercruise estimate (base and per-ls term).
/// A knob for a measured pilot, 1.0 by default; the "double it" ruling
/// of 2026-09-09 was withdrawn once the comparison behind it was found
/// to be against the base alone (ledger).
pub const SUPERCRUISE_SCALE: f64 = 1.0;

/// Measured: n=280, median 55s, p90 106s; rounded to 60s. The medium
/// pad's figure; see [`Timing::undock_for_pad`].
pub const UNDOCK_SECONDS: f64 = 60.0;

/// Undock to hyperspace by pad size (maintainer, 2026-09-09, from the first
/// measured loop, verbatim: "undock and hyperspace - large: 70s,
/// medium: 60s, small: 50s"; the Panther Mk II flew 69 s and 81 s).
pub const UNDOCK_SECONDS_LARGE: f64 = 70.0;
pub const UNDOCK_SECONDS_MEDIUM: f64 = 60.0;
pub const UNDOCK_SECONDS_SMALL: f64 = 50.0;

/// Supercruise travel time to a body `distance_ls` from the arrival
/// point, at the default timing: the fitted flat-plus-slope shape.
pub fn supercruise_seconds(distance_ls: f64) -> f64 {
    Timing::default().supercruise_seconds(distance_ls)
}

/// The time constants a leg estimate is built from — the commander's
/// own when the client has measured them from the journal, the
/// documented defaults otherwise (maintainer, 2026-09-09: "time args should
/// be optional to the api with sensible defaults if omitted"). Every
/// field is optional on the wire; an omitted one is its default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timing {
    pub jump_seconds: f64,
    pub undock_seconds: f64,
    pub docking_seconds: f64,
    pub market_seconds: f64,
    pub supercruise_base_seconds: f64,
    /// Seconds per 1,000 ls of arrival distance on top of the base.
    pub supercruise_per_kls: f64,
    /// Multiplier on the whole supercruise estimate; see
    /// [`SUPERCRUISE_SCALE`].
    pub supercruise_scale: f64,
    /// True when the numbers came from the commander's journal, so a
    /// leg's duration may say `measured` instead of `estimated`.
    pub measured: bool,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            jump_seconds: JUMP_SECONDS,
            undock_seconds: UNDOCK_SECONDS,
            docking_seconds: DOCKING_SECONDS,
            market_seconds: MARKET_SECONDS,
            supercruise_base_seconds: SUPERCRUISE_BASE_SECONDS,
            supercruise_per_kls: SUPERCRUISE_PER_KLS,
            supercruise_scale: SUPERCRUISE_SCALE,
            measured: false,
        }
    }
}

impl Timing {
    /// The undock default for a ship's pad; no pad known is the medium
    /// figure, the old single constant.
    pub fn undock_for_pad(pad: Option<ed_domain::station::PadSize>) -> f64 {
        use ed_domain::station::PadSize;
        match pad {
            Some(PadSize::Large) => UNDOCK_SECONDS_LARGE,
            Some(PadSize::Small) => UNDOCK_SECONDS_SMALL,
            Some(PadSize::Medium) | None => UNDOCK_SECONDS_MEDIUM,
        }
    }

    /// The defaults for a ship of this pad.
    pub fn for_pad(pad: Option<ed_domain::station::PadSize>) -> Self {
        Timing {
            undock_seconds: Self::undock_for_pad(pad),
            ..Timing::default()
        }
    }

    /// What the pipeline prices with: the commander's own numbers stand
    /// as sent when `measured`; otherwise the undock term is the pad's
    /// default, whatever constant the wire carried. The pad is the
    /// request's `min_pad` — the client sends the ship's own.
    pub fn resolve(self, pad: Option<ed_domain::station::PadSize>) -> Self {
        if self.measured {
            self
        } else {
            Timing {
                undock_seconds: Self::undock_for_pad(pad),
                ..self
            }
        }
    }

    /// The bounds a shared server accepts: a value outside them is a
    /// broken client, not a fast pilot. NaN and absurd values fall back
    /// to the default term, not to the edge of the range.
    pub fn clamped(self) -> Self {
        let d = Timing::default();
        let term = |v: f64, default: f64, lo: f64, hi: f64| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                default
            }
        };
        Timing {
            jump_seconds: term(self.jump_seconds, d.jump_seconds, 5.0, 120.0),
            undock_seconds: term(self.undock_seconds, d.undock_seconds, 5.0, 600.0),
            docking_seconds: term(self.docking_seconds, d.docking_seconds, 5.0, 600.0),
            market_seconds: term(self.market_seconds, d.market_seconds, 0.0, 600.0),
            supercruise_base_seconds: term(
                self.supercruise_base_seconds,
                d.supercruise_base_seconds,
                5.0,
                600.0,
            ),
            supercruise_per_kls: term(self.supercruise_per_kls, d.supercruise_per_kls, 0.0, 60.0),
            supercruise_scale: term(self.supercruise_scale, d.supercruise_scale, 0.25, 4.0),
            measured: self.measured,
        }
    }

    /// Flat base plus a shallow per-1,000-ls slope, scaled: ≈150 s at
    /// the pad next door, ≈153 s at 5,000 ls, ≈200 s at 100,000 ls.
    pub fn supercruise_seconds(&self, distance_ls: f64) -> f64 {
        let kls = distance_ls.max(0.0) / 1_000.0;
        (self.supercruise_base_seconds + self.supercruise_per_kls * kls) * self.supercruise_scale
    }

    /// Time to fly `distance_ly` at `range_ly`, supercruise `arrival_ls`
    /// out, dock, and trade. Jump count is `ceil(distance / range)` with a
    /// floor of one for any move at all.
    pub fn leg_seconds_at_range(
        &self,
        distance_ly: f64,
        arrival_ls: f64,
        range_ly: f64,
    ) -> Duration {
        let jumps = if distance_ly <= 0.0 {
            0.0
        } else {
            (distance_ly / range_ly.max(1.0)).ceil().max(1.0)
        };
        let seconds = self.undock_seconds
            + jumps * self.jump_seconds
            + self.supercruise_seconds(arrival_ls)
            + self.docking_seconds
            + self.market_seconds;
        Duration {
            // The commander's own constants make the estimate theirs;
            // the documented defaults stay an estimate.
            confidence: if self.measured {
                Confidence::Measured
            } else {
                Confidence::Estimated
            },
            seconds,
        }
    }
}

/// How confident a duration is. Carried alongside every estimate so a caller
/// can say "about" rather than implying a stopwatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Derived from the commander's own journal.
    Measured,
    /// Documented approximation; shape is right, magnitude is not verified.
    Estimated,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Duration {
    pub seconds: f64,
    pub confidence: Confidence,
}

impl Duration {
    pub fn hours(&self) -> f64 {
        self.seconds / 3600.0
    }
}

/// Ship performance inputs to a route estimate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Ship {
    pub cargo_capacity: i64,
    /// Unladen jump range in ly (the Loadout figure at the time it was
    /// written). Used for empty legs.
    pub jump_range_ly: f64,
    /// Range with the hold full. Range scales with 1/mass, so a Cutter
    /// carrying 1,000 t loses a third of it; loaded legs use this.
    pub laden_range_ly: f64,
}

impl Ship {
    /// Laden range from the Loadout numbers: `max * (unladen + fuel) /
    /// (unladen + fuel + cargo)`. Falls back to the unladen range when the
    /// masses are unknown.
    pub fn laden_range(
        max_range: f64,
        unladen_mass: Option<f64>,
        fuel_t: Option<f64>,
        cargo_t: i64,
    ) -> f64 {
        match (unladen_mass, fuel_t) {
            (Some(m), Some(f)) if m > 0.0 => max_range * (m + f) / (m + f + cargo_t as f64),
            _ => max_range,
        }
    }
}

impl Default for Ship {
    fn default() -> Self {
        Ship {
            cargo_capacity: 0,
            jump_range_ly: 20.0,
            laden_range_ly: 20.0,
        }
    }
}

/// Time to fly `distance_ly`, then supercruise to a station `distance_ls`
/// out, dock, and trade.
///
/// Jump count is `ceil(distance / range)` with a floor of one for any move at
/// all: two systems 0.1 ly apart still cost a full jump.
pub fn leg_seconds(distance_ly: f64, arrival_ls: f64, ship: &Ship) -> Duration {
    leg_seconds_at_range(distance_ly, arrival_ls, ship.jump_range_ly)
}

/// Same, with an explicit jump range -- laden for a loaded leg -- at the
/// default timing.
pub fn leg_seconds_at_range(distance_ly: f64, arrival_ls: f64, range_ly: f64) -> Duration {
    Timing::default().leg_seconds_at_range(distance_ly, arrival_ls, range_ly)
}

/// Jumps needed to cover a distance at a given range.
pub fn jump_count(distance_ly: f64, jump_range_ly: f64) -> i64 {
    if distance_ly <= 0.0 {
        return 0;
    }
    (distance_ly / jump_range_ly.max(1.0)).ceil().max(1.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every timing term is optional on the wire and defaults to the
    /// documented constant; the commander's own constants change the
    /// leg by exactly their difference and stamp it `measured`; a
    /// shared server clamps the absurd and replaces the non-finite.
    #[test]
    fn timing_is_optional_per_field_and_measured_when_the_commander_says_so() {
        let omitted: Timing = serde_json::from_str("{}").unwrap();
        assert_eq!(omitted, Timing::default());
        let partial: Timing =
            serde_json::from_str(r#"{"jump_seconds": 30, "measured": true}"#).unwrap();
        assert_eq!(partial.jump_seconds, 30.0);
        assert_eq!(
            partial.docking_seconds, DOCKING_SECONDS,
            "an omitted field is its default"
        );
        let default_leg = Timing::default().leg_seconds_at_range(90.0, 50.0, 30.0);
        let own_leg = partial.leg_seconds_at_range(90.0, 50.0, 30.0);
        assert_eq!(default_leg.confidence, Confidence::Estimated);
        assert_eq!(own_leg.confidence, Confidence::Measured);
        assert!(
            (own_leg.seconds - default_leg.seconds - 3.0 * 12.0).abs() < 1e-9,
            "three jumps, twelve seconds slower each"
        );
        assert_eq!(
            leg_seconds_at_range(90.0, 50.0, 30.0).seconds,
            default_leg.seconds,
            "the free function is the default timing"
        );
        let wild = Timing {
            jump_seconds: 0.5,
            undock_seconds: 9_999.0,
            market_seconds: f64::NAN,
            ..Timing::default()
        }
        .clamped();
        assert_eq!(
            (wild.jump_seconds, wild.undock_seconds, wild.market_seconds),
            (5.0, 600.0, MARKET_SECONDS)
        );
    }

    /// Undock is keyed by the ship's pad unless the commander measured
    /// it: large 70, medium 60, small 50, unknown = medium.
    #[test]
    fn undock_follows_the_pad_until_it_is_measured() {
        use ed_domain::station::PadSize;
        assert_eq!(
            Timing::default()
                .resolve(Some(PadSize::Large))
                .undock_seconds,
            70.0
        );
        assert_eq!(
            Timing::default()
                .resolve(Some(PadSize::Medium))
                .undock_seconds,
            60.0
        );
        assert_eq!(
            Timing::default()
                .resolve(Some(PadSize::Small))
                .undock_seconds,
            50.0
        );
        assert_eq!(
            Timing::default().resolve(None).undock_seconds,
            UNDOCK_SECONDS
        );
        let own = Timing {
            undock_seconds: 81.0,
            measured: true,
            ..Timing::default()
        };
        assert_eq!(
            own.resolve(Some(PadSize::Small)).undock_seconds,
            81.0,
            "measured stands"
        );
        assert_eq!(
            Timing::for_pad(Some(PadSize::Large)).jump_seconds,
            JUMP_SECONDS,
            "only undock is pad-keyed"
        );
    }

    /// The supercruise scale multiplies the whole estimate, base and
    /// per-ls term alike, and the default is the documented shape. The
    /// maintainer's measured loop (Metz 5,392 ls ⇄ Lovell 357 ls, Panther
    /// Mk II, 2026-09-09) is the pin the scale is judged against.
    #[test]
    fn supercruise_scale_multiplies_the_whole_estimate() {
        let doubled = Timing {
            supercruise_scale: 2.0,
            ..Timing::default()
        };
        for ls in [0.0, 357.0, 5_392.0] {
            assert!(
                (doubled.supercruise_seconds(ls) - 2.0 * Timing::default().supercruise_seconds(ls))
                    .abs()
                    < 1e-9
            );
        }
        let far_pilot: Timing = serde_json::from_str(r#"{"supercruise_per_kls": 10}"#).unwrap();
        assert!(
            (far_pilot.supercruise_seconds(5_000.0) - 200.0).abs() < 1e-9,
            "the slope is a wire field too"
        );
    }

    #[test]
    fn supercruise_grows_slowly_and_is_never_free() {
        let near = supercruise_seconds(10.0);
        let mid = supercruise_seconds(1_000.0);
        let far = supercruise_seconds(100_000.0);
        assert!(near < mid && mid < far, "must be monotonic");
        assert!(supercruise_seconds(0.0) >= SUPERCRUISE_BASE_SECONDS);
        // A 100x distance increase must not cost 100x the time: the fit
        // is a flat base with a shallow slope.
        assert!(
            far < mid * 2.0,
            "growth is a shallow slope, never proportional"
        );
        // The fitted shape against the first measured loop (2026-09-09,
        // Panther Mk II): 98 s flown at 357 ls, 139 s at 5,392 ls.
        assert!((supercruise_seconds(357.0) - 150.2).abs() < 0.1);
        assert!((supercruise_seconds(5_392.0) - 152.7).abs() < 0.1);
    }

    #[test]
    fn any_move_at_all_costs_a_full_jump() {
        // Two systems 0.1 ly apart are still one jump, not a fraction of one.
        assert_eq!(jump_count(0.1, 50.0), 1);
        assert_eq!(jump_count(0.0, 50.0), 0);
        assert_eq!(jump_count(50.0, 50.0), 1);
        assert_eq!(jump_count(51.0, 50.0), 2);
        assert_eq!(jump_count(100.0, 25.0), 4);
    }

    #[test]
    fn a_distant_station_can_cost_more_than_extra_jumps() {
        let ship = Ship {
            cargo_capacity: 700,
            jump_range_ly: 30.0,
            laden_range_ly: 30.0,
        };
        // Close station, far away in ly.
        let far_close = leg_seconds(90.0, 50.0, &ship);
        // Near system, but the station is 250,000 ls out.
        let near_far = leg_seconds(15.0, 250_000.0, &ship);
        assert!(
            near_far.seconds > far_close.seconds,
            "a 250k ls arrival ({:.0}s) should beat three jumps ({:.0}s) for cost",
            near_far.seconds,
            far_close.seconds
        );
    }

    #[test]
    fn laden_range_scales_with_mass() {
        // A 1,000 t hold on a 1,000 t hull with 100 t of fuel: 1100/2100 of the range.
        let r = Ship::laden_range(37.6, Some(1000.0), Some(100.0), 1000);
        assert!((r - 37.6 * 1100.0 / 2100.0).abs() < 1e-9);
        assert_eq!(
            Ship::laden_range(37.6, None, None, 1000),
            37.6,
            "unknown masses: no guess"
        );
    }

    #[test]
    fn estimates_never_claim_to_be_measured() {
        let ship = Ship::default();
        // The supercruise and market terms are estimated, so no leg
        // estimate may present itself as measured.
        assert_eq!(
            leg_seconds(30.0, 100.0, &ship).confidence,
            Confidence::Estimated
        );
    }
}
