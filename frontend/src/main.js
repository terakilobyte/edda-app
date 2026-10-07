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

// A plain browser, no Tauri: `?transport=fake` answers from fixtures
// (browserTransport.js) so Playwright and an agent's browser can see the
// real tabs render. The chunk loads only when asked for.
if (new URLSearchParams(location.search).get("transport") === "fake") {
  const [{ browserTransport }, { setTransport }] = await Promise.all([import("./lib/browserTransport.js"), import("./lib/transport.js")]);
  setTransport(browserTransport());
}

const app = mount(App, { target: document.getElementById("app") });

export default app;
