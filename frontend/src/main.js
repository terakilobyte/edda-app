import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";

// Every uncaught error in the webview reaches the app log (target
// `frontend`), beside the Rust side's lines. 2026-10-07: a plot the
// store had received and logged as shown never rendered, and nothing
// recorded why -- a template exception leaves a blank tab and no trace.
import { frontendLog } from "./lib/api.js";
const report = (kind, detail) => { try { frontendLog("warn", `${kind}: ${detail}`.slice(0, 2000)).catch(() => {}); } catch { /* the log is best effort */ } };
window.addEventListener("error", (e) => report("js error", `${e.message} @ ${e.filename ?? "?"}:${e.lineno ?? "?"}:${e.colno ?? "?"}${e.error?.stack ? "\n" + e.error.stack : ""}`));
window.addEventListener("unhandledrejection", (e) => report("unhandled rejection", e.reason?.stack ?? String(e.reason)));

const app = mount(App, { target: document.getElementById("app") });

export default app;
