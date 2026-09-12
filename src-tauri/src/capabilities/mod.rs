//! The capability layer: one typed function per thing the app can do.
//!
//! `docs/PLAN.md` §4.1: the tool surface is the product, and it has two
//! consumers -- the ship computer (JSON in, JSON out, over the model's
//! wire) and the panels (Tauri IPC). Both used to re-implement the same
//! calls against the same crate functions with their own argument parsing,
//! defaults and error strings, and the two drifted. Now each operation is
//! a typed request struct (defaults in exactly one place), a typed
//! response, and a [`CapError`]; the Tauri command deserialises, calls,
//! serialises, and the tool registry in [`tools`] does the same from the
//! model's input. A tool the model can call but nobody wired up cannot
//! exist: `tool_definitions()` is derived from the registry.

use crate::ai::Effects;
use crate::state::AppState;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

pub mod carrier;
pub mod commander;
pub mod galaxy;
pub mod tools;
pub mod trade;

/// What went wrong, apart from which kind of wrong.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Detail {
    /// For a person. Clean prose, no instructions to a model in it.
    pub message: String,
    /// For whoever retries -- what to change. Structured, so the model is
    /// guided without the message reading like a prompt.
    pub hint: Option<String>,
    /// Structured context worth returning with the error (valid choices,
    /// the list that was empty...).
    pub data: Option<Value>,
}

impl std::fmt::Display for Detail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Every capability fails with one of these. The kind decides what the
/// caller does: a panel shows `NotFound` quietly, retries `Unavailable`
/// when `retryable`, and reports `Internal`; the model reads the same
/// fields instead of parsing English.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CapError {
    #[error("{0}")]
    NotFound(Detail),
    #[error("{0}")]
    InvalidInput(Detail),
    #[error("{detail}")]
    Unavailable { detail: Detail, retryable: bool },
    #[error("{0}")]
    Internal(Detail),
}

pub type CapResult<T> = Result<T, CapError>;

impl CapError {
    pub fn not_found(message: impl Into<String>) -> Self {
        CapError::NotFound(Detail {
            message: message.into(),
            ..Default::default()
        })
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        CapError::InvalidInput(Detail {
            message: message.into(),
            ..Default::default()
        })
    }
    pub fn unavailable(message: impl Into<String>, retryable: bool) -> Self {
        CapError::Unavailable {
            detail: Detail {
                message: message.into(),
                ..Default::default()
            },
            retryable,
        }
    }
    pub fn internal(message: impl Into<String>) -> Self {
        CapError::Internal(Detail {
            message: message.into(),
            ..Default::default()
        })
    }
    /// Attach guidance for the retry.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.detail_mut().hint = Some(hint.into());
        self
    }
    /// Attach structured context.
    pub fn data(mut self, data: Value) -> Self {
        self.detail_mut().data = Some(data);
        self
    }
    pub fn kind(&self) -> &'static str {
        match self {
            CapError::NotFound(_) => "not_found",
            CapError::InvalidInput(_) => "invalid_input",
            CapError::Unavailable { .. } => "unavailable",
            CapError::Internal(_) => "internal",
        }
    }
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            CapError::Unavailable {
                retryable: true,
                ..
            }
        )
    }
    pub fn message(&self) -> &str {
        &self.detail().message
    }
    pub fn hint_text(&self) -> Option<&str> {
        self.detail().hint.as_deref()
    }
    fn detail(&self) -> &Detail {
        match self {
            CapError::NotFound(d) | CapError::InvalidInput(d) | CapError::Internal(d) => d,
            CapError::Unavailable { detail, .. } => detail,
        }
    }
    fn detail_mut(&mut self) -> &mut Detail {
        match self {
            CapError::NotFound(d) | CapError::InvalidInput(d) | CapError::Internal(d) => d,
            CapError::Unavailable { detail, .. } => detail,
        }
    }
}

/// The wire shape, for IPC and for the model: `{kind, message, retryable,
/// hint?, data?}`. `message` alone is what the panels used to receive as a
/// bare string, so `ApiError.message` reads the same as before.
#[derive(Serialize)]
struct Wire<'a> {
    kind: &'static str,
    message: &'a str,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a Value>,
}

impl Serialize for CapError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        Wire {
            kind: self.kind(),
            message: self.message(),
            retryable: self.retryable(),
            hint: self.hint_text(),
            data: self.detail().data.as_ref(),
        }
        .serialize(s)
    }
}

impl From<rusqlite::Error> for CapError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => CapError::not_found("no such record"),
            e => CapError::internal(e.to_string()),
        }
    }
}

impl From<anyhow::Error> for CapError {
    fn from(e: anyhow::Error) -> Self {
        CapError::internal(format!("{e:#}"))
    }
}

