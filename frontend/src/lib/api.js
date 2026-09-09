// The API seam. Every command the frontend invokes and every backend event
// it listens for is one row in the tables below; the named wrappers the
// panels import are generated from them. Calls go through `transport.js`
// (Tauri by default, a fake in tests) and failures become `ApiError`.
//
// Command rows are `[tauriCommand, argNames?, defaults?]`: positional
// arguments are mapped onto the named object Tauri expects. `"*"` means the
// single argument is the args object itself. The contract test checks each
// command against src-tauri's generate_handler! and each event against
// what Rust actually emits.
import { getTransport } from "./transport.js";

export class ApiError extends Error {
  /**
   * @param {string} command
   * @param {unknown} cause the raw rejection: a structured
   *   `{kind, message, retryable, hint?, data?}` from a capability-backed
   *   command, or a bare string from one not yet typed
   */
  constructor(command, cause) {
    super(ApiError.messageOf(cause));
    this.name = "ApiError";
    this.command = command;
    this.cause = cause;
    const o = cause && typeof cause === "object" ? cause : {};
    /** @type {"not_found"|"invalid_input"|"unavailable"|"internal"|"unknown"} */
    this.kind = typeof o.kind === "string" ? o.kind : "unknown";
    this.retryable = o.retryable === true;
    /** @type {string|null} what to change before retrying, when the backend says */
    this.hint = typeof o.hint === "string" ? o.hint : null;
    this.data = o.data ?? null;
  }
  static messageOf(cause) {
    if (cause instanceof Error) return cause.message;
    if (typeof cause === "string") return cause;
    if (cause && typeof cause === "object" && typeof cause.message === "string") return cause.message;
    try { return JSON.stringify(cause); } catch { return String(cause); }
  }
  // The panels show `String(e)`; keep that the bare message.
  toString() { return this.message; }
}

/** Invoke a backend command; rejects with ApiError. */
export async function call(command, args) {
  try {
    return await getTransport().invoke(command, args);
  } catch (e) {
    throw e instanceof ApiError ? e : new ApiError(command, e);
  }
}

/**
 * Subscribe to a backend event. Returns the unsubscribe function
 * synchronously (the subscription itself completes on a microtask, and
 * unsubscribing before then is honoured).
 * @param {string} event
 * @param {(e: {event: string, payload: any}) => void} cb
 * @returns {() => void}
 */
export function on(event, cb) {
  let live = true;
  const p = getTransport().listen(event, cb).then((un) => { if (!live) un?.(); return un; }).catch(() => null);
  return () => { live = false; p.then((un) => un?.()); };
}

