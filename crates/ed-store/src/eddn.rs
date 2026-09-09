//! Compatibility facade for callers that still apply decoded EDDN envelopes.

pub use crate::sqlite::*;

use anyhow::Result;
use ed_eddn::Envelope;
use rusqlite::Connection;

pub fn apply(connection: &Connection, envelope: &Envelope) -> Result<Applied> {
    let Some(operation) = envelope.operation() else {
        return Ok(Applied {
            skipped: 1,
            ..Applied::default()
        });
    };
    apply_operation(connection, &operation)
}
