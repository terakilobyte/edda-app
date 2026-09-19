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
        // The Frontier link's last /fleetcarrier answer, if the commander
        // linked: the real hold, tank, balance and orders, with fetched_at.
        let live = ed_store::carrier_capi::load(s.conn())?;
        let note = if carriers.is_empty() {
            "EDDA has not seen a carrier in the journal yet. Owning one writes CarrierBuy; opening Carrier Management once writes CarrierStats (tank, capacity, services, balance)."
        } else if live.is_some() {
            "Journal-derived figures carry as_of and age_hours. `live` is Frontier's own report of the commander's carrier (hold per commodity in tonnes, tank, balance, orders, services) with fetched_at — prefer it for the hold, and say when it was fetched."
        } else {
            "Journal-derived: every figure carries as_of and age_hours — say the age. The hold is not known: the journal only sees the commander's own transfers. A Frontier link (Settings → Frontier account) fetches the real one."
        };
        Ok(json!({ "carriers": carriers, "live": live, "note": note, "provenance": if live.is_some() { "journal+capi" } else { "journal" } }))
    })
}