// ── Commands ───────────────────────────────────────────────────────
export const COMMANDS = {
  // Status & inventory
  getStatus: ["get_status"],
  getInventory: ["get_inventory"],
  listCommodities: ["list_commodities"],
  syncNow: ["sync_now"],
  dbStats: ["db_stats"],
  dataLocationGet: ["data_location_get"],
  dataLocationChoose: ["data_location_choose"],
  vacuum: ["vacuum"],
  // Engineering
  listModuleTypes: ["list_module_types"],
  listBlueprintNames: ["list_blueprint_names", ["moduleType"]],
  checkBlueprint: ["check_blueprint", ["moduleType", "name", "fromGrade", "targetGrade", "minimum", "complete"], { minimum: false, complete: true }],
  blueprintAccess: ["blueprint_access", ["moduleType", "name", "grade"]],
  listEngineers: ["list_engineers"],
  checkExperimental: ["check_experimental", ["moduleType", "name"]],
  shipModules: ["ship_modules", ["shipId"], { shipId: null }],
  shipsList: ["ships_list", ["includeHistorical"], { includeHistorical: false }],
  carrierStatus: ["carrier_status"],
  shipSlef: ["ship_slef", ["shipId", "proposed"], { shipId: null, proposed: null }],
  shipLinks: ["ship_links", ["shipId"], { shipId: null }],
  materialShopping: ["material_shopping", "*"],
  // Galaxy
  findSystem: ["find_system", ["name"]],
  stationsInSystem: ["stations_in_system", ["system", "includeCarriers", "includeMinor"], { includeCarriers: false, includeMinor: false }],
  findStation: ["find_station", ["name"]],
  nearestService: ["nearest_service", ["system", "service", "minPad", "radiusLy", "includeCarriers"]],
  stationMarket: ["station_market", ["stationId"]],
  commoditySearch: ["commodity_search", ["query"]],
  outfittingSearch: ["outfitting_search", ["query"]],
  shipyardSearch: ["shipyard_search", ["query"]],
  // Trade (`query` keys are snake_case: they deserialize straight into ProfitQuery)
  profitRoutes: ["profit_routes", ["query"]],
  cancelSearch: ["cancel_search"],
  currentRoute: ["current_route"],
  powerplayOptions: ["powerplay_options"],
  // Galaxy index (the bundled bubble)
  galaxyStatus: ["galaxy_status"],
  activityHeatmap: ["activity_heatmap"],
  feedbackSend: ["feedback_send", ["text", "includeLog"]],
  telemetryPrefs: ["telemetry_prefs"],
  telemetryPrefsSet: ["telemetry_prefs_set", ["enabled"]],
  shipScoopInfo: ["ship_scoop_info", ["shipId"]],
  sellHoldSearch: ["sell_hold_search", ["radiusLy", "minPad", "includeCarriers", "maxAgeHours"]],
  markHere: ["mark_here"],
  miningSearch: ["mining_search", ["text", "radiusLy"]],
  miningMaterials: ["mining_materials"],
  markAdd: ["mark_add", ["label", "body", "note"]],
  markRemove: ["mark_remove", ["id"]],
  gameState: ["game_state"],
  // App self-update
  appUpdateCheck: ["app_update_check"],
  releaseNotesGet: ["release_notes_get"],
  releaseNotesSeen: ["release_notes_seen"],
  appUpdateInstall: ["app_update_install"],
  appRestart: ["app_restart"],
  // Voice input
  listenStatus: ["listen_status"],
  listenConfigSet: ["listen_config_set", ["config"]],
  listenSetup: ["listen_setup", ["model", "wakeWord"], { wakeWord: null }],
  listenModelsRemove: ["listen_models_remove"],
  audioDevices: ["audio_devices"],
  listenPtt: ["listen_ptt", ["down"]],
  joyDevices: ["joy_devices"],
  pttCapture: ["ptt_capture", ["secs"], { secs: 8 }],
  // Routing
  galaxyFind: ["galaxy_find", ["name"]],
  edsmSystem: ["edsm_system", ["name"]],
  plotRoute: ["plot_route", ["query"]],
  injectionsAvailable: ["injections_available"],
  cancelRoute: ["cancel_route"],
  importSpanshRoute: ["import_spansh_route", ["link"]],
  routeActivate: ["route_activate", ["route", "source"]],
  tradeFollowStart: ["trade_follow_start", ["legs", "kind"]],
  tradeFollowStop: ["trade_follow_stop"],
  tradeFollowStatus: ["trade_follow_status"],
  carrierRoutePlot: ["carrier_route_plot", ["to", "from"], { from: null }],
  carrierRouteStart: ["carrier_route_start", ["route"]],
  carrierRouteStatus: ["carrier_route_status"],
  carrierRouteClear: ["carrier_route_clear"],
  carrierRouteNext: ["carrier_route_next"],
  routeClear: ["route_clear"],
  routeClearInGame: ["route_clear_in_game"],
  routeFollowStatus: ["route_follow_status"],
  routeActiveGet: ["route_active_get"],
  routeTargetNext: ["route_target_next"],
  targetMacroEnabledGet: ["target_macro_enabled_get"],
  targetMacroEnabledSet: ["target_macro_enabled_set", ["enabled"]],
  gameRouteMaxGet: ["game_route_max_get"],
  gameRouteMaxSet: ["game_route_max_set", ["lightyears"]],
  targetMacroGet: ["target_macro_get"],
  targetMacroSet: ["target_macro_set", ["steps"]],
  targetMacroCheck: ["target_macro_check"],
  targetMacroPresets: ["target_macro_presets"],
  targetMacroTest: ["target_macro_test"],
  routePlotTest: ["route_plot_test", ["system"]],
  mapSetupTest: ["map_setup_test"],
  mapSetupSay: ["map_setup_say", ["text"]],
  mapSetupCancel: ["map_setup_cancel"],
  mapSetupTarget: ["map_setup_target"],
  routePlotInGame: ["route_plot_in_game", ["system"]],
  targetKeyStatus: ["target_key_status"],
  targetTriggerCapture: ["target_trigger_capture"],
  targetTriggerClear: ["target_trigger_clear"],
  mapPointCapture: ["map_point_capture", ["which"]],
  mapPointsGet: ["map_points_get"],
  mapDelaySet: ["map_delay_set", ["which", "ms"]],
  mapPointsClear: ["map_points_clear"],
  macroRecordStart: ["macro_record_start"],
  macroRecordStop: ["macro_record_stop", ["system"]],
  galaxyNear: ["galaxy_near", ["pos", "radiusLy", "limit"], { limit: 2000 }],
  nameComplete: ["name_complete", ["kind", "prefix"]],
  // Powerplay & merits
  meritModel: ["merit_model"],
  powerplaySeen: ["powerplay_seen"],
  meritTimeline: ["merit_timeline", ["since", "bucket"], { bucket: "day" }],
  // Combat
  combatSummary: ["combat_summary", ["since"]],
  combatTimeline: ["combat_timeline", ["since", "bucket"], { bucket: "day" }],
  recentKills: ["recent_kills", ["limit"], { limit: 30 }],
  // Missions
  missions: ["missions", ["activeOnly"], { activeOnly: true }],
  // Voice, callouts, overlay
  voiceStatus: ["voice_status"],
  voiceModels: ["voice_models"],
  voiceUseWindows: ["voice_use_windows"],
  voiceCatalog: ["voice_catalog"],
  voiceInstall: ["voice_install", ["model"]],
  voiceRemove: ["voice_remove", ["model"]],
  voiceInstallDefault: ["voice_install_default"],
  personas: ["personas"],
  setPersona: ["set_persona", ["id"]],
  setVoice: ["set_voice", ["model"]],
  voiceServerGet: ["voice_server_get"],
  calloutsGet: ["callouts_get"],
  calloutsSet: ["callouts_set", ["off"]],
  signalWatchGet: ["signal_watch_get"],
  signalWatchSet: ["signal_watch_set", ["ids"]],
  voiceServerProbe: ["voice_server_probe", ["config"]],
  voiceServerSet: ["voice_server_set", ["enabled", "config"]],
  speechEngineStatus: ["speech_engine_status"],
  speechEngineInstall: ["speech_engine_install", ["engine"]],
  speechEngineStart: ["speech_engine_start", ["engine"]],
  getAiConfig: ["get_ai_config"],
  aiEval: ["ai_eval", ["only", "provider"], { only: null, provider: null }],
  setAiConfig: ["set_ai_config", "*"],
  say: ["say", ["text"]],
  sayNow: ["say_now", ["text"]],
  voiceInterrupt: ["voice_interrupt"],
  setMuted: ["set_muted", ["muted"]],
  recentCallouts: ["recent_callouts"],
  setOverlayInteractive: ["set_overlay_interactive", ["interactive"]],
  overlayVisible: ["overlay_visible", ["visible"]],
  // Ship computer. ai_ask resolves to {text, citations: [{url, title}], …}.
  aiAsk: ["ai_ask", ["question"]],
  aiReset: ["ai_reset"],
};

