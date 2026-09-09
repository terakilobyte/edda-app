//! The complete Status.json `Flags` and `Flags2` bit tables.
//!
//! Ported from EDDI (github.com/EDCD/EDDI, `DataDefinitions/Status.cs`),
//! Copyright the EDDI contributors, licensed Apache-2.0 — see
//! docs/THIRD-PARTY.md. One authoritative module instead of magic
//! numbers scattered through the callout rules; the names follow the
//! journal documentation.

#![allow(dead_code)]

// Flags (bit 0..31).
pub const DOCKED: i64 = 1 << 0;
pub const LANDED: i64 = 1 << 1;
pub const LANDING_GEAR_DOWN: i64 = 1 << 2;
pub const SHIELDS_UP: i64 = 1 << 3;
pub const SUPERCRUISE: i64 = 1 << 4;
pub const FLIGHT_ASSIST_OFF: i64 = 1 << 5;
pub const HARDPOINTS_DEPLOYED: i64 = 1 << 6;
pub const IN_WING: i64 = 1 << 7;
pub const LIGHTS_ON: i64 = 1 << 8;
pub const CARGO_SCOOP_DEPLOYED: i64 = 1 << 9;
pub const SILENT_RUNNING: i64 = 1 << 10;
pub const SCOOPING_FUEL: i64 = 1 << 11;
pub const SRV_HANDBRAKE: i64 = 1 << 12;
pub const SRV_TURRET: i64 = 1 << 13;
pub const SRV_UNDER_SHIP: i64 = 1 << 14;
pub const SRV_DRIVE_ASSIST: i64 = 1 << 15;
pub const FSD_MASS_LOCKED: i64 = 1 << 16;
pub const FSD_CHARGING: i64 = 1 << 17;
pub const FSD_COOLDOWN: i64 = 1 << 18;
pub const LOW_FUEL: i64 = 1 << 19; // < 25 %
pub const OVERHEATING: i64 = 1 << 20; // > 100 %
pub const HAS_LAT_LONG: i64 = 1 << 21;
pub const IN_DANGER: i64 = 1 << 22;
pub const BEING_INTERDICTED: i64 = 1 << 23;
pub const IN_MAIN_SHIP: i64 = 1 << 24;
pub const IN_FIGHTER: i64 = 1 << 25;
pub const IN_SRV: i64 = 1 << 26;
pub const HUD_ANALYSIS_MODE: i64 = 1 << 27;
pub const NIGHT_VISION: i64 = 1 << 28;
pub const ALTITUDE_FROM_AVERAGE_RADIUS: i64 = 1 << 29;
pub const HYPERSPACE: i64 = 1 << 30;
pub const SRV_HIGH_BEAM: i64 = 1 << 31;

// Flags2 (bit 0..22).
pub const ON_FOOT: i64 = 1 << 0;
pub const IN_TAXI: i64 = 1 << 1;
pub const IN_MULTICREW: i64 = 1 << 2;
pub const ON_FOOT_IN_STATION: i64 = 1 << 3;
pub const ON_FOOT_ON_PLANET: i64 = 1 << 4;
pub const AIM_DOWN_SIGHT: i64 = 1 << 5;
pub const LOW_OXYGEN: i64 = 1 << 6;
pub const LOW_HEALTH: i64 = 1 << 7;
pub const COLD: i64 = 1 << 8;
pub const HOT: i64 = 1 << 9;
pub const VERY_COLD: i64 = 1 << 10;
pub const VERY_HOT: i64 = 1 << 11;
pub const GLIDE_MODE: i64 = 1 << 12;
pub const ON_FOOT_IN_HANGAR: i64 = 1 << 13;
pub const ON_FOOT_IN_SOCIAL_SPACE: i64 = 1 << 14;
pub const ON_FOOT_EXTERIOR: i64 = 1 << 15;
pub const BREATHABLE_ATMOSPHERE: i64 = 1 << 16;
pub const TELEPRESENCE_MULTICREW: i64 = 1 << 17;
pub const PHYSICAL_MULTICREW: i64 = 1 << 18;
pub const FSD_HYPERDRIVE_CHARGING: i64 = 1 << 19;
/// Supercruise Overcharge (SCO) active. MEASURED live 2026-09-01
/// (Mandalay, 5A SCO, Wongi): two separate overcharges each raised and
/// dropped exactly this bit (Flags2 0 <-> 0x100000, no co-firing with
/// bit 19 charging or bit 21 assist -- assist was fitted), and the
/// windows carried the overcharge drain signature: `ReservoirReplenished`
/// every ~7 s / 0.5 t (~0.07 t/s against the ship's 0.083 t/s ScoBurn
/// prior; normal supercruise is an order of magnitude slower). EDDI's
/// table said the same; now it is our own measurement.
pub const FSD_SCO_ACTIVE: i64 = 1 << 20;
/// Supercruise assist FLYING the ship -- not merely fitted or enabled:
/// measured live 2026-09-01 (Mandalay, Wongi), the bit is 0 with the
/// module on until assist takes over toward a locked destination, then
/// rises/falls exactly alone on engage/disengage (two clean pairs). It
/// stayed up through an interdiction tether and evasion. Mutually
/// exclusive with [`FSD_SCO_ACTIVE`] by game mechanics: the SCO will
/// not fire while assist is engaged (verified in the cockpit), so
/// 0x300000 should never occur.
pub const FSD_SUPERCRUISE_ASSIST: i64 = 1 << 21;
pub const NPC_CREW_ACTIVE: i64 = 1 << 22;

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the bits the app actually acts on against the documented hex
    /// values, so a table edit cannot silently move a guard.
    #[test]
    fn acted_on_bits_match_the_documented_values() {
        assert_eq!(LOW_FUEL, 0x0008_0000);
        assert_eq!(OVERHEATING, 0x0010_0000);
        assert_eq!(IN_DANGER, 0x0040_0000);
        assert_eq!(SCOOPING_FUEL, 0x0000_0800);
        assert_eq!(FSD_HYPERDRIVE_CHARGING, 0x0008_0000);
        assert_eq!(FSD_SCO_ACTIVE, 0x0010_0000);
        assert_eq!(FSD_SUPERCRUISE_ASSIST, 0x0020_0000);
    }
}