impl From<serde_json::Error> for CapError {
    fn from(e: serde_json::Error) -> Self {
        CapError::invalid(format!("malformed input: {e}"))
            .hint("check the argument names and types against the tool schema")
    }
}

/// The legacy shape: a bare string from a helper that has not been typed
/// yet. Treated as internal -- nothing in a string says it is retryable.
impl From<String> for CapError {
    fn from(e: String) -> Self {
        CapError::internal(e)
    }
}

impl From<ed_route::request::ProfitRequestError> for CapError {
    fn from(e: ed_route::request::ProfitRequestError) -> Self {
        CapError::invalid(e.to_string()).hint(e.hint())
    }
}

/// What a tool runs against: the app and the side-effect seam.
pub struct Ctx<'a> {
    pub state: &'a AppState,
    pub fx: &'a dyn Effects,
}

/// One registered tool. The name, the description and the schema are what
/// the model sees; `run` is what happens. They live in one row so nothing
/// can be described and not runnable, or runnable and not described.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
    pub run: fn(&Ctx<'_>, &Value) -> CapResult<Value>,
}

pub fn registry() -> &'static [ToolSpec] {
    tools::REGISTRY
}

pub fn find(name: &str) -> Option<&'static ToolSpec> {
    registry().iter().find(|t| t.name == name)
}

/// The model-facing tool table, derived from the registry.
pub fn tool_definitions() -> Value {
    Value::Array(
        registry()
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": (t.schema)() }))
            .collect(),
    )
}

/// A tool's input as its typed request.
pub fn parse<T: DeserializeOwned>(input: &Value) -> CapResult<T> {
    Ok(serde_json::from_value(input.clone())?)
}

/// An error as the model sees it: `{"error": {kind, message, retryable, hint?, data?}}`.
pub fn error_value(e: &CapError) -> Value {
    json!({ "error": e })
}

