// 2026-10-16: the Combat tab's "By ship" section renders the store's
// `combat_by_ship` / `combat_sessions` rows. A template exception leaves a
// blank tab, so this renders the section server-side with the rows the
// store produced from one real day of the maintainer's journal
// (crates/ed-store/tests/fixtures/combat-2026-10-05.jsonl, pinned to the
// knob in tests/combat_sessions.rs) and asserts the figures are in the
// markup.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import CombatByShip from "../lib/CombatByShip.svelte";

const ships = [
  { ship_id: 37, ship: "type9_military", hull: "Type-10 Defender", ship_name: "", sessions: 1, secs: 34205, fighting_secs: 33815, kills: 304, kills_timed: 31, kills_fighter: 0, credits: 23848716, under_attack: 280, hull_hits: 0, shields_down: 1, died: 0, median_ttk_s: 37, p90_ttk_s: 166, median_interval_s: 68 },
  { ship_id: null, ship: "(unknown)", hull: "(unknown)", ship_name: "", sessions: 1, secs: 3597, fighting_secs: 2997, kills: 28, kills_timed: 12, kills_fighter: 12, credits: 3388700, under_attack: 83, hull_hits: 0, shields_down: 0, died: 0, median_ttk_s: 32, p90_ttk_s: 117, median_interval_s: 57 },
];
const sessions = [
  { id: 2, ship_id: 37, ship: "type9_military", hull: "Type-10 Defender", ship_name: "", entered_ts: "2026-10-05T02:33:42Z", left_ts: "2026-10-05T12:03:47Z", closed_by: "shutdown", secs: 34205, fighting_secs: 33815, kills: 304, kills_timed: 31, kills_fighter: 0, credits: 23848716, under_attack: 280, hull_hits: 0, shields_down: 1, died: 0, system_name: "Anana", median_ttk_s: 37 },
  { id: 1, ship_id: null, ship: "(unknown)", hull: "(unknown)", ship_name: "", entered_ts: "2026-10-05T00:01:32Z", left_ts: "2026-10-05T01:01:29Z", closed_by: "loadgame", secs: 3597, fighting_secs: 2997, kills: 28, kills_timed: 12, kills_fighter: 12, credits: 3388700, under_attack: 83, hull_hits: 0, shields_down: 0, died: 0, system_name: "Anana", median_ttk_s: 32 },
];

describe("CombatByShip with the fixture day's rows", () => {
  it("renders every ship that fought with its rate, time to kill and session count", () => {
    const { body } = render(CombatByShip, { props: { ships, sessions, selected: null, onPick: () => {} } });
    expect(body).toContain("By ship");
    expect(body).toContain("Type-10 Defender");
    expect(body).toContain("9 h 30 min"); // 34205 s
    expect(body).toContain("32.0"); // 304 kills over 9.5 h
    expect(body).toContain("37 s"); // median time to kill
    expect(body).toContain("166 s"); // p90
    expect(body).toContain("31 / 0"); // timed / fighter
    expect(body).toContain("12 / 12"); // the mid-fight day start: every timed kill came with the fighter out
    expect(body).toContain("2.5M cr"); // 23.8M over 9.5 h
    // Sessions list, every ship's, with the closer in words.
    expect(body).toContain("2026-10-05 02:33:42");
    expect(body).toContain("quit");
    expect(body).toContain("relaunched");
    expect(body).toContain("(12 fighter)");
  });

  it("narrows the sessions to the picked ship", () => {
    const { body } = render(CombatByShip, { props: { ships, sessions: sessions.slice(0, 1), selected: 37, onPick: () => {} } });
    expect(body).toContain("Sessions · Type-10 Defender");
    expect(body).not.toContain("relaunched");
  });

  it("renders nothing for a commander who never fought", () => {
    const { body } = render(CombatByShip, { props: { ships: [], sessions: [], selected: null, onPick: () => {} } });
    expect(body).not.toContain("By ship");
    expect(body).not.toContain("<table");
  });
});
