//! Route planning: what a route costs, and where the money is.
//!
//! * [`cost`] -- wall-clock cost of jumps, supercruise and docking, with
//!   honest confidence labels.
//! * [`ships`] -- landing pad size per hull, so a route never sends a Cutter
//!   to an outpost.
//! * [`profit`] -- the profit finder: best single legs and round trips from
//!   where the commander is, scored in credits per hour.

pub mod cost;
pub mod profit;
pub mod request;
pub mod ships;
