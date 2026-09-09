<script>
  import { getStatus, currentRoute } from "./api.js";
  import { fmtInt } from "./format.js";
  import { fuelPct, fuelLabel } from "./ui.js";
  import { journalResource } from "./lifecycle.svelte.js";

  let status = $state(null);
  let routeView = $state(null);

  const res = journalResource(async () => {
    status = await getStatus();
    try { routeView = await currentRoute(); } catch {}
  });
  const error = $derived(res.error);
  // Unknown fuel draws an empty bar but is not "low".
  const fuel = $derived(fuelPct(status));
</script>

<section class="panel">
  <h2>Ship</h2>
  {#if error}
    <p class="error">{error}</p>
  {:else if !status}
    <p class="muted">Loading…</p>
  {:else}
    <div class="sys">{status.location?.system_name ?? "Unknown system"}</div>
    <div class="row" style="margin-bottom:0.5rem">
      {#if status.location?.docked}
        <span class="pill cyan">Docked · {status.location.station_name}</span>
      {:else}
        <span class="pill">In flight</span>
      {/if}
      {#if status.controlling_power}
        <span class="pill accent">{status.controlling_power}</span>
        <span class="pill">{status.power_state ?? "?"}</span>
      {/if}
    </div>

    <dl class="kv">
      <dt>Ship</dt>
      <dd>{status.ship_name || status.ship || "—"}{status.ship_name && status.ship ? ` (${status.ship})` : ""}</dd>

      {#if status.nav?.target_system}
        <dt>Next</dt>
        <dd>
          {status.nav.target_system}
          <span class="pill {status.nav.scoopable === false ? 'warn' : status.nav.scoopable ? 'ok' : ''}">
            {status.nav.star_class ?? "?"}{status.nav.scoopable === false ? " · not scoopable" : ""}
          </span>
          {#if status.nav.remaining_jumps != null}
            <span class="muted">· {status.nav.remaining_jumps} jumps</span>
          {/if}
        </dd>
      {/if}

      {#if routeView?.route}
        <dt>Route</dt>
        <dd class="small">{routeView.route.hops.length - 1} jumps · {routeView.route.total_ly.toFixed(0)} ly to {routeView.route.hops[routeView.route.hops.length - 1].system}
          {#if routeView.next}<div class="muted">{routeView.next}</div>{/if}
          {#if routeView.brief}<details><summary class="muted">briefing</summary><div class="muted">{routeView.brief}</div></details>{/if}
        </dd>
      {/if}

      <dt>Fuel</dt>
      <dd>
        <span class="num">{fuelLabel(status)}</span>
        <div class="bar" style="margin-top:4px"><div class="bar-fill {fuel != null && fuel < 25 ? 'bad' : ''}" style="width:{fuel ?? 0}%"></div></div>
      </dd>

      <dt>Cargo</dt>
      <dd class="num">{fmtInt(status.cargo_count)} / {fmtInt(status.cargo_capacity)} t</dd>

      {#if status.location?.system_security || status.location?.population != null}
        <dt>System</dt>
        <dd class="small muted">
          {status.location.system_allegiance ?? ""}
          {status.location.system_security ? `· ${status.location.system_security}` : ""}
          {status.location.population != null ? `· pop ${fmtInt(status.location.population)}` : ""}
        </dd>
      {/if}
    </dl>
  {/if}
</section>

<style>
  .sys { font-size: 1.25rem; font-weight: 600; color: var(--accent-2); margin-bottom: 0.3rem; }
</style>
