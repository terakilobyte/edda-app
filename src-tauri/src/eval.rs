//! Ship-computer eval: scripted cockpit questions with expectations, run
//! against whatever provider is configured, scored and written out.
//!
//! The point is tuning the *tool surface* -- names, descriptions, result
//! shapes, the system prompt -- until a small local model gets them right;
//! what a 12B model needs spelled out, a frontier model handles too, so
//! the improvements transfer. Expectations are grounded in the commander's
//! own journal (the current system, the real combat rank...) so a pass
//! means the model used our data rather than guessed.
//!
//! Dev hook (debug builds): drop a `run.json` into `.data/eval/` -- `{}` for
//! the whole suite, `{"only": ["where", "ranks"]}` for some -- and the
//! running app picks it up within a few seconds and writes `report.json`
//! beside it. Settings has a button that does the same.

use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

/// One question and what a good answer looks like.
struct Case {
    id: &'static str,
    question: &'static str,
    /// At least one of these must be called (empty = no requirement).
    expect_tools: &'static [&'static str],
    /// None of these may be called.
    forbid_tools: &'static [&'static str],
    /// The answer must contain at least one of these (case-insensitive).
    must_contain: &'static [&'static str],
    /// The answer must contain this field of this tool's live output.
    must_contain_from: Option<(&'static str, &'static str)>,
    /// A recorded side effect that must have happened (substring), e.g. "press".
    expect_effect: Option<&'static str>,
    /// Spoken replies: keep it short.
    max_chars: usize,
}

