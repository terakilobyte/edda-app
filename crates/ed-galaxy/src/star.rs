//! Main-star classes for the router.
//!
//! The enum itself -- parsing from the journal and Spansh, scoopability,
//! supercharge multiplier, hazards -- lives in [`ed_domain::star`] so the
//! store and the app answer those questions from the same table. What is
//! ours is the 4-bit code each class takes in the EDGX star file
//! (`docs/BINARY-FORMATS.md`); [`StarClassCode`] is that mapping.

pub use ed_domain::star::StarClass;
/// The EDGX nibble for a class — the contract itself moved to
/// `ed_domain::star` (2026-09-09) so the store's EDDN writer can teach
/// `stars.class` without depending on the router; re-exported here for
/// every existing caller.
pub use ed_domain::star::StarClassCode;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for c in 0..=15u8 {
            assert_eq!(StarClass::from_code(c).code(), c);
        }
    }

    /// The nibble values are the on-disk contract.
    #[test]
    fn codes_are_stable() {
        assert_eq!(StarClass::Unknown.code(), 0);
        assert_eq!(StarClass::K.code(), 6);
        assert_eq!(StarClass::Proto.code(), 11);
        assert_eq!(StarClass::Exotic.code(), 12);
        assert_eq!(StarClass::WhiteDwarf.code(), 13);
        assert_eq!(StarClass::Neutron.code(), 14);
        assert_eq!(StarClass::BlackHole.code(), 15);
        assert_eq!(StarClass::from_code(99), StarClass::Unknown);
    }
}
