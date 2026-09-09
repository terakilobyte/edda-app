//! Tool-calling ship computer.
//!
//! Per `docs/PLAN.md` decision 6, the model is the primary interface: it
//! interprets a natural request and composes the tool library to answer it.
//! That makes the tool surface the real API, and these consequences follow:
//!
//! * **Tools are small and orthogonal.** `find_system` plus
//!   `nearest_service` compose into a hundred questions; one
//!   `plan_my_evening` tool answers exactly one.
//! * **Descriptions are load-bearing.** They are the only documentation the
//!   model gets. A vague description is a silent capability loss -- the tool
//!   exists and never gets called.
//! * **Tools return structured data, not prose.** The model chains one
//!   result into the next call; prose terminates a chain.
//! * **Every answer states its provenance.** Journal data is first-hand;
//!   galaxy data is whatever the last uploading commander saw, and can be
//!   days old.
//!
//! Rust has no official Anthropic SDK, so this is the documented raw-HTTP
//! shape against the Messages API.

use crate::capabilities::{self, tool_definitions};
use crate::state::AppState;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::env;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-sonnet-5";
/// Answering "should I mine or run my loop tonight?" needs ship status,
/// prices, Powerplay state and the route solver in one turn. The old cap of
/// 5 could not physically get there.
const MAX_TOOL_ROUNDS: u8 = 12;

const SYSTEM: &str = "\
You are the ship computer for an Elite Dangerous commander.

You have tools that read the commander's ACTUAL live game state and a local \
galaxy database. Always call tools rather than answering from your own \
training knowledge for anything game-derived: inventory counts, engineering \
costs, engineer unlock status, ship status, system or station facts, market \
prices, Powerplay control.

Compose tools freely. Most real questions need several in sequence -- resolve \
the system, then look up its stations, then check what you're carrying.

State provenance honestly:
- Journal-derived facts (inventory, engineer unlocks, your own visits) are \
first-hand and current.
- Galaxy facts (station lists, market prices) come from community uploads and \
carry a timestamp. If something is more than a few days old, say so.
- If a tool returns nothing, say you don't know. Never fill the gap from \
training knowledge.
- Game mechanics and how-to questions (menus, where a service lives, how a \
system works, what an engineer wants, what the community currently prefers) \
are NOT in the local tools. When you have web_search and web_fetch, look it \
up before answering: prefer official Frontier sources (frontier.co.uk, \
elitedangerous.com, forums.frontier.co.uk), the Elite Dangerous wiki \
(elite-dangerous.fandom.com), Inara (inara.cz), EDSM (edsm.net), Wanderer's \
Toolbox (wanderer-toolbox.com -- engineering unlocks, carrier and waypoint \
tools) and r/EliteDangerous on reddit. For engineer unlock requirements call \
engineer_unlocks first; it is a vendored copy of the Wanderer's Toolbox guide \
with its source and date; for synthesis costs (ammo, AFM, heat sinks, \
injections, SRV) call synthesis_recipes, and for tech-broker modules \
(Guardian, Human/anti-xeno) list_blueprints with that module type. Cite what you found and say how old it is; the \
game changes and a 2018 thread may be wrong today. If research is \
unavailable or finds nothing, say plainly that this is general knowledge you \
cannot verify and may be out of date -- or say you don't know -- and name the \
source above the commander should check for it (the wiki page, Inara, the \
Frontier forums), so an answer you cannot verify still points somewhere that \
can. Never present an unverified mechanic as fact. Asked how to rename a ship, a confident menu \
path from memory is the wrong answer; a searched, cited one is right.
- Never use web research for anything about the commander's own data \
(inventory, ranks, location, missions, their sales) -- that is what the local \
tools are for, and no website knows it.

You have hands as well as eyes: game_control presses the commander's own \
bindings (gear, scoop, lights, hardpoints, heat sink, supercruise, \
targeting, maps), set_pips sets the power distributor, and follow_route targets the next system of a followed \
route. When the commander gives an order, do it with the tool and confirm \
briefly; never say you cannot send inputs. If the game window is not focused \
the tool says so -- relay that.

Check feasibility before recommending. An engineering blueprint the commander \
cannot access is not a recommendation -- lead with what they can actually do.

