//! The whole galaxy, small enough to route across.
//!
//! Spansh's full dump is 116 GB compressed and carries every body of every
//! system. Routing needs four things per system -- id, name, position and
//! the main star's class -- so [`import`] streams the dump once and keeps
//! only those, in a flat binary index that is memory-mapped by [`format`]
//! and queried through a grid. The JSON is never stored.
//!
//! [`router`] plans jump routes over that index: plain A* for point to
//! point, with neutron-star and white-dwarf supercharging when allowed,
//! and cooperative cancel/progress hooks because a 20,000 ly plot is not
//! instant.

pub mod agg;
pub mod alt;
pub mod boost_side;
pub mod carrier;
pub mod cgraph;
pub mod fuel;
pub mod format;
pub mod import;
pub mod loadout;
pub mod overlay;
pub mod presence;
pub mod router;
pub mod long_range;
pub mod star;

pub use format::{Galaxy, StarRecord};
pub use star::{StarClass, StarClassCode};

/// Configure rayon's global pool for the planner: leave two cores for the
/// UI and the journal, and run each worker through `on_start` (used to
/// drop the thread's priority) so a two-minute plot never starves the app.
/// Call once at startup; later calls are ignored.
pub fn init_thread_pool(on_start: fn()) {
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let n = cpus.saturating_sub(2).max(1);
    let _ = rayon::ThreadPoolBuilder::new().num_threads(n).thread_name(|i| format!("planner-{i}")).start_handler(move |_| on_start()).build_global();
}