/// Run one tool by name. Never fails: an error is a value the model can
/// read, with its kind and hint intact.
pub fn execute(state: &AppState, fx: &dyn Effects, name: &str, input: &Value) -> Value {
    let ctx = Ctx { state, fx };
    let Some(spec) = find(name) else {
        return error_value(
            &CapError::not_found(format!("unknown tool: {name}"))
                .hint("call one of the tools you were given")
                .data(json!(registry().iter().map(|t| t.name).collect::<Vec<_>>())),
        );
    };
    match tools::canonical_input(state, input).and_then(|input| (spec.run)(&ctx, &input)) {
        Ok(v) => v,
        Err(e) => error_value(&e),
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A real `AppState` over a temporary database with a small galaxy: an
    //! origin with a starport, a neighbour with a large-pad outpost that
    //! pays well for gold, and a commander docked at the origin.
    use super::*;

    pub struct Fixture {
        pub state: AppState,
        _dir: tempfile::TempDir,
    }

    pub fn fixture(hull: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&journal).unwrap();
        let db = data.join("edda.sqlite3");
        let store = ed_store::Store::open(&db, &journal).unwrap();
        let conn = store.conn();
        conn.execute_batch(&format!(
            "INSERT INTO sys_systems (id64,name,x,y,z) VALUES (1,'Origin',0,0,0),(2,'Near',10,0,0);
             INSERT INTO sys_stations (id,system_id64,name,type,distance_to_arrival,pad_large,pad_medium,pad_small,has_market,is_carrier) VALUES
               (10,1,'Home Port','Coriolis Starport',100,8,10,10,1,0),
               (20,2,'Big Dock','Orbis Starport',300,8,8,8,1,0);
             INSERT INTO sys_commodities(symbol,name) VALUES ('gold','Gold');
             WITH v(station_id,symbol,buy,sell,demand,supply) AS (VALUES
               (10,'gold',9000,8500,0,5000),
               (20,'gold',0,12000,3000,0))
             INSERT INTO sys_market (station_id,commodity_id,buy_price,sell_price,demand,supply,updated)
             SELECT v.station_id,c.id,v.buy,v.sell,v.demand,v.supply,unixepoch('now')-3600 FROM v JOIN sys_commodities c ON c.symbol=v.symbol;
             INSERT INTO location (id, ts, system_name, docked, station_name) VALUES (1,'2026-08-28T00:00:00Z','Origin',1,'Home Port');
             INSERT INTO loadout (id, ts, ship, ship_name, cargo_capacity, unladen_mass, max_jump_range) VALUES (1,'2026-08-28T00:00:00Z','{hull}','Test Ship',200,400.0,20.0);
             INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 1, '2026-08-28T00:00:00Z', 'Loadout',
               '{{\"event\":\"Loadout\",\"ShipID\":1,\"Ship\":\"{hull}\",\"CargoCapacity\":200,\"MaxJumpRange\":20.0,\"UnladenMass\":400.0,\"FuelCapacity\":{{\"Main\":16.0}}}}');"
        ))
        .unwrap();
        let state = AppState::new(store, data, db);
        Fixture { state, _dir: dir }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Recording;

    #[test]
    fn every_definition_has_a_runner_and_vice_versa() {
        let defs = tool_definitions();
        let defined: Vec<&str> = defs
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        let registered: Vec<&str> = registry().iter().map(|t| t.name).collect();
        assert_eq!(
            defined, registered,
            "definitions are derived from the registry"
        );
        let mut names = registered.clone();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), registered.len(), "duplicate tool names");
        for d in defs.as_array().unwrap() {
            assert!(
                d["input_schema"]["type"] == "object",
                "{} schema",
                d["name"]
            );
            assert!(
                !d["description"].as_str().unwrap().is_empty(),
                "{} description",
                d["name"]
            );
            assert!(find(d["name"].as_str().unwrap()).is_some());
        }
    }

    #[test]
    fn an_unknown_tool_is_a_typed_not_found_the_model_can_read() {
        let fx = Recording::default();
        let f = testing::fixture("cutter");
        let out = execute(&f.state, &fx, "find_sytsem", &json!({ "name": "Sol" }));
        assert_eq!(out["error"]["kind"], "not_found");
        assert_eq!(out["error"]["retryable"], false);
        assert!(out["error"]["hint"].is_string());
        assert!(out["error"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "find_system"));
    }

    #[test]
    fn errors_serialise_as_kind_message_retryable_hint() {
        let e = CapError::unavailable("galaxy index not built yet", true)
            .hint("build it under Settings");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            json!({ "kind": "unavailable", "message": "galaxy index not built yet", "retryable": true, "hint": "build it under Settings" })
        );
        let e = CapError::not_found("no such system");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({ "kind": "not_found", "message": "no such system", "retryable": false })
        );
        // The message never carries the guidance.
        let e: CapError = ed_route::request::ProfitRequestError::NoCargoRacks.into();
        assert!(
            !e.message().contains("pass cargo_capacity"),
            "{}",
            e.message()
        );
        assert!(e.hint_text().unwrap().contains("cargo_capacity"));
    }

    #[test]
    fn malformed_tool_input_is_invalid_input_with_a_hint() {
        let fx = Recording::default();
        let f = testing::fixture("cutter");
        let out = execute(
            &f.state,
            &fx,
            "systems_near",
            &json!({ "system": "Origin", "radius_ly": "twenty" }),
        );
        assert_eq!(out["error"]["kind"], "invalid_input", "{out}");
        assert!(out["error"]["hint"].is_string());
    }

    /// The trade tab can plan for a stored ship (maintainer, 2026-09-07): a
    /// ship_id the journal knows selects that Loadout; one it never saw
    /// falls back to the live ship rather than refusing.
    #[test]
    fn a_profit_search_can_plan_for_a_stored_ship() {
        let f = testing::fixture("cutter");
        let conn = f.state.read_conn().unwrap();
        let live = trade::live_ship(&conn);
        let raw: String = conn
            .query_row(
                "SELECT raw FROM events WHERE event = 'Loadout' ORDER BY ts DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let id = serde_json::from_str::<Value>(&raw).unwrap()["ShipID"]
            .as_i64()
            .unwrap();
        let mut req = ed_route::request::ProfitRequest::default();
        req.ship_id = Some(id);
        assert_eq!(
            trade::ship_for(&conn, &req),
            live,
            "the flown ship's own id is the live ship"
        );
        req.ship_id = Some(999_999_999);
        assert_eq!(
            trade::ship_for(&conn, &req),
            live,
            "an unknown id falls back to the live ship"
        );
        req.ship_id = None;
        assert_eq!(trade::ship_for(&conn, &req), live);
    }

    /// The planner's hand-coded FSD injection grades (ed_galaxy::router::
    /// INJECTION_RECIPES) must agree with the synthesis library, so the
    /// library is the one place a recipe change is made.
    #[test]
    fn the_injection_table_matches_the_synthesis_library() {
        let catalog = ed_engineering::Catalog::load();
        let fsd = catalog.find_synthesis("FSD Injection").unwrap();
        for (_, grade, mats) in ed_galaxy::router::INJECTION_RECIPES {
            let lib = fsd
                .grades
                .iter()
                .find(|g| g.grade.eq_ignore_ascii_case(grade))
                .unwrap_or_else(|| panic!("{grade}"));
            let mut a: Vec<String> = mats.iter().map(|m| m.to_ascii_lowercase()).collect();
            let mut b: Vec<String> = lib
                .ingredients
                .iter()
                .map(|i| i.name.to_ascii_lowercase())
                .collect();
            a.sort();
            b.sort();
            assert_eq!(a, b, "{grade}");
        }
    }

    /// synthesis_recipes answers with grades, costs and how many the
    /// commander can make now; an unknown name lists what exists.
    #[test]
    fn synthesis_recipes_tool_answers_from_the_library() {
        let f = testing::fixture("cutter");
        let fx = Recording::default();
        let out = execute(&f.state, &fx, "synthesis_recipes", &json!({}));
        assert!(out["recipes"].as_array().unwrap().len() >= 25, "{out}");
        let out = execute(
            &f.state,
            &fx,
            "synthesis_recipes",
            &json!({ "name": "heat sink" }),
        );
        assert_eq!(out["recipe"]["name"], "Heat Sink", "{out}");
        assert_eq!(out["recipe"]["grades"].as_array().unwrap().len(), 3);
        assert!(
            out["recipe"]["grades"][0]["ingredients"][0]["need"]
                .as_i64()
                .unwrap()
                > 0
        );
        assert!(out["recipe"]["grades"][0]["can_make_now"].is_number());
        assert_eq!(out["provenance"], "vendored");
        let out = execute(
            &f.state,
            &fx,
            "synthesis_recipes",
            &json!({ "name": "teleporter" }),
        );
        assert_eq!(out["error"]["kind"], "invalid_input", "{out}");
        assert!(out["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("FSD Injection"));
    }

    /// The no-EDMC route path: a jump's StarPos becomes the planner's
    /// coordinates, and a name the journal never saw yields nothing
    /// (the query runs against the fixture's real events table).
    #[test]
    fn the_journal_knows_where_a_visited_system_is() {
        let jump = json!({ "event": "FSDJump", "StarSystem": "Shui Wei Sector DG-O b6-0", "StarPos": [119.09375, -135.0, 82.03125] });
        assert_eq!(
            crate::routing::star_pos(&jump),
            Some([119.09375, -135.0, 82.03125])
        );
        assert_eq!(
            crate::routing::star_pos(&json!({ "event": "Docked" })),
            None
        );
        let f = testing::fixture("cutter");
        let conn = f.state.read_conn().unwrap();
        assert!(crate::routing::journal_coords(&conn, "Nowhere Sector ZZ-Z z0-0").is_none());
    }

    #[test]
    fn find_profit_with_an_unknown_hull_does_not_return_large_pad_stations() {
        let fx = Recording::default();
        // A known hull gets past the pad check to the API stage (which the
        // unit suite has no endpoint for: `unavailable`, never a
        // filter-less search and never prod).
        let known = testing::fixture("cutter");
        let out = execute(&known.state, &fx, "find_profit", &json!({}));
        assert_eq!(out["error"]["kind"], "unavailable", "{out}");
        // Unknown hull: refuse with a typed error, never a filter-less search.
        let odd = testing::fixture("fdev_next_hull");
        let out = execute(&odd.state, &fx, "find_profit", &json!({}));
        assert!(out.get("legs").is_none(), "{out}");
        assert_eq!(out["error"]["kind"], "invalid_input");
        assert!(out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("fdev_next_hull"));
        assert!(out["error"]["hint"].as_str().unwrap().contains("min_pad"));
        // The panel's command goes through the same capability.
        let req = ed_route::request::ProfitRequest::default();
        let err = tauri::async_runtime::block_on(crate::remote_trade::report(&odd.state, &req))
            .unwrap_err();
        assert!(matches!(err, CapError::InvalidInput(_)), "{err:?}");
    }

    #[test]
    fn market_search_with_an_unknown_hull_fails_closed_too() {
        let fx = Recording::default();
        let odd = testing::fixture("fdev_next_hull");
        let out = execute(
            &odd.state,
            &fx,
            "market_search",
            &json!({ "kind": "commodity", "text": "Gold" }),
        );
        assert_eq!(out["error"]["kind"], "invalid_input", "{out}");
        // ...and an explicit pad is honoured: the search reaches the API
        // stage (no endpoint under test, so `unavailable`).
        let out = execute(
            &odd.state,
            &fx,
            "market_search",
            &json!({ "kind": "commodity", "text": "Gold", "min_pad": "large" }),
        );
        assert_eq!(out["error"]["kind"], "unavailable", "{out}");
    }

    #[test]
    fn command_and_tool_share_one_default_radius() {
        // The literal used to live in both commands.rs and ai.rs.
        let req: galaxy::NearestServiceRequest = parse(&json!({ "service": "shipyard" })).unwrap();
        assert_eq!(
            req.radius_ly,
            galaxy::NearestServiceRequest::default().radius_ly
        );
        assert_eq!(galaxy::NearestServiceRequest::default().radius_ly, 50.0);
        assert_eq!(galaxy::SystemsNearRequest::default().radius_ly, 20.0);
    }
}