// ── Events from the backend ────────────────────────────────────────
export const EVENTS = {
  onJournalChanged: "journal-changed",
  onSyncProgress: "sync-progress",
  onSyncComplete: "sync-complete",
  onCallout: "callout",
  onSupercharge: "supercharge",
  onOverlayInteractive: "overlay-interactive",
  onGameState: "game-state",
  onKnowledgeProgress: "knowledge-progress",
  onRouteProgress: "route-progress",
  onRouteCandidate: "route-candidate",
  onRouteReplanned: "route-replanned",
  onRouteFollow: "route-follow",
  onTradeFollow: "trade-follow",
  onCarrierRoute: "carrier-route",
  onListenState: "listen-state",
  onListenHeard: "listen-heard",
  onListenPartial: "listen-partial",
  onListenReply: "listen-reply",
  onListenSetup: "listen-setup",
  onSpeechEngineProgress: "speech-engine-progress",
  onAppUpdate: "app-update",
};

function wrap([command, names, defaults = {}]) {
  if (!names) return () => call(command);
  if (names === "*") return (args) => call(command, args);
  return (...vals) => {
    const args = {};
    names.forEach((n, i) => { args[n] = vals[i] !== undefined ? vals[i] : defaults[n]; });
    return call(command, args);
  };
}
const wrapped = Object.fromEntries(Object.entries(COMMANDS).map(([k, row]) => [k, wrap(row)]));
const listeners = Object.fromEntries(Object.entries(EVENTS).map(([k, ev]) => [k, (cb) => on(ev, cb)]));

