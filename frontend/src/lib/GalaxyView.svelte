<script>
  // 3D galaxy with the plotted route: a sampled star backdrop (shipped with
  // the app), the populated systems as a brighter layer, the route as a
  // line with hop markers, the follow cursor highlighted. Left drag
  // rotates, right drag pans, scroll zooms, double-click reframes (and
  // restores Sol-south).
  import { onMount, onDestroy } from "svelte";
  import { galaxyNear } from "./api.js";
  import * as THREE from "three";
  import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
  import { Line2 } from "three/examples/jsm/lines/Line2.js";
  import { LineGeometry } from "three/examples/jsm/lines/LineGeometry.js";
  import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
  // Fat lines need the viewport size; every material made is kept so
  // resize() can update them.
  const lineMaterials = new Set();
  function fatLine(points, color, width, opacity) {
    const geo = new LineGeometry();
    geo.setPositions(points.flatMap((p) => [p.x, p.y, p.z]));
    const mat = new LineMaterial({ color, linewidth: width, transparent: true, opacity, depthTest: false, depthWrite: false, worldUnits: false });
    mat.resolution.set(host?.clientWidth || 800, host?.clientHeight || 400);
    lineMaterials.add(mat);
    const line = new Line2(geo, mat);
    line.computeLineDistances();
    line.renderOrder = 10;
    return line;
  }

  let { route = null, nextIndex = 0, height = 420, candidates = [], focus = null, idleRotate = false, heat = null, identify = true, starBrightness = 1 } = $props();

  let host = $state(null);
  let renderer, scene, camera, controls, raf, ro;
  let mapInteractionActive = false, lastMapInteraction = 0;
  let routeGroup = null, focusMarker = null;
  let backdrop = null, populated = null;
  let ready = false;
  let hover = $state("");
  // Screen-space labels for the ends of the route, the next hop and Sol.
  let labels = $state([]);
  function updateLabels() {
    if (!camera || !host) return;
    const w = host.clientWidth, h = host.clientHeight;
    const out = [];
    const place = (p, text, kind) => {
      const v = p.clone().project(camera);
      if (v.z > 1 || v.z < -1) return;
      const x = (v.x + 1) / 2 * w, y = (1 - v.y) / 2 * h;
      if (x < -40 || x > w + 40 || y < -20 || y > h + 20) return;
      out.push({ x, y, text, kind });
    };
    if (route?.hops?.length) {
      const hops = route.hops;
      place(toScene(hops[0].pos), hops[0].name, "start");
      place(toScene(hops[hops.length - 1].pos), hops[hops.length - 1].name, "end");
      if (nextIndex > 0 && nextIndex < hops.length - 1) place(toScene(hops[nextIndex].pos), `next: ${hops[nextIndex].name}`, "next");
    }
    place(new THREE.Vector3(0, 0, 0), "Sol", "sol");
    place(toScene([-9530.5, -910.28, 19808.125]), "Colonia", "sol");
    place(toScene([25.21875, -20.90625, 25899.96875]), "Sagittarius A*", "sol");
    if (pinned) place(toScene(pinned.pos), pinned.text, "pin");
    labels = out;
  }

  const CLASS_COLOR = {
    neutron: 0x7ec8ff, white_dwarf: 0xc9d6ff, black_hole: 0xff5c5c,
    o: 0x9fb4ff, b: 0xaac4ff, a: 0xe0e8ff, f: 0xfff4d6, g: 0xffe59a, k: 0xffc36b, m: 0xff8c5a,
    l: 0xb06b4a, t: 0x8a5a55, y: 0x6a4a55, proto: 0xffb3e6, exotic: 0xc0a0ff, unknown: 0x8b95a5,
  };
  // Star class codes as the index stores them (ed_galaxy::StarClass::code).
  const CODE_COLOR = [0x8b95a5, 0x9fb4ff, 0xaac4ff, 0xe0e8ff, 0xfff4d6, 0xffe59a, 0xffc36b, 0xff8c5a, 0xb06b4a, 0x8a5a55, 0x6a4a55, 0xffb3e6, 0xc0a0ff, 0xc9d6ff, 0x7ec8ff, 0xff5c5c];

  // Galactic coordinates: x east, y up, z toward the core. three.js is
  // right-handed with y up, so z is negated to keep east on the right.
  const toScene = (p) => new THREE.Vector3(p[0], p[1], -p[2]);

  async function loadStars(url, size, dim, popSpan = 0, opacity = 0.7, { hidePopulated = false, populatedLayer = false } = {}) {
    const buf = await (await fetch(url)).arrayBuffer();
    const n = Math.floor(buf.byteLength / 16);
    const view = new DataView(buf);
    const pos = new Float32Array(n * 3);
    const col = new Float32Array(n * 3);
    const c = new THREE.Color();
    const inhabitedGreen = new THREE.Color(0x55d98a);
    for (let i = 0; i < n; i++) {
      const o = i * 16;
      pos[i * 3] = view.getFloat32(o, true);
      pos[i * 3 + 1] = view.getFloat32(o + 4, true);
      pos[i * 3 + 2] = -view.getFloat32(o + 8, true);
      const classCode = view.getUint8(o + 12);
      // Byte 13 is population on a log scale (0 = none): populated systems
      // glow by how many people live there, not flatly for having a station.
      const pop = view.getUint8(o + 13) / 255;
      // Occupied systems have their own layer. Zeroing their sampled
      // backdrop copy prevents additive class colour from washing out green.
      if (hidePopulated && pop > 0) {
        col[i * 3] = 0; col[i * 3 + 1] = 0; col[i * 3 + 2] = 0;
        continue;
      }
      c.setHex(CODE_COLOR[classCode] ?? 0x8b95a5);
      // Preserve the stellar-class colour, but make inhabited space read as
      // a soft green cloud. Population is log-scaled, so Earth-like systems
      // tint more strongly without making small colonies disappear.
      if (populatedLayer || pop > 0) c.lerp(inhabitedGreen, 0.38 + 0.32 * pop);
      // A known stellar class is our compact "explored data exists" cue.
      // Unknown-class backdrop points remain present but recede by half.
      const exploredScale = !populatedLayer && classCode === 0 ? 0.5 : 1;
      c.multiplyScalar((dim + popSpan * pop) * exploredScale);
      col[i * 3] = c.r; col[i * 3 + 1] = c.g; col[i * 3 + 2] = c.b;
    }
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(pos, 3));
    geo.setAttribute("color", new THREE.BufferAttribute(col, 3));
    // Additive: dense regions pile up towards white, so the per-point
    // contribution has to stay small where thousands of points overlap.
    const mat = new THREE.PointsMaterial({ size, vertexColors: true, sizeAttenuation: false, transparent: true, opacity, depthWrite: false, blending: THREE.AdditiveBlending });
    // The layer's designed opacity; the star-brightness control scales
    // relative to it, so full slider = the normal look.
    mat.userData.baseOpacity = opacity;
    return new THREE.Points(geo, mat);
  }

  // Maintainer (2026-09-04, live-activity feedback): the star layers dim under
  // the heat layer, commander-controlled ("a slider for controlling the
  // brightness might be the way to go").
  function applyStarBrightness() {
    for (const layer of [backdrop, populated]) {
      if (layer?.material) layer.material.opacity = (layer.material.userData.baseOpacity ?? 1) * starBrightness;
    }
  }
  $effect(() => { void starBrightness; if (ready) applyStarBrightness(); });

  function buildRoute() {
    if (routeGroup) { scene.remove(routeGroup); routeGroup.traverse((o) => { o.geometry?.dispose(); if (o.material) { lineMaterials.delete(o.material); o.material.dispose?.(); } }); routeGroup = null; }
    if (!route?.hops?.length || !scene) return;
    routeGroup = new THREE.Group();
    const hops = route.hops;
    const pts = hops.map((h) => toScene(h.pos));
    // Path: ordinary segments orange, supercharged segments blue; drawn
    // as fat lines on top of everything so the backdrop never hides them.
    for (let i = 1; i < hops.length; i++) {
      routeGroup.add(fatLine([pts[i - 1], pts[i]], hops[i].boosted ? 0x9ad9ff : 0xffa348, 2.5, i < nextIndex ? 0.4 : 1.0));
    }
    // Hop markers: colour by star class, bigger for ends and neutrons.
    const pos = new Float32Array(hops.length * 3), col = new Float32Array(hops.length * 3);
    const c = new THREE.Color();
    hops.forEach((h, i) => {
      pos[i * 3] = pts[i].x; pos[i * 3 + 1] = pts[i].y; pos[i * 3 + 2] = pts[i].z;
      c.setHex(CLASS_COLOR[h.class] ?? 0x8b95a5);
      col[i * 3] = c.r; col[i * 3 + 1] = c.g; col[i * 3 + 2] = c.b;
    });
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(pos, 3));
    geo.setAttribute("color", new THREE.BufferAttribute(col, 3));
    const markers = new THREE.Points(geo, new THREE.PointsMaterial({ size: 7, vertexColors: true, sizeAttenuation: false, depthWrite: false, depthTest: false }));
    markers.renderOrder = 11;
    markers.name = "hops";
    routeGroup.add(markers);
    // Ends and the next system.
    const ball = (p, color, r) => { const s = new THREE.Mesh(new THREE.SphereGeometry(r, 12, 12), new THREE.MeshBasicMaterial({ color })); s.position.copy(p); return s; };
    const span = Math.max(1, route.straight_ly ?? 100);
    routeGroup.add(ball(pts[0], 0xffffff, span * 0.004));
    routeGroup.add(ball(pts[pts.length - 1], 0xff8c1a, span * 0.004));
    if (nextIndex > 0 && nextIndex < pts.length) {
      const cur = ball(pts[nextIndex], 0x39d98a, span * 0.005);
      cur.name = "next";
      routeGroup.add(cur);
    }
    // Other candidates from a plot in progress: one yellow polyline each.
    for (const c of candidates ?? []) {
      if (!c?.hops?.length) continue;
      routeGroup.add(fatLine(c.hops.map((h) => toScene(h.pos)), 0xffb000, 1.8, 0.8));
    }
    scene.add(routeGroup);
  }

  // The live activity layer: unlabeled additive glow at 100 ly cell
  // centers — orange for markets, cyan for traffic, whitening while a
  // cell is RISING (heating up), dimming as the backend decay cools it.
  // Never pickable: raycast is a no-op, so no interaction can turn a
  // blob into a name.
  let heatLayer = null;
  // Round glow sprite (maintainer: "instead of squares use spheres") — a
  // radial-gradient texture turns each additive point into a soft orb.
  let heatSprite = null;
  function glowSprite() {
    if (heatSprite) return heatSprite;
    const cv = document.createElement("canvas");
    cv.width = cv.height = 64;
    const g = cv.getContext("2d");
    const grad = g.createRadialGradient(32, 32, 0, 32, 32, 32);
    grad.addColorStop(0, "rgba(255,255,255,1)");
    grad.addColorStop(0.35, "rgba(255,255,255,0.6)");
    grad.addColorStop(1, "rgba(255,255,255,0)");
    g.fillStyle = grad;
    g.fillRect(0, 0, 64, 64);
    heatSprite = new THREE.CanvasTexture(cv);
    return heatSprite;
  }
  function rebuildHeat() {
    if (heatLayer) { scene?.remove(heatLayer); heatLayer.geometry?.dispose(); heatLayer.material?.dispose(); heatLayer = null; }
    if (!heat?.length || !scene) return;
    const pos = new Float32Array(heat.length * 3), col = new Float32Array(heat.length * 3);
    const c = new THREE.Color(), market = new THREE.Color(0xff8c1a), traffic = new THREE.Color(0x5ec8e5), hot = new THREE.Color(0xffffff);
    heat.forEach((cell, i) => {
      pos[i * 3] = cell.x; pos[i * 3 + 1] = cell.y; pos[i * 3 + 2] = -cell.z;
      const total = cell.market + cell.traffic;
      c.copy(traffic).lerp(market, total > 0 ? cell.market / total : 0.5);
      c.lerp(hot, cell.rising * 0.6);
      const glow = Math.min(1, Math.log10(1 + total) / 1.7);
      c.multiplyScalar(0.2 + 0.8 * glow);
      col[i * 3] = c.r; col[i * 3 + 1] = c.g; col[i * 3 + 2] = c.b;
    });
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(pos, 3));
    geo.setAttribute("color", new THREE.BufferAttribute(col, 3));
    // The sprite tapers to transparent, so the point size grows to keep
    // the visible core about what the old 14 px square showed.
    heatLayer = new THREE.Points(geo, new THREE.PointsMaterial({ size: 24, map: glowSprite(), vertexColors: true, sizeAttenuation: false, transparent: true, opacity: 0.9, depthWrite: false, depthTest: false, blending: THREE.AdditiveBlending }));
    heatLayer.raycast = () => {};
    heatLayer.renderOrder = 9;
    scene.add(heatLayer);
  }

  // Double-click / Enter: back to a known view — the focused system, the
  // route, or the whole galaxy — with the Sol-south orientation restored
  // (the idle drift and free orbiting both roll camera-up over time).
  function reframe() {
    if (!camera) return;
    camera.up.set(0, 0, -1);
    if (focus?.pos) showFocus(); else frameRoute();
  }

  // No route yet: the whole galaxy, seen from above the bubble's side.
  function frameGalaxy() {
    if (!camera) return;
    const center = toScene([0, 0, 15000]);
    const size = 60000;
    camera.position.copy(center).add(new THREE.Vector3(0, size, 0));
    camera.near = 30;
    camera.far = 400000;
    camera.updateProjectionMatrix();
    controls.target.copy(center);
    controls.update();
  }

  function frameRoute() {
    if (!route?.hops?.length) { frameGalaxy(); return; }
    if (!camera) return;
    const box = new THREE.Box3();
    route.hops.forEach((h) => box.expandByPoint(toScene(h.pos)));
    const center = box.getCenter(new THREE.Vector3());
    const size = Math.max(box.getSize(new THREE.Vector3()).length(), 60);
    // True top-down view. With camera-up set to scene -Z, coreward
    // (+galactic Z) is up: Sol sits south of the core, east on the right.
    camera.position.copy(center).add(new THREE.Vector3(0, size, 0));
    camera.near = Math.max(0.5, size / 2000);
    camera.far = 200000;
    camera.updateProjectionMatrix();
    controls.target.copy(center);
    controls.update();
  }

  function showFocus() {
    if (focusMarker) { scene?.remove(focusMarker); focusMarker.geometry?.dispose(); focusMarker.material?.dispose(); focusMarker = null; }
    if (!focus?.pos || !scene || !camera) {
      pinned = null;
      if (!route?.hops?.length) frameGalaxy();
      return;
    }
    const center = toScene(focus.pos);
    focusMarker = new THREE.Mesh(new THREE.SphereGeometry(5, 16, 16), new THREE.MeshBasicMaterial({ color: 0xffd23f, depthTest: false }));
    focusMarker.position.copy(center);
    focusMarker.renderOrder = 20;
    scene.add(focusMarker);
    pinned = { pos: focus.pos, text: focus.name };
    // Close enough to show the local stellar neighbourhood while retaining
    // enough context to orient the selected system in the galactic plane.
    camera.position.copy(center).add(new THREE.Vector3(0, 300, 0));
    camera.near = 0.1;
    camera.far = 400000;
    camera.updateProjectionMatrix();
    controls.target.copy(center);
    controls.update();
  }

  function resize() {
    if (!renderer || !host) return;
    const w = host.clientWidth, h = host.clientHeight;
    renderer.setSize(w, h, false);
    for (const m of lineMaterials) m.resolution.set(w, h);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  }

  const raycaster = new THREE.Raycaster();
  raycaster.params.Points.threshold = 0;
  // Ctrl+click on any star: name the nearest system in the index there.
  let pinned = $state(null);
  async function onClick(e) {
    // Activity mode: identification is disabled by design (the heatmap
    // must not become a tool for finding who to rob).
    if (!identify || !e.ctrlKey || !camera) return;
    const rect = host.getBoundingClientRect();
    const m = new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
    raycaster.setFromCamera(m, camera);
    const dist = camera.position.distanceTo(controls.target);
    raycaster.params.Points.threshold = dist * 0.012;
    const targets = [populated, backdrop].filter(Boolean);
    const hit = raycaster.intersectObjects(targets, false)[0];
    if (!hit) return;
    const pos = [hit.point.x, hit.point.y, -hit.point.z];
    pinned = { pos, text: "…" };
    try {
      const near = await galaxyNear(pos, 60, 12);
      if (near.length) {
        const n = near[0];
        pinned = { pos: n.pos, text: `${n.name} · ${n.class.replace("_", " ")} (${Math.round(Math.hypot(...n.pos)).toLocaleString()} ly from Sol)` };
      } else {
        pinned = { pos, text: "nothing in the index here" };
      }
    } catch { pinned = null; }
  }
  function onMove(e) {
    if (!routeGroup || !route) return;
    const rect = host.getBoundingClientRect();
    const m = new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
    raycaster.setFromCamera(m, camera);
    // Threshold in world units scaled to the view: ~8 px.
    const dist = camera.position.distanceTo(controls.target);
    raycaster.params.Points.threshold = dist * 0.012;
    const hit = raycaster.intersectObject(routeGroup.getObjectByName("hops"), false)[0];
    if (hit) {
      const h = route.hops[hit.index];
      hover = `${hit.index}: ${h.name} · ${h.class.replace("_", " ")} · ${h.distance_ly.toFixed(1)} ly${h.boosted ? " ⚡" : ""}${h.fuel_after != null ? ` · ${h.fuel_after.toFixed(0)} t` : ""}`;
    } else {
      hover = "";
    }
  }

  onMount(async () => {
    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false, powerPreference: "high-performance" });
    renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
    renderer.setClearColor(0x06080c, 1);
    host.appendChild(renderer.domElement);
    scene = new THREE.Scene();
    camera = new THREE.PerspectiveCamera(55, 1, 1, 200000);
    // Maintainer orientation ruling 2026-09-04: Sol south, coreward up — the
    // screen axes align with the galactic axes (and their StellarForge
    // axis deserts), matching the in-game galaxy map. Scene -Z is
    // galactic +Z (coreward), so it is the camera-up for top-down views.
    camera.up.set(0, 0, -1);
    controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.dampingFactor = 0.08;
    controls.zoomSpeed = 1.2;
    // Maintainer control scheme (settled 2026-09-05): left drag rotates, right
    // drag pans, scroll zooms, double-click reframes. A left+right lift
    // chord existed for one evening and was removed by the maintainer — it
    // fought OrbitControls' pointer state (breaking orbit and zoom after
    // use), and in a top-down view a vertical slide reads as zoom anyway.
    controls.mouseButtons = { LEFT: THREE.MOUSE.ROTATE, MIDDLE: THREE.MOUSE.DOLLY, RIGHT: THREE.MOUSE.PAN };
    // Give an untouched map a barely perceptible clockwise drift. Rolling
    // around the current sightline preserves the chosen target and zoom;
    // the first pan, orbit, or wheel gesture leaves the camera wholly under
    // the commander's control for the rest of this view.
    controls.addEventListener("start", () => { mapInteractionActive = true; });
    controls.addEventListener("end", () => { mapInteractionActive = false; lastMapInteraction = performance.now(); });
    resize();
    ro = new ResizeObserver(resize);
    ro.observe(host);
    // Sol marker.
    const sol = new THREE.Mesh(new THREE.SphereGeometry(6, 10, 10), new THREE.MeshBasicMaterial({ color: 0xfff1a8 }));
    scene.add(sol);
    const colonia = new THREE.Mesh(new THREE.SphereGeometry(6, 10, 10), new THREE.MeshBasicMaterial({ color: 0xfff1a8 }));
    colonia.position.copy(toScene([-9530.5, -910.28, 19808.125]));
    scene.add(colonia);
    const sgra = new THREE.Mesh(new THREE.SphereGeometry(6, 10, 10), new THREE.MeshBasicMaterial({ color: 0xfff1a8 }));
    sgra.position.copy(toScene([25.21875, -20.90625, 25899.96875]));
    scene.add(sgra);
    ready = true;
    buildRoute();
    if (focus?.pos) showFocus(); else frameRoute();
    let tick = 0, previousFrame = performance.now();
    const loop = (now = performance.now()) => {
      const dt = Math.min(0.1, (now - previousFrame) / 1000);
      previousFrame = now;
      if (idleRotate && !mapInteractionActive && now - lastMapInteraction >= 5000 && camera && controls) {
        const sightline = controls.target.clone().sub(camera.position).normalize();
        // One revolution in roughly 20 minutes: visible when watched, never
        // distracting. Negative screen-axis rotation reads clockwise.
        camera.up.applyAxisAngle(sightline, -Math.PI * 2 * dt / 1200).normalize();
      }
      controls.update();
      renderer.render(scene, camera);
      if (++tick % 2 === 0) updateLabels();
      raf = requestAnimationFrame(loop);
    };
    loop();
    try {
      [backdrop, populated] = await Promise.all([
        loadStars("/galaxy-stars.bin", 1.2, 0.10, 0, 0.6, { hidePopulated: true }),
        loadStars("/galaxy-populated.bin", 1.4, 0.12, 0.52, 0.3, { populatedLayer: true }),
      ]);
      scene.add(backdrop);
      scene.add(populated);
      applyStarBrightness();
    } catch (e) {
      console.warn("galaxy backdrop not loaded", e);
    }
  });

  onDestroy(() => {
    ro?.disconnect();
    cancelAnimationFrame(raf);
    controls?.dispose();
    renderer?.dispose();
    scene?.traverse((o) => { o.geometry?.dispose?.(); o.material?.dispose?.(); });
  });

  // Rebuild when the route or the follow cursor changes.
  $effect(() => { route; nextIndex; candidates; if (ready) { buildRoute(); } });
  $effect(() => { heat; if (ready) { rebuildHeat(); } });
  let focusedFor = "";
  $effect(() => {
    const key = focus?.pos ? `${focus.name}:${focus.pos.join(",")}` : "";
    if (ready && key !== focusedFor) { focusedFor = key; showFocus(); }
  });
  // Re-frame only when the trip changes (ends move), not for every better
  // candidate of the same trip: that reset the camera under the user.
  let framedFor = "";
  $effect(() => {
    const ends = route?.hops?.length ? `${route.hops[0].name}>${route.hops[route.hops.length - 1].name}` : "";
    if (ready && ends !== framedFor) { framedFor = ends; frameRoute(); }
  });
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="galaxy" bind:this={host} style="height:{height}px" onmousemove={onMove} ondblclick={reframe} onclick={onClick} onkeydown={(e) => { if (e.key === "Enter") reframe(); }} role="application" tabindex="0" aria-label="Interactive 3D galaxy map with the plotted route; press Enter to frame the route">
  {#each labels as l}<div class="label {l.kind}" style="left:{l.x}px; top:{l.y}px">{l.text}</div>{/each}
  {#if hover}<div class="hover">{hover}</div>{/if}
  <div class="hint">drag to orbit · right-drag to pan · scroll to zoom · double-click to re-frame · Ctrl+click to name a star</div>
</div>

<style>
  .galaxy { position: relative; width: 100%; background: #06080c; border: 1px solid var(--line); border-radius: 5px; overflow: hidden; }
  .galaxy :global(canvas) { display: block; width: 100%; height: 100%; }
  .hover { position: absolute; top: 0.5rem; left: 0.6rem; font-size: 0.8rem; background: rgba(6, 8, 12, 0.8); padding: 0.2rem 0.5rem; border-radius: 4px; pointer-events: none; }
  .label { position: absolute; transform: translate(10px, -50%); font-size: 0.8rem; font-weight: 600; color: #e4e6ea; text-shadow: 0 0 4px #000, 0 0 8px #000; pointer-events: none; white-space: nowrap; }
  .label.start { color: #ffffff; }
  .label.end { color: #ff8c1a; }
  .label.next { color: #39d98a; }
  .label.sol { color: #fff1a8; font-weight: 400; font-size: 0.72rem; }
  .label.pin { color: #ffd23f; }
  .hint { position: absolute; bottom: 0.4rem; right: 0.6rem; font-size: 0.72rem; color: var(--muted); pointer-events: none; }
</style>
