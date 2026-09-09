// The transport seam: api.js talks to whatever `setTransport` gave it and
// turns every failure into an ApiError.
import { describe, it, expect, beforeEach } from "vitest";
import { fakeTransport } from "./fakeTransport.js";
import { setTransport } from "../lib/transport.js";
import { call, on, ApiError, getStatus, missions, checkBlueprint, shipModules, materialShopping, onJournalChanged } from "../lib/api.js";

let t;
beforeEach(() => { t = fakeTransport(); setTransport(t); });

describe("call()", () => {
  it("invokes through the current transport", async () => {
    t = fakeTransport({ get_status: { commander: "Jameson" } }); setTransport(t);
    expect(await call("get_status")).toEqual({ commander: "Jameson" });
    expect(t.calls).toEqual([{ name: "get_status", args: undefined }]);
  });

  it("normalises a string rejection (what Tauri commands throw) into ApiError", async () => {
    t = fakeTransport({ get_status: () => Promise.reject("no journal yet") }); setTransport(t);
    const err = await call("get_status").catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.message).toBe("no journal yet");
    expect(err.cause).toBe("no journal yet");
    expect(err.command).toBe("get_status");
    // The panels' `error = String(e)` keeps showing just the message.
    expect(String(err)).toBe("no journal yet");
  });

  it("keeps a structured capability error's kind, retryable flag and hint", async () => {
    const wire = { kind: "invalid_input", message: "landing pad size unknown for hull \"fdev_next_hull\"", retryable: false, hint: "pass min_pad (small, medium or large), or 'any'" };
    t = fakeTransport({ profit_routes: () => Promise.reject(wire), sync_now: () => Promise.reject({ kind: "unavailable", message: "database busy", retryable: true }), get_status: () => Promise.reject("no journal yet") }); setTransport(t);
    const err = await call("profit_routes", { query: {} }).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    // The message stays clean prose for the panel...
    expect(String(err)).toBe('landing pad size unknown for hull "fdev_next_hull"');
    // ...and the guidance travels beside it, structured.
    expect(err.kind).toBe("invalid_input");
    expect(err.retryable).toBe(false);
    expect(err.hint).toContain("min_pad");
    const busy = await call("sync_now").catch((e) => e);
    expect(busy.kind).toBe("unavailable");
    expect(busy.retryable).toBe(true);
    expect(busy.hint).toBeNull();
    // A bare-string rejection (an untyped command) has no kind.
    const bare = await call("get_status").catch((e) => e);
    expect(bare.kind).toBe("unknown");
    expect(bare.retryable).toBe(false);
  });

  it("normalises an Error and an object rejection", async () => {
    t = fakeTransport({ a: new Error("boom"), b: () => Promise.reject({ message: "denied" }), c: () => Promise.reject({ code: 7 }) }); setTransport(t);
    expect((await call("a").catch((e) => e)).message).toBe("boom");
    expect((await call("b").catch((e) => e)).message).toBe("denied");
    expect((await call("c").catch((e) => e)).message).toBe('{"code":7}');
  });
});

describe("generated wrappers", () => {
  it("send the same command names and argument shapes as before", async () => {
    await getStatus();
    await missions();
    await missions(false);
    await checkBlueprint("fsd", "Increased range", 0, 5);
    await shipModules();
    await materialShopping({ trades: [1] });
    expect(t.calls).toEqual([
      { name: "get_status", args: undefined },
      { name: "missions", args: { activeOnly: true } },
      { name: "missions", args: { activeOnly: false } },
      { name: "check_blueprint", args: { moduleType: "fsd", name: "Increased range", fromGrade: 0, targetGrade: 5, minimum: false, complete: true } },
      { name: "ship_modules", args: { shipId: null } },
      { name: "material_shopping", args: { trades: [1] } },
    ]);
  });
});

describe("on()", () => {
  it("subscribes through the transport and returns an unsubscribe", async () => {
    const seen = [];
    const off = on("journal-changed", (e) => seen.push(e.payload));
    await Promise.resolve();
    t.emit("journal-changed", 1);
    expect(seen).toEqual([1]);
    off();
    await new Promise((r) => setTimeout(r, 0));
    t.emit("journal-changed", 2);
    expect(seen).toEqual([1]);
    expect(t.count("journal-changed")).toBe(0);
  });

  it("the named listeners work the same way (and still tolerate being awaited)", async () => {
    const off = await onJournalChanged(() => {});
    expect(typeof off).toBe("function");
    expect(t.count("journal-changed")).toBe(1);
  });
});