// ── Named wrappers (generated) ─────────────────────────────────────
export const {
  getStatus, getInventory, listCommodities, syncNow, dbStats, dataLocationGet, dataLocationChoose, vacuum,
  listModuleTypes, listBlueprintNames, checkBlueprint, blueprintAccess, listEngineers, checkExperimental, shipModules, shipsList, carrierStatus, shipSlef, shipLinks, materialShopping,
  findSystem, stationsInSystem, findStation, nearestService, stationMarket, commoditySearch, outfittingSearch, shipyardSearch,
  profitRoutes, cancelSearch, currentRoute, powerplayOptions,
  galaxyStatus, activityHeatmap, feedbackSend, telemetryPrefs, telemetryPrefsSet, shipScoopInfo, sellHoldSearch, miningSearch, miningMaterials, markAdd, markRemove, markHere, gameState,
  appUpdateCheck, appUpdateInstall, appRestart, releaseNotesGet, releaseNotesSeen,
  listenStatus, listenConfigSet, listenSetup, listenModelsRemove, audioDevices, listenPtt, joyDevices, pttCapture,
  galaxyFind, edsmSystem, plotRoute, injectionsAvailable, cancelRoute, importSpanshRoute, routeActivate, routeClear, routeClearInGame, routeFollowStatus, routeActiveGet, routeTargetNext,
  tradeFollowStart, tradeFollowStop, tradeFollowStatus, carrierRoutePlot, carrierRouteStart, carrierRouteStatus, carrierRouteClear, carrierRouteNext,
  targetMacroEnabledGet, targetMacroEnabledSet, gameRouteMaxGet, gameRouteMaxSet, targetMacroGet, targetMacroSet, targetMacroCheck, targetMacroPresets, targetMacroTest,
  routePlotTest, mapSetupTest, mapSetupSay, mapSetupCancel, mapSetupTarget, routePlotInGame, targetKeyStatus, targetTriggerCapture, targetTriggerClear, mapPointCapture, mapPointsGet, mapDelaySet, mapPointsClear, macroRecordStart, macroRecordStop,
  galaxyNear, nameComplete,
  meritModel, powerplaySeen, meritTimeline, combatSummary, combatTimeline, recentKills, missions,
  voiceStatus, voiceModels, voiceUseWindows, voiceCatalog, voiceInstall, voiceRemove, voiceInstallDefault, personas, setPersona, setVoice, voiceServerGet,
  calloutsGet, calloutsSet, signalWatchGet, signalWatchSet, voiceServerProbe, voiceServerSet, speechEngineStatus, speechEngineInstall, speechEngineStart,
  getAiConfig, aiEval, say, sayNow, voiceInterrupt, setMuted, recentCallouts, setOverlayInteractive, overlayVisible, aiAsk, aiReset,
} = wrapped;

/** set_ai_config takes the three common fields plus provider-specific extras. */
export const setAiConfig = (apiKey, model, research = null, extra = {}) =>
  wrapped.setAiConfig({ apiKey, model, research, ...extra });

export const {
  onJournalChanged, onSyncProgress, onSyncComplete, onCallout, onSupercharge, onOverlayInteractive, onGameState,
  onKnowledgeProgress,
  onRouteProgress, onRouteCandidate, onRouteReplanned, onRouteFollow, onTradeFollow, onCarrierRoute,
  onListenState, onListenHeard, onListenPartial, onListenReply, onListenSetup, onSpeechEngineProgress, onAppUpdate,
} = listeners;