const CASES: &[Case] = &[
    Case { id: "where", question: "Where am I right now?", expect_tools: &["get_ship_status"], forbid_tools: &["plot_route", "find_profit"], must_contain: &[], must_contain_from: Some(("get_ship_status", "/location/system_name")), expect_effect: None, max_chars: 400 },
    Case { id: "ship", question: "What ship am I flying?", expect_tools: &["get_ship_status"], forbid_tools: &[], must_contain: &[], must_contain_from: Some(("get_ship_status", "/ship")), expect_effect: None, max_chars: 300 },
    Case { id: "ranks", question: "What is my combat rank?", expect_tools: &["commander_ranks"], forbid_tools: &["combat_stats"], must_contain: &[], must_contain_from: Some(("commander_ranks", "/ranks/ranks/0/name")), expect_effect: None, max_chars: 400 },
    Case { id: "elite-gap", question: "How far away from having Elite rank in combat am I?", expect_tools: &["commander_ranks"], forbid_tools: &["plot_route", "current_route"], must_contain: &["elite"], must_contain_from: None, expect_effect: None, max_chars: 600 },
    Case { id: "route", question: "How many jumps are left on my route?", expect_tools: &["current_route", "follow_route"], forbid_tools: &["plot_route"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 400 },
    Case { id: "inventory-one", question: "How much tellurium do I have?", expect_tools: &["get_inventory"], forbid_tools: &["material_sources"], must_contain: &["tellurium"], must_contain_from: None, expect_effect: None, max_chars: 300 },
    Case { id: "sources", question: "Where can I farm tellurium?", expect_tools: &["material_sources"], forbid_tools: &[], must_contain: &["tellurium", "shard", "hip 36601"], must_contain_from: None, expect_effect: None, max_chars: 800 },
    Case { id: "fsd-gap", question: "What am I short of for grade 5 increased FSD range on my frame shift drive?", expect_tools: &["get_engineering_gap", "material_shopping_list"], forbid_tools: &[], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 1200 },
    Case { id: "shopping", question: "Give me a shopping list for grade 5 dirty drives on my thrusters, trading what I can at material traders.", expect_tools: &["material_shopping_list"], forbid_tools: &[], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 1500 },
    Case { id: "engineers", question: "Which engineers have I unlocked?", expect_tools: &["list_engineers"], forbid_tools: &[], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 800 },
    Case { id: "shipyard", question: "Where's the nearest station with a shipyard?", expect_tools: &["nearest_service"], forbid_tools: &["find_profit"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 500 },
    Case { id: "trade", question: "What's the best trade run from here within 40 light years?", expect_tools: &["find_profit"], forbid_tools: &[], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 900 },
    // Item 52 C: the carrier router, never the ship router, for a carrier.
    Case { id: "carrier-move", question: "Move my carrier to Deciat.", expect_tools: &["plot_carrier_route"], forbid_tools: &["plot_route"], must_contain: &["deciat"], must_contain_from: None, expect_effect: None, max_chars: 700 },
    Case { id: "carrier-next-jump", question: "What's the next carrier jump?", expect_tools: &["carrier_route"], forbid_tools: &["plot_route", "current_route"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 400 },
    // Item 53: stored ships are located from the journal; the ladder never guesses.
    Case { id: "ship-where", question: "Where is my Anaconda?", expect_tools: &["list_ships"], forbid_tools: &["find_station", "plot_route", "nearest_service"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 500 },
    Case { id: "ship-navigate", question: "Navigate to my Anaconda please.", expect_tools: &["list_ships"], forbid_tools: &["find_station"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 600 },
    // Item 52 A: the carrier is answered from the journal, with its age.
    Case { id: "carrier-tritium", question: "How much tritium is on my carrier?", expect_tools: &["get_carrier_status"], forbid_tools: &["get_inventory", "get_ship_status"], must_contain: &["tritium"], must_contain_from: None, expect_effect: None, max_chars: 500 },
    Case { id: "carrier-where", question: "Where is my carrier?", expect_tools: &["get_carrier_status"], forbid_tools: &["get_ship_status", "find_station"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 400 },
    Case { id: "carrier-plot", question: "Plot a route to my carrier.", expect_tools: &["get_carrier_status", "plot_route"], forbid_tools: &["find_station"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 600 },
    Case { id: "carrier-tritium-source", question: "Where can I buy tritium for my carrier?", expect_tools: &["get_carrier_status", "market_search"], forbid_tools: &["find_profit"], must_contain: &["tritium"], must_contain_from: None, expect_effect: None, max_chars: 800 },
    // Honest gaps (maintainer, 2026-09-06): a board EDDA never observed is
    // "no local data", never "sells nothing"; the community API answers
    // for it by default, and the reply says so.
    Case { id: "board-gap", question: "What does station 999999999 sell?", expect_tools: &["station_market"], forbid_tools: &["market_search", "find_profit"], must_contain: &["no local data", "no data", "community"], must_contain_from: None, expect_effect: None, max_chars: 600 },
    Case { id: "plot", question: "Plot a route to Sol.", expect_tools: &["plot_route"], forbid_tools: &[], must_contain: &["sol"], must_contain_from: None, expect_effect: None, max_chars: 600 },
    Case { id: "missions", question: "What missions do I have on?", expect_tools: &["missions"], forbid_tools: &[], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 900 },
    Case { id: "chat", question: "Who is the best commander in the game?", expect_tools: &[], forbid_tools: &["plot_route", "find_profit", "get_inventory", "current_route", "engineer_unlocks", "list_engineers"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 600 },
    Case { id: "pips", question: "Put four pips into systems and the rest into engines.", expect_tools: &["set_pips"], forbid_tools: &["game_control"], must_contain: &[], must_contain_from: None, expect_effect: Some("press pips_reset"), max_chars: 300 },
    Case { id: "pips-full", question: "Full pips to weapons!", expect_tools: &["set_pips"], forbid_tools: &["game_control"], must_contain: &[], must_contain_from: None, expect_effect: Some("press pips_weapons x2"), max_chars: 200 },
    Case { id: "gear", question: "Lower the landing gear.", expect_tools: &["game_control"], forbid_tools: &[], must_contain: &[], must_contain_from: None, expect_effect: Some("press"), max_chars: 200 },
    Case { id: "trader", question: "Where's the nearest manufactured material trader?", expect_tools: &["nearest_service"], forbid_tools: &["material_sources", "find_profit"], must_contain: &["light"], must_contain_from: None, expect_effect: None, max_chars: 500 },
    Case { id: "kills", question: "How many kills have I made this week?", expect_tools: &["combat_stats"], forbid_tools: &["commander_ranks"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 500 },
    Case { id: "merits", question: "Who am I pledged to in Powerplay, and how many merits do I have?", expect_tools: &["commander_ranks"], forbid_tools: &["get_merit_model", "powerplay_seen"], must_contain: &["merit"], must_contain_from: Some(("commander_ranks", "/ranks/powerplay/power")), expect_effect: None, max_chars: 400 },
    Case { id: "target-none", question: "Target the next system.", expect_tools: &["follow_route", "game_control", "current_route"], forbid_tools: &["plot_route"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 300 },
    Case { id: "clear", question: "Clear the route.", expect_tools: &["follow_route"], forbid_tools: &["plot_route"], must_contain: &[], must_contain_from: None, expect_effect: None, max_chars: 300 },
    Case { id: "say", question: "Say 'all hands, prepare for departure' over the speakers.", expect_tools: &["say"], forbid_tools: &[], must_contain: &[], must_contain_from: None, expect_effect: Some("say"), max_chars: 300 },
    Case { id: "scoop", question: "Open the cargo scoop and turn the lights on.", expect_tools: &["game_control"], forbid_tools: &["set_pips"], must_contain: &[], must_contain_from: None, expect_effect: Some("press lights"), max_chars: 300 },
    Case { id: "station-sys", question: "What stations are there in Wongi?", expect_tools: &["stations_in_system", "find_system"], forbid_tools: &["find_profit"], must_contain: &["schmitt"], must_contain_from: None, expect_effect: None, max_chars: 700 },
    Case { id: "factors", question: "Where's the nearest interstellar factors so I can pay off my bounties?", expect_tools: &["nearest_service"], forbid_tools: &["find_profit", "plot_route"], must_contain: &["light"], must_contain_from: None, expect_effect: None, max_chars: 500 },
    Case { id: "dist", question: "How far is Sol from here?", expect_tools: &["find_system"], forbid_tools: &["plot_route"], must_contain: &["light"], must_contain_from: None, expect_effect: None, max_chars: 300 },
];

#[derive(Debug, Serialize, Clone)]
pub struct CaseResult {
    pub id: &'static str,
    pub question: &'static str,
    pub pass: bool,
    pub failures: Vec<String>,
    pub tools_used: Vec<String>,
    pub effects: Vec<String>,
    pub answer: String,
    pub ms: u128,
}

#[derive(Debug, Serialize, Clone)]
pub struct Report {
    pub provider: String,
    pub model: String,
    pub passed: usize,
    pub total: usize,
    pub results: Vec<CaseResult>,
}

#[derive(Debug, Deserialize, Default)]
pub struct RunRequest {
    #[serde(default)]
    pub only: Vec<String>,
    /// Dev hook only: run this one tool with `input` and write `tool.json`
    /// instead of the suite -- for looking at result shapes.
    /// Run against this provider ("anthropic" | "openai") instead of the
    /// saved one; the saved setting is untouched.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub input: Option<Value>,
}

fn provider_label(state: &AppState) -> (String, String) {
    let cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    if cfg.ai_provider.as_deref() == Some("openai") {
        (
            cfg.openai_base_url.clone().unwrap_or_default(),
            cfg.openai_model.clone().unwrap_or_default(),
        )
    } else {
        (
            "anthropic".into(),
            cfg.anthropic_model
                .clone()
                .unwrap_or_else(|| "default".into()),
        )
    }
}

fn markdown_smell(text: &str) -> bool {
    text.contains("**")
        || text.lines().any(|l| {
            l.trim_start().starts_with("# ")
                || l.trim_start().starts_with("- ")
                || l.trim_start().starts_with("* ")
        })
}

pub async fn run(state: &AppState, req: &RunRequest) -> Report {
    // Provider override for the run only (in memory, never saved).
    let saved = req.provider.as_ref().map(|p| {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.ai_provider.replace(p.clone())
    });
    let report = run_inner(state, req).await;
    if let Some(prev) = saved {
        state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ai_provider = prev;
    }
    report
}

async fn run_inner(state: &AppState, req: &RunRequest) -> Report {
    let (provider, model) = provider_label(state);
    let fx = crate::ai::Recording::default();
    let mut results = Vec::new();
    for case in CASES {
        if !req.only.is_empty() && !req.only.iter().any(|o| o == case.id) {
            continue;
        }
        // Every case starts a fresh conversation.
        state.chat.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let started = std::time::Instant::now();
        let mut failures = Vec::new();
        let (answer, tools_used) = match crate::ai::ask_with(state, &fx, case.question).await {
            Ok(a) => (a.text, a.tools_used),
            Err(e) => {
                failures.push(format!("error: {e:#}"));
                (String::new(), Vec::new())
            }
        };
        let effects = fx.take();
        let lower = answer.to_lowercase();
        if let Some(e) = case.expect_effect {
            if !effects.iter().any(|x| x.contains(e)) {
                failures.push(format!("expected a {e:?} effect, got {effects:?}"));
            }
        }
        if !case.expect_tools.is_empty()
            && !case
                .expect_tools
                .iter()
                .any(|t| tools_used.iter().any(|u| u == t))
        {
            failures.push(format!(
                "expected one of {:?} to be called",
                case.expect_tools
            ));
        }
        for t in case.forbid_tools {
            if tools_used.iter().any(|u| u == t) {
                failures.push(format!("{t} must not be called"));
            }
        }
        if !case.must_contain.is_empty()
            && !case
                .must_contain
                .iter()
                .any(|s| lower.contains(&s.to_lowercase()))
        {
            failures.push(format!(
                "answer should mention one of {:?}",
                case.must_contain
            ));
        }
        if let Some((tool, pointer)) = case.must_contain_from {
            let live = crate::ai::tool_output(state, &fx, tool, &json!({}));
            fx.take();
            match live.pointer(pointer).and_then(Value::as_str) {
                Some(v) if !v.is_empty() => {
                    if !lower.contains(&v.to_lowercase()) {
                        failures.push(format!(
                            "answer should contain {v:?} (from {tool}{pointer})"
                        ));
                    }
                }
                _ => failures.push(format!("(no live value at {tool}{pointer}; check skipped)")),
            }
        }
        if answer.is_empty() && failures.is_empty() {
            failures.push("empty answer".into());
        }
        if answer.chars().count() > case.max_chars {
            failures.push(format!(
                "too long for speech: {} chars > {}",
                answer.chars().count(),
                case.max_chars
            ));
        }
        if markdown_smell(&answer) {
            failures.push("markdown in a spoken reply".into());
        }
        let pass = failures.iter().all(|f| f.starts_with('('));
        tracing::info!(case = case.id, pass, tools = ?tools_used, ms = started.elapsed().as_millis() as u64, "eval case");
        results.push(CaseResult {
            id: case.id,
            question: case.question,
            pass,
            failures,
            tools_used,
            effects,
            answer,
            ms: started.elapsed().as_millis(),
        });
    }
    state.chat.lock().unwrap_or_else(|e| e.into_inner()).clear();
    let passed = results.iter().filter(|r| r.pass).count();
    Report {
        provider,
        model,
        passed,
        total: results.len(),
        results,
    }
}

#[tauri::command]
pub async fn ai_eval(
    state: tauri::State<'_, AppState>,
    only: Option<Vec<String>>,
    provider: Option<String>,
) -> Result<Report, String> {
    let req = RunRequest {
        only: only.unwrap_or_default(),
        provider,
        tool: None,
        input: None,
    };
    let report = run(&state, &req).await;
    let dir = state.data_dir.join("eval");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(
        dir.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    );
    Ok(report)
}

/// Debug builds: poll `.data/eval/run.json` so the suite can be driven from
/// a shell while the app keeps running.
/// Debug builds only: polls `.data/eval/run.json` and runs what it asks.
/// A supervised job; `token` ends the poll on exit.
pub async fn dev_hook(token: tokio_util::sync::CancellationToken, app: AppHandle) {
    if !cfg!(debug_assertions) {
        return;
    }
    {
        let state = app.state::<AppState>();
        let dir = state.data_dir.join("eval");
        let run_file = dir.join("run.json");
        while crate::jobs::sleep_unless_cancelled(&token, std::time::Duration::from_secs(3)).await {
            let Ok(text) = std::fs::read_to_string(&run_file) else {
                continue;
            };
            let _ = std::fs::remove_file(&run_file);
            let req: RunRequest = serde_json::from_str(&text).unwrap_or_default();
            if let Some(tool) = req.tool.as_deref() {
                let fx = crate::ai::Recording::default();
                let out = crate::ai::tool_output(
                    &state,
                    &fx,
                    tool,
                    &req.input.clone().unwrap_or(json!({})),
                );
                let _ = std::fs::write(
                    dir.join("tool.json"),
                    serde_json::to_string_pretty(
                        &json!({ "tool": tool, "output": out, "effects": fx.take() }),
                    )
                    .unwrap_or_default(),
                );
                continue;
            }
            tracing::info!(only = ?req.only, "eval run requested");
            let report = run(&state, &req).await;
            let _ = std::fs::write(
                dir.join("report.json"),
                serde_json::to_string_pretty(&report).unwrap_or_default(),
            );
            tracing::info!(passed = report.passed, total = report.total, "eval done");
        }
    }
}