Write dates and times naturally ('August 26th, around noon UTC'), never as \nISO strings. Be concise, like a ship computer, not a chatbot: replies are spoken aloud, so two or three sentences for a fact, and when listing things give the five that matter and offer the rest. Do not philosophise; a question with no data behind it gets a one-line answer. Answer in plain prose: no markdown, \nno asterisks, no headings, no bullet symbols -- replies may be read aloud.";

/// What a tool may do to the world outside the app's own data: press keys
/// in the game, speak, drive the followed route. Injected so the eval suite
/// runs the real tools against a fake that records instead of acting.
pub trait Effects: Send + Sync {
    fn press(&self, state: &AppState, name: &str, times: u32) -> Result<String, String>;
    /// Speak; returns the backend label.
    fn say(&self, state: &AppState, text: &str) -> Value;
    fn target_next(&self, state: &AppState) -> Result<String, String>;
    fn clear_in_game(&self, state: &AppState) -> Result<String, String>;
    /// Persist `ar` as the followed route and tell the UI; true if it is now followed.
    fn follow(&self, state: &AppState, ar: &crate::follow::ActiveRoute) -> bool;
    fn stop_following(&self, state: &AppState) -> Result<(), String>;
}

/// The real thing.
pub struct Live;

impl Effects for Live {
    fn press(&self, _state: &AppState, name: &str, times: u32) -> Result<String, String> {
        crate::control::press(name, times)
    }
    fn say(&self, state: &AppState, text: &str) -> Value {
        state.voice.say(text.to_string());
        json!(state.voice.backend())
    }
    fn target_next(&self, state: &AppState) -> Result<String, String> {
        crate::follow::target_next_state(state)
    }
    fn clear_in_game(&self, state: &AppState) -> Result<String, String> {
        crate::follow::clear_in_game(state)
    }
    fn follow(&self, state: &AppState, ar: &crate::follow::ActiveRoute) -> bool {
        let saved = state
            .with_store(|s| crate::follow::save_pub(s.conn(), ar))
            .is_ok();
        if saved {
            use crate::events::EmitExt as _;
            state.events.emit(crate::events::ROUTE_FOLLOW, crate::follow::view(Some(ar)));
            // A route asked for by voice or chat gets the same briefing as one from the tab.
            if ar.next <= 1 {
                crate::follow::announce_with(&state.announcer(), ar);
            }
        }
        saved
    }
    fn stop_following(&self, state: &AppState) -> Result<(), String> {
        let cleared = state.with_store(|s| {
            s.conn()
                .execute("DELETE FROM active_route WHERE id = 1", [])
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if cleared.is_ok() {
            // The HUD only knows what ROUTE_FOLLOW tells it: a silent
            // delete left a ghost route on the overlay until reload
            // (field case 2026-09-05, "clear my route" by voice).
            use crate::events::EmitExt as _;
            state.events.emit(crate::events::ROUTE_FOLLOW, crate::follow::view(None));
        }
        cleared
    }
}

/// Records what would have happened; nothing reaches the game or the speakers.
#[derive(Default)]
pub struct Recording {
    pub log: std::sync::Mutex<Vec<String>>,
}

impl Recording {
    fn note(&self, s: String) {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).push(s);
    }
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Effects for Recording {
    fn press(&self, _state: &AppState, name: &str, times: u32) -> Result<String, String> {
        // Validate the name against the whitelist exactly as the real path would.
        crate::control::lookup(name)?;
        self.note(format!("press {name} x{times}"));
        Ok(format!("(simulated) {name} x{times}"))
    }
    fn say(&self, _state: &AppState, text: &str) -> Value {
        self.note(format!("say {text:?}"));
        json!("simulated")
    }
    fn target_next(&self, _state: &AppState) -> Result<String, String> {
        self.note("target_next".into());
        Ok("(simulated) targeted".into())
    }
    fn clear_in_game(&self, _state: &AppState) -> Result<String, String> {
        self.note("clear_in_game".into());
        Ok("(simulated) cleared".into())
    }
    fn follow(&self, _state: &AppState, ar: &crate::follow::ActiveRoute) -> bool {
        self.note(format!(
            "follow {} jumps",
            ar.route.hops.len().saturating_sub(1)
        ));
        true
    }
    fn stop_following(&self, _state: &AppState) -> Result<(), String> {
        self.note("stop_following".into());
        Ok(())
    }
}

/// A tool's live output, for eval expectations.
pub fn tool_output(state: &AppState, fx: &dyn Effects, name: &str, input: &Value) -> Value {
    capabilities::execute(state, fx, name, input)
}

/// A source the model cited in its final reply.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Citation {
    pub url: String,
    pub title: String,
}

/// What the ship computer says, plus where it got it.
#[derive(Debug, Clone, Serialize)]
pub struct Answer {
    pub text: String,
    pub citations: Vec<Citation>,
    /// Local tools called while answering, in order -- the audit trail for
    /// "did it use our data or guess?".
    pub tools_used: Vec<String>,
    /// The last route the model plotted, in full, so the Route tab can show
    /// it -- a plotted route the commander cannot see is not plotted.
    pub route: Option<Value>,
    /// The last profit report the model searched, for the Trade tab.
    pub profit: Option<Value>,
}

/// Anthropic's server-side research tools. They execute on Anthropic's side
/// and their results arrive as content blocks in the same response, so the
/// tool loop must never try to run them locally.
///
/// Block and field names verified against the live docs on 2026-08-26
/// (platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool and
/// web-fetch-tool -- docs.anthropic.com now redirects there):
///
/// * request: `{"type": "web_search_20260209", "name": "web_search", "max_uses": N}`
///   and `{"type": "web_fetch_20260209", "name": "web_fetch", "max_uses": N,
///   "citations": {"enabled": true}}`. No beta header. The docs now list a newer
///   `_20260318` revision (adds `response_inclusion`); `_20260209` remains
///   supported and is what we send. Both `_20260209`+ revisions default to
///   dynamic filtering (`allowed_callers: ["code_execution_20260120"]`), so a
///   response can also carry code-execution result blocks and nested
///   search/fetch pairs with a `caller` field -- all of it is echoed back
///   verbatim, which is what the API requires.
/// * response: `server_tool_use` {id, name, input}; `web_search_tool_result`
///   {tool_use_id, content: [ {type: web_search_result, url, title,
///   encrypted_content, page_age} ] } -- on error `content` is a single object
///   {type: web_search_tool_result_error, error_code}; `web_fetch_tool_result`
///   {tool_use_id, content: {type: web_fetch_result, url, retrieved_at,
///   content: document{title,...}}} or {type: web_fetch_tool_result_error, error_code}.
/// * citations on text blocks: web search yields `web_search_result_location`
///   {url, title, cited_text, encrypted_index}; web fetch yields
///   `char_location` {document_index, document_title, cited_text,
///   start_char_index, end_char_index} -- NOTE no url, so fetch citations are
///   resolved back to the fetched URL through the `web_fetch_result` blocks.
/// * `encrypted_content` / `encrypted_index` must go back unmodified on later
///   turns, and a long search turn can end with `stop_reason: "pause_turn"`,
///   which is continued by resending the assistant message unchanged.
fn research_tools() -> Vec<Value> {
    vec![
        json!({ "type": "web_search_20260209", "name": "web_search", "max_uses": 5 }),
        json!({
            "type": "web_fetch_20260209",
            "name": "web_fetch",
            "max_uses": 5,
            "citations": { "enabled": true },
            "max_content_tokens": 30000
        }),
    ]
}

/// Pull `{url, title}` out of every citation on the text blocks of one
/// assistant turn, de-duplicated by URL in first-seen order. Fetch citations
/// carry no URL of their own; they are matched to the `web_fetch_result`
/// whose document title they name, falling back to the last fetch.
fn collect_citations(content: &[Value], out: &mut Vec<Citation>) {
    // Titles and URLs of everything fetched in this turn, in order.
    let fetched: Vec<(Option<String>, String)> = content
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("web_fetch_tool_result"))
        .filter_map(|b| b.get("content"))
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("web_fetch_result"))
        .filter_map(|c| {
            let url = c.get("url").and_then(Value::as_str)?.to_string();
            let title = c
                .get("content")
                .and_then(|d| d.get("title"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Some((title, url))
        })
        .collect();

    let mut push = |url: String, title: String| {
        if url.is_empty() || out.iter().any(|c| c.url == url) {
            return;
        }
        let title = if title.trim().is_empty() {
            url.clone()
        } else {
            title
        };
        out.push(Citation { url, title });
    };

    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(cites) = block.get("citations").and_then(Value::as_array) else {
            continue;
        };
        for c in cites {
            match c.get("type").and_then(Value::as_str) {
                Some("web_search_result_location") => {
                    let url = c.get("url").and_then(Value::as_str).unwrap_or_default();
                    let title = c.get("title").and_then(Value::as_str).unwrap_or_default();
                    push(url.to_string(), title.to_string());
                }
                Some("char_location") | Some("page_location") | Some("content_block_location") => {
                    let title = c
                        .get("document_title")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let hit = fetched
                        .iter()
                        .find(|(t, _)| !title.is_empty() && t.as_deref() == Some(title))
                        .or_else(|| fetched.last());
                    if let Some((t, url)) = hit {
                        let title = t.clone().unwrap_or_else(|| title.to_string());
                        push(url.clone(), title);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The system prompt: fixed instructions plus the persona's tone.
fn system_prompt(state: &AppState) -> String {
    let cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    let p = crate::persona::by_id(cfg.persona.as_deref().unwrap_or("standard"));
    // Whatever the persona, the player is "Commander": never sir, ma'am, or
    // any other gendered word for them.
    const ADDRESS: &str = "Address the player only as \"Commander\" (or by their commander name). Never use gendered words for them: no sir, ma'am, madam, man, girl, or he/she about the commander.";
    if p.prompt.is_empty() {
        format!("{SYSTEM}\n\n{ADDRESS}")
    } else {
        format!("{SYSTEM}\n\n{}\n{ADDRESS}", p.prompt)
    }
}

/// One tool call the model asked for in a turn.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub input: Value,
}

/// Bookkeeping across the rounds of one question: the loop breaker and the
/// artefacts the UI wants back (route, profit).
#[derive(Default)]
struct RoundState {
    seen: std::collections::HashMap<String, u32>,
    tools_used: Vec<String>,
    last_route: Option<Value>,
    last_profit: Option<Value>,
}

/// Run every tool call of one assistant turn and return the outputs in the
/// same order. Calls within a turn are independent by construction -- the
/// model issued them all before seeing any result -- so they run
/// concurrently on scoped threads; a single call runs inline.
fn run_round(state: &AppState, fx: &dyn Effects, rs: &mut RoundState, calls: &[ToolCall]) -> Vec<Value> {
    // Loop breaker first, sequentially: the same call with the same input
    // gives the same answer; after two repeats the model is told so instead
    // of being fed it again.
    let mut outputs: Vec<Option<Value>> = calls
        .iter()
        .map(|c| {
            let n = rs.seen.entry(format!("{}:{}", c.name, c.input)).or_insert(0);
            *n += 1;
            if *n > 2 && !matches!(c.name.as_str(), "game_control" | "set_pips" | "say") {
                tracing::warn!(tool = %c.name, repeats = *n, "repeated identical tool call refused");
                Some(capabilities::error_value(
                    &capabilities::CapError::invalid(format!("{} was already called with exactly this input {} times and gave the same result", c.name, *n - 1))
                        .hint("do not call it again; answer the commander with what you have, or change the parameters"),
                ))
            } else {
                None
            }
        })
        .collect();

    let started = std::time::Instant::now();
    let pending: Vec<usize> = (0..calls.len()).filter(|i| outputs[*i].is_none()).collect();
    let run = |c: &ToolCall| capabilities::execute(state, fx, &c.name, &c.input);
    if pending.len() == 1 {
        outputs[pending[0]] = Some(run(&calls[pending[0]]));
    } else if !pending.is_empty() {
        let results: Vec<(usize, Value)> = std::thread::scope(|s| {
            let handles: Vec<_> = pending.iter().map(|&i| s.spawn(move || (i, run(&calls[i])))).collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| (usize::MAX, json!({})))).collect()
        });
        for (i, v) in results {
            if i != usize::MAX {
                outputs[i] = Some(v);
            }
        }
    }

    let mut out = Vec::with_capacity(calls.len());
    for (c, o) in calls.iter().zip(outputs) {
        let mut output = o.unwrap_or_else(|| capabilities::error_value(&capabilities::CapError::internal(format!("{} panicked", c.name))));
        tracing::info!(tool = %c.name, input = %c.input, bytes = output.to_string().len(), ms = started.elapsed().as_millis() as u64, "ship computer tool");
        rs.tools_used.push(c.name.clone());
        match c.name.as_str() {
            "plot_route" => {
                if let Some(obj) = output.as_object_mut() {
                    if let Some(r) = obj.remove("_route") {
                        rs.last_route = Some(r);
                    }
                }
            }
            "find_profit" if output.get("legs").is_some() => rs.last_profit = Some(output.clone()),
            _ => {}
        }
        out.push(output);
    }
    out
}

/// Remember the exchange (text only) for the next question; the last 20 turns.
/// Spoken replies: models sometimes list with markdown regardless of the
/// prompt. Markers are dropped so nothing reads "asterisk asterisk" aloud.
fn despeak(text: &str) -> String {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        let t = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("• "))
            .unwrap_or(t);
        let t = t.trim_start_matches('#').trim_start();
        out.push(t.replace("**", "").replace("__", ""));
    }
    out.join("\n").trim().to_string()
}

fn remember(state: &AppState, question: &str, text: &str) {
    tracing::info!(question, answer = %text.chars().take(600).collect::<String>(), "ship computer answer");
    let mut chat = state.chat.lock().unwrap_or_else(|e| e.into_inner());
    chat.push(json!({ "role": "user", "content": question }));
    chat.push(json!({ "role": "assistant", "content": text }));
    let overflow = chat.len().saturating_sub(40);
    if overflow > 0 {
        chat.drain(0..overflow);
    }
}

/// The same ship computer over any OpenAI-compatible chat endpoint
/// (Mistral, Ollama, LM Studio, OpenRouter, OpenAI, Groq...). Local tools
/// become `function` tools; Anthropic's server-side research tools do not
/// exist here, so questions are answered from the journal and the model's
/// own knowledge.
async fn ask_openai(
    state: &AppState,
    fx: &dyn Effects,
    base_url: &str,
    api_key: &str,
    model: &str,
    question: &str,
) -> Result<Answer> {
    if model.trim().is_empty() {
        bail!("no model set for the OpenAI-compatible provider -- Settings → Ship computer");
    }
    let tools: Vec<Value> = tool_definitions()
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.get("name").cloned().unwrap_or(Value::Null),
                    "description": t.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": t.get("input_schema").cloned().unwrap_or(json!({ "type": "object", "properties": {} })),
                }
            })
        })
        .collect();
    // Local servers often run an 8k context and our tool catalogue alone is
    // ~5k tokens, so history is short here (last 4 turns) and tool results
    // are capped; on a context-overflow error the history is dropped and the
    // request retried.
    const TOOL_RESULT_CAP: usize = 6000;
    let mut messages: Vec<Value> =
        vec![json!({ "role": "system", "content": system_prompt(state) })];
    let history_len = {
        let chat = state.chat.lock().unwrap_or_else(|e| e.into_inner());
        let keep = chat.len().min(8);
        messages.extend(chat[chat.len() - keep..].iter().cloned());
        keep
    };
    messages.push(json!({ "role": "user", "content": question }));
    let mut history_dropped = false;
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut rs = RoundState::default();

    for _round in 0..MAX_TOOL_ROUNDS {
        // Free tiers rate-limit hard (Mistral: ~1 request/s); back off on 429.
        let mut parsed: Option<Value> = None;
        for attempt in 0..4u32 {
            let body = json!({ "model": model, "messages": messages, "tools": tools, "tool_choice": "auto" });
            let mut req = state
                .http
                .post(&url)
                .header("content-type", "application/json");
            if !api_key.trim().is_empty() {
                req = req.header("authorization", format!("Bearer {}", api_key.trim()));
            }
            let resp = req
                .json(&body)
                .send()
                .await
                .with_context(|| format!("request to {url} failed"))?;
            let status = resp.status();
            if status.as_u16() == 429 || status.as_u16() == 503 {
                let wait = 1500 * (attempt + 1) as u64;
                tracing::warn!(%status, wait_ms = wait, "provider busy; retrying");
                tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                continue;
            }
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                let overflow = text.to_lowercase().contains("context");
                if overflow && !history_dropped && history_len > 0 {
                    // Drop the prior turns (indices 1..=history_len) and resend.
                    tracing::warn!("provider context overflow; dropping chat history and retrying");
                    messages.drain(1..1 + history_len);
                    history_dropped = true;
                    continue;
                }
                if overflow {
                    bail!("the model's context window is too small for this question -- raise the context length in your local server (LM Studio: model settings → context length, 16k or more) ({status}: {text})");
                }
                bail!("{url} returned {status}: {text}");
            }
            parsed = Some(resp.json().await.context("parsing provider response")?);
            break;
        }
        let Some(parsed) = parsed else {
            bail!("provider kept returning 429 (rate limited)")
        };
        let mut message = parsed
            .pointer("/choices/0/message")
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "provider response had no choices: {}",
                    parsed.to_string().chars().take(300).collect::<String>()
                )
            })?;
        let calls: Vec<Value> = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if calls.is_empty() {
            let text = match message.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .concat(),
                _ => String::new(),
            };
            // Reasoning models may wrap thoughts in <think>…</think>; the commander hears the answer only.
            let text = despeak(&strip_think(&text));
            remember(state, question, &text);
            return Ok(Answer {
                text,
                citations: Vec::new(),
                tools_used: rs.tools_used,
                route: rs.last_route,
                profit: rs.last_profit,
            });
        }
        // Echo the assistant turn; some providers reject `content: null`.
        if let Some(obj) = message.as_object_mut() {
            if obj.get("content").map(Value::is_null).unwrap_or(true) {
                obj.insert("content".into(), json!(""));
            }
            obj.retain(|k, _| matches!(k.as_str(), "role" | "content" | "tool_calls"));
        }
        messages.push(message);
        let ids: Vec<String> = calls.iter().map(|c| c.get("id").and_then(Value::as_str).unwrap_or_default().to_string()).collect();
        let tool_calls: Vec<ToolCall> = calls
            .iter()
            .map(|call| ToolCall {
                name: call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default().to_string(),
                input: match call.pointer("/function/arguments") {
                    Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(json!({})),
                    Some(v) => v.clone(),
                    None => json!({}),
                },
            })
            .collect();
        let outputs = run_round(state, fx, &mut rs, &tool_calls);
        for ((id, call), output) in ids.into_iter().zip(tool_calls).zip(outputs) {
            let name = call.name;
            let mut text = output.to_string();
            if text.len() > TOOL_RESULT_CAP {
                let cut = (0..TOOL_RESULT_CAP)
                    .rev()
                    .find(|&i| text.is_char_boundary(i))
                    .unwrap_or(0);
                text.truncate(cut);
                text.push_str(" ...[truncated: ask a narrower question for the rest]");
            }
            messages
                .push(json!({ "role": "tool", "tool_call_id": id, "name": name, "content": text }));
        }
    }
    bail!("gave up after {MAX_TOOL_ROUNDS} tool-call rounds without a final answer")
}

