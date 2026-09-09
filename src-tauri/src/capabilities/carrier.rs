//! The commander's fleet carrier, first-hand from the journal (Item 52 A).
//! One read shared by the ship computer's tool, the `carrier_status`
//! command and the Ships tab card, so all three say the same thing.

use super::CapResult;
use crate::state::AppState;
use serde_json::{json, Value};

/// Every carrier the journal knows, owned first, every figure aged from
/// now. An empty list says why, so the model never has to guess.
pub fn status(state: &AppState) -> CapResult<Value> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    state.with_read(|s| {
        let carriers = ed_store::carrier::status(s.conn(), &now)?;
        let note = if carriers.is_empty() {
            "EDDA has not seen a carrier in the journal yet. Owning one writes CarrierBuy; opening Carrier Management once writes CarrierStats (tank, capacity, services, balance)."
        } else {
            "Journal-derived: every figure carries as_of and age_hours — say the age. hold_moved is what the commander moved aboard, not the full hold; other commanders' deposits and sales are invisible until the Frontier link exists."
        };
        Ok(json!({ "carriers": carriers, "note": note, "provenance": "journal" }))
    })
}