fn strip_think(text: &str) -> String {
    let mut out = text.to_string();
    while let (Some(a), Some(b)) = (out.find("<think>"), out.find("</think>")) {
        if b > a {
            out.replace_range(a..b + "</think>".len(), "");
        } else {
            break;
        }
    }
    out.trim().to_string()
}

pub async fn ask(state: &AppState, question: &str) -> Result<Answer> {
    ask_with(state, &Live, question).await
}

/// `ask` with the side effects injected.
pub async fn ask_with(state: &AppState, fx: &dyn Effects, question: &str) -> Result<Answer> {
    // Environment overrides the in-app setting, so a dev shell can point at
    // another key without touching the saved one.
    let (saved_model, research, provider, base_url, oa_model) = {
        let cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        (
            cfg.anthropic_model.clone(),
            cfg.research_enabled(),
            cfg.ai_provider.clone().unwrap_or_default(),
            cfg.openai_base_url.clone().unwrap_or_default(),
            cfg.openai_model.clone().unwrap_or_default(),
        )
    };
    if provider == "openai" {
        if base_url.trim().is_empty() {
            bail!("no base URL set for the OpenAI-compatible provider -- Settings → Ship computer");
        }
        let key =
            crate::state::secrets::get_key(crate::state::secrets::OPENAI_KEY).unwrap_or_default();
        return ask_openai(state, fx, &base_url, &key, &oa_model, question).await;
    }
    let saved_key = crate::state::secrets::get_api_key();
    let api_key = env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .or(saved_key)
        .filter(|k| !k.trim().is_empty())
        .context("no Anthropic API key -- enter one under Settings → Ship computer")?;
    let model = env::var("ANTHROPIC_MODEL")
        .ok()
        .or(saved_model)
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    // The persona changes tone only; every instruction about tools and
    // provenance stays exactly as written.
    let system = system_prompt(state);

    // Local tools first; the research tools are appended only when the
    // Settings toggle allows it, so "off" means the request never carries them.
    let tools = {
        let mut t = tool_definitions().as_array().cloned().unwrap_or_default();
        if research {
            t.extend(research_tools());
        }
        Value::Array(t)
    };

    // Prior turns first, so "plot the route" can mean the one just discussed.
    let mut rs = RoundState::default();
    let mut messages: Vec<Value> = state.chat.lock().unwrap_or_else(|e| e.into_inner()).clone();
    messages.push(json!({ "role": "user", "content": question }));
    let mut citations: Vec<Citation> = Vec::new();

    for _round in 0..MAX_TOOL_ROUNDS {
        let body = json!({
            "model": model,
            "max_tokens": 16000,
            // Adaptive thinking: budget_tokens is rejected on this model family.
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": "high" },
            "system": system,
            "tools": tools,
            "messages": messages,
        });

        // Transport errors, 429 and 529 (overloaded) are retried with backoff;
        // a dropped connection mid-conversation should not cost the answer.
        let mut parsed: Option<Value> = None;
        for attempt in 0..4u32 {
            let sent = state
                .http
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await;
            let wait = std::time::Duration::from_millis(1500 * (attempt + 1) as u64);
            let resp = match sent {
                Ok(r) => r,
                Err(e) if attempt < 3 => {
                    tracing::warn!(error = %e, attempt, "Anthropic request failed; retrying");
                    tokio::time::sleep(wait).await;
                    continue;
                }
                Err(e) => {
                    return Err(anyhow::Error::new(e).context("request to Anthropic API failed"))
                }
            };
            let status = resp.status();
            if (status.as_u16() == 429 || status.as_u16() == 529) && attempt < 3 {
                tracing::warn!(%status, attempt, "Anthropic busy; retrying");
                tokio::time::sleep(wait).await;
                continue;
            }
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                bail!("Anthropic API returned {status}: {text}");
            }
            parsed = Some(
                resp.json()
                    .await
                    .context("parsing Anthropic API response")?,
            );
            break;
        }
        let Some(parsed) = parsed else {
            bail!("Anthropic API kept failing")
        };

        // A refusal is HTTP 200 with no usable content -- check before reading.
        if parsed.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
            let detail = parsed
                .get("stop_details")
                .and_then(|d| d.get("explanation"))
                .and_then(Value::as_str)
                .unwrap_or("no explanation given");
            bail!("the model declined this request ({detail})");
        }

        let content = parsed
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        // Only OUR tools come back as `tool_use`. Server tools (web_search,
        // web_fetch) appear as `server_tool_use` with their results already
        // attached, and must not be executed here.
        let tool_uses: Vec<&Value> = content
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .collect();

        // Citations accumulate across rounds: a search in round 1 can feed a
        // local-tool round 2 and the final prose in round 3.
        collect_citations(&content, &mut citations);

        // A long server-side search turn can be paused by the API; resending
        // the assistant message unchanged resumes it.
        if parsed.get("stop_reason").and_then(Value::as_str) == Some("pause_turn") {
            messages.push(json!({ "role": "assistant", "content": content }));
            continue;
        }

        if tool_uses.is_empty() {
            // Text blocks only -- thinking blocks carry `thinking`, not `text`.
            // Cited replies arrive as several adjacent text blocks forming one
            // sentence, so they are concatenated without a separator.
            let text: String = content
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .concat();
            let text = despeak(&text);
            remember(state, question, &text);
            return Ok(Answer {
                text,
                citations,
                tools_used: rs.tools_used,
                route: rs.last_route,
                profit: rs.last_profit,
            });
        }

        // The assistant turn goes back verbatim, thinking blocks included --
        // they must be echoed unchanged when continuing on the same model.
        messages.push(json!({ "role": "assistant", "content": content }));

        // All tool results must go back in ONE user message; splitting them
        // trains the model to stop making parallel calls.
        let tool_calls: Vec<ToolCall> = tool_uses
            .iter()
            .map(|call| ToolCall {
                name: call.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                input: call.get("input").cloned().unwrap_or(json!({})),
            })
            .collect();
        let outputs = run_round(state, fx, &mut rs, &tool_calls);
        let results: Vec<Value> = tool_uses
            .iter()
            .zip(outputs)
            .map(|(call, output)| {
                json!({
                    "type": "tool_result",
                    "tool_use_id": call.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "content": output.to_string(),
                })
            })
            .collect();
        messages.push(json!({ "role": "user", "content": results }));
    }

    bail!("gave up after {MAX_TOOL_ROUNDS} tool-call rounds without a final answer")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_citations_are_collected_and_deduplicated() {
        let content = vec![
            json!({ "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": { "query": "rename ship" } }),
            json!({ "type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [
                { "type": "web_search_result", "url": "https://a.example/x", "title": "A", "encrypted_content": "..", "page_age": "May 1, 2026" }
            ]}),
            json!({ "type": "text", "text": "Rename it ", "citations": [
                { "type": "web_search_result_location", "url": "https://a.example/x", "title": "A", "cited_text": "..", "encrypted_index": ".." }
            ]}),
            json!({ "type": "text", "text": "in the livery.", "citations": [
                { "type": "web_search_result_location", "url": "https://a.example/x", "title": "A", "cited_text": "..", "encrypted_index": ".." },
                { "type": "web_search_result_location", "url": "https://b.example/y", "title": "", "cited_text": "..", "encrypted_index": ".." }
            ]}),
        ];
        let mut out = Vec::new();
        collect_citations(&content, &mut out);
        assert_eq!(
            out,
            vec![
                Citation {
                    url: "https://a.example/x".into(),
                    title: "A".into()
                },
                Citation {
                    url: "https://b.example/y".into(),
                    title: "https://b.example/y".into()
                },
            ]
        );
    }

    #[test]
    fn fetch_citations_resolve_to_the_fetched_url() {
        let content = vec![
            json!({ "type": "web_fetch_tool_result", "tool_use_id": "srvtoolu_2", "content": {
                "type": "web_fetch_result", "url": "https://wiki.example/Ship_Naming", "retrieved_at": "2026-08-26T10:00:00Z",
                "content": { "type": "document", "title": "Ship naming", "source": { "type": "text", "media_type": "text/plain", "data": "..." } }
            }}),
            json!({ "type": "web_fetch_tool_result", "tool_use_id": "srvtoolu_3", "content": {
                "type": "web_fetch_tool_result_error", "error_code": "url_not_accessible"
            }}),
            json!({ "type": "text", "text": "Livery, then Name.", "citations": [
                { "type": "char_location", "document_index": 0, "document_title": "Ship naming", "start_char_index": 1, "end_char_index": 9, "cited_text": ".." }
            ]}),
        ];
        let mut out = Vec::new();
        collect_citations(&content, &mut out);
        assert_eq!(
            out,
            vec![Citation {
                url: "https://wiki.example/Ship_Naming".into(),
                title: "Ship naming".into()
            }]
        );
    }

    /// Two tool_use blocks in one assistant turn both run, and the results
    /// come back in the order the blocks were issued -- the API pairs each
    /// tool_result to its tool_use_id positionally in our message.
    #[test]
    fn a_round_runs_every_tool_use_and_keeps_order() {
        let f = crate::capabilities::testing::fixture("cutter");
        let fx = Recording::default();
        let mut rs = RoundState::default();
        let calls = vec![
            ToolCall { name: "say".into(), input: json!({ "text": "one" }) },
            ToolCall { name: "list_blueprints".into(), input: json!({}) },
            ToolCall { name: "say".into(), input: json!({ "text": "three" }) },
        ];
        let out = run_round(&f.state, &fx, &mut rs, &calls);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["spoken"], "one");
        assert!(out[1]["module_types"].is_array(), "{}", out[1]);
        assert_eq!(out[2]["spoken"], "three");
        let mut effects = fx.take();
        effects.sort();
        assert_eq!(effects, vec!["say \"one\"", "say \"three\""]);
        assert_eq!(rs.tools_used, vec!["say", "list_blueprints", "say"]);
    }

    #[test]
    fn a_repeated_identical_call_is_refused_with_a_structured_error() {
        let f = crate::capabilities::testing::fixture("cutter");
        let fx = Recording::default();
        let mut rs = RoundState::default();
        let call = ToolCall { name: "list_blueprints".into(), input: json!({}) };
        for _ in 0..2 {
            let out = run_round(&f.state, &fx, &mut rs, std::slice::from_ref(&call));
            assert!(out[0].get("error").is_none());
        }
        let out = run_round(&f.state, &fx, &mut rs, std::slice::from_ref(&call));
        assert_eq!(out[0]["error"]["kind"], "invalid_input");
        assert!(out[0]["error"]["hint"].as_str().unwrap().contains("change the parameters"));
    }

    #[test]
    fn research_tools_are_server_side_only() {
        for t in research_tools() {
            assert!(
                t.get("input_schema").is_none(),
                "server tools carry no schema"
            );
            assert!(t["type"].as_str().unwrap().ends_with("_20260209"));
        }
    }
}
