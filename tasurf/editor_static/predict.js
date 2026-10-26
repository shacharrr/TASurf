// The instant prediction: where edited inputs take the run, worked out here in the editor within a frame or
// two of the edit, while the game's exact preview is on its way (as Bunnymod XT's TAS editor does: predict
// at once, then replace the prediction with the exact path).
//
// It finds the first tick whose inputs differ from the newest exact path (the preview or the last run),
// starts from that path's exact state the tick before, and plays the new inputs with a copy of Source's
// player movement (CGameMovement: friction, ground and air acceleration, gravity, jumping, sliding along what
// the player hits, steps, standing on the ground) against the map's own brushes (/api/mapclip, the planes the
// game's collision uses), with the map's teleports and pushes. It works a few milliseconds a frame, nearest
// ticks first, so a drag shows its effect at once and the rest follows.
//
// What it can't see: displacements, props, moving brushes, map logic (outputs, filters, speed and gravity
// changes), and the game's float32 rounding. So it drifts the further it goes; the game's
// preview replaces it.
//
// It reads the page's state (S) and calls valueAt, render and later-style scheduling of its own.
"use strict";
(() => {
  // PARALLEL: a slide along a surface the box is already closer to than DIST_EPSILON comes out a hair into it in
  // double precision, which the game (in float32) doesn't stop on; less than this into it counts as along it
  const PARALLEL = 1e-4;
  const DIST_EPSILON = 0.03125, NON_JUMP_VELOCITY = 140, MAX_CLIP_PLANES = 5, DUCK_MUL = 0.33333334, DEG = 180 / Math.PI;
  const JUMP_SPEED = Math.sqrt(2 * 800 * 57);  // CS:S: a jump reaches 57 units at 800 gravity
  // CS:S stamina: a jump costs 25 of 100, recovering 19 a second; while any is left, jumps are lower and ground speed drops
  const STAMINA_JUMP = 25 / 19 * 1000, staminaRatio = ms => (100 - ms / 1000 * 19) / 100;
  const HULL = { mins: [-16, -16, 0], maxs: [16, 16, 72] }, DUCKED = { mins: [-16, -16, 0], maxs: [16, 16, 54] };
  // MASK_PLAYERSOLID: solid, window, grate, moveable, playerclip, monster
  const PLAYER_SOLID = 0x1 | 0x2 | 0x8 | 0x4000 | 0x10000 | 0x2000000;
  const IN = { attack: 1, jump: 2, duck: 4, forward: 8, back: 16, use: 32, turnleft: 128, turnright: 256,
    moveleft: 512, moveright: 1024, attack2: 2048, reload: 8192, walk: 131072 };
  const SIGN = { forward: [1, 0], back: [-1, 0], moveleft: [0, -1], moveright: [0, 1] };
  const DEFAULT_MOVE = { dt: 0.015, gravity: 800, airaccelerate: 150, accelerate: 5, friction: 4, stopspeed: 75,
    maxspeed: 250, maxvelocity: 3500, stepsize: 18, bounce: 0, bunnyhop: 1 };  // bunnyhop: sv_enablebunnyhopping
  const ZONES = new Set(["func_buyzone", "func_bomb_target", "func_hostage_rescue", "func_vip_safetyzone", "func_escapezone"]);
  // brush entities that never block players
  const NOT_SOLID = new Set(["func_illusionary", "func_areaportal", "func_areaportalwindow", "func_occluder", "func_dustmotes",
    "func_dustcloud", "func_smokevolume", "func_precipitation", "func_clip_vphysics", "func_ladder", "func_vehicleclip", "func_lod"]);

  const W = { name: null, ready: false, loading: false, error: null };  // the map: what the player collides with
  const J = { key: null, job: null, wait: null };                         // the prediction being worked out
  let R = null;  // its result so far

  // ---------------------------------------------------------------- the map
  function entities(text) {
    const out = [];
    let ent = null, key = null;
    for (const m of text.matchAll(/"([^"]*)"|([{}])/g)) {
      if (m[2] === "{") { ent = {}; key = null; }
      else if (m[2] === "}") { if (ent) out.push(ent); ent = null; key = null; }
      else if (ent) { if (key === null) key = m[1]; else { ent[key] = m[1]; key = null; } }
    }
    return out;
  }
  const vec3 = (s, d = [0, 0, 0]) => { const v = String(s || "").trim().split(/\s+/).map(Number); return v.length === 3 && v.every(isFinite) ? v : d; };
  function solidEntity(e) {
    const cls = e.classname || "";
    if (cls.startsWith("trigger_") || ZONES.has(cls) || NOT_SOLID.has(cls)) return false;
    if (cls === "func_brush" && (e.Solidity === "1" || (e.Solidity !== "2" && e.StartDisabled === "1"))) return false;  // never solid, or off
    if (cls === "func_wall_toggle" && (+e.spawnflags & 1)) return false;  // starts hidden
    return cls.startsWith("func_") || cls.startsWith("momentary_");
  }
  // what a trigger does to a player touching it (null: nothing, or nothing we can tell)
  function triggerOf(e, named) {
    const flags = +e.spawnflags || 0;
    if (e.StartDisabled === "1" || !(flags & 1)) return null;  // off, or not for players
    if (e.classname === "trigger_teleport") {
      const dest = named.get(e.target || "");
      if (!dest) return null;
      const mark = e.landmark ? named.get(e.landmark) : null;
      return { kind: "teleport", to: vec3(dest.origin), landmark: mark ? vec3(mark.origin) : null };  // the game keeps your velocity
    }
    const a = vec3(e.pushdir), speed = +e.speed || 0, p = a[0] / DEG, y = a[1] / DEG;  // trigger_push: pushdir is angles
    const v = [Math.cos(p) * Math.cos(y) * speed, Math.cos(p) * Math.sin(y) * speed, -Math.sin(p) * speed];
    return { kind: "push", vel: v, once: !!(flags & 128) };
  }
  async function load(name) {
    Object.assign(W, { name, ready: false, loading: true, error: null });
    try {
      const r = await fetch("/api/mapclip?name=" + encodeURIComponent(name));
      if (!r.ok) {
        let msg = `HTTP ${r.status}`;
        try { msg = (await r.json()).error || msg; } catch (e) { /* not JSON */ }
        throw new Error(msg);
      }
      const buf = await r.arrayBuffer();
      if (W.name !== name) return;
      build(buf);
      W.ready = true; W.gen = (W.gen || 0) + 1;  // a new world: what was worked out with the old one is stale
    } catch (e) {
      if (W.name === name) W.error = String(e.message || e);
    }
    if (W.name === name) { W.loading = false; J.key = null; render(); }
  }
  function build(buf) {
    const dv = new DataView(buf), latin = new TextDecoder("latin1");
    if (buf.byteLength < 20 || latin.decode(new Uint8Array(buf, 0, 4)) !== "TACL") throw new Error("not the map's collision data");
    const nb = dv.getUint32(8, true), np = dv.getUint32(12, true), nt = dv.getUint32(16, true);
    const table = new Uint32Array(buf.slice(20, 20 + nb * 16)), src = new Float32Array(buf.slice(20 + nb * 16, 20 + nb * 16 + np * 16));
    const ents = entities(latin.decode(new Uint8Array(buf, 20 + nb * 16 + np * 16, nt)));
    const byModel = new Map(), named = new Map();
    for (const e of ents) {
      const m = /^\*(\d+)$/.exec(e.model || "");
      if (m) byModel.set(+m[1], e);
      if (e.targetname && !named.has(e.targetname)) named.set(e.targetname, e);
    }
    const planes = [], solids = [], triggers = [];
    for (let b = 0; b < nb; b++) {
      const first = table[4 * b], count = table[4 * b + 1], contents = table[4 * b + 2], model = table[4 * b + 3];
      const e = model ? byModel.get(model) : null;
      let trig = null;
      if (model) {
        if (!e) continue;
        if (e.classname === "trigger_teleport" || e.classname === "trigger_push") { trig = triggerOf(e, named); if (!trig) continue; }
        else if (!solidEntity(e) || !(contents & PLAYER_SOLID)) continue;
      } else if (!(contents & PLAYER_SOLID)) continue;
      const o = e ? vec3(e.origin) : [0, 0, 0];  // a brush entity's brushes are relative to its origin
      const brush = { first: planes.length / 4, count, lo: [-Infinity, -Infinity, -Infinity], hi: [Infinity, Infinity, Infinity], trig };
      for (let i = 0; i < count; i++) {
        const j = 4 * (first + i), n = [src[j], src[j + 1], src[j + 2]], d = src[j + 3] + n[0] * o[0] + n[1] * o[1] + n[2] * o[2];
        planes.push(n[0], n[1], n[2], d);
        for (let a = 0; a < 3; a++) {  // its box, from its axial sides (the compiler gives every brush all six)
          if (n[(a + 1) % 3] !== 0 || n[(a + 2) % 3] !== 0) continue;
          if (n[a] === 1) brush.hi[a] = Math.min(brush.hi[a], d); else if (n[a] === -1) brush.lo[a] = Math.max(brush.lo[a], -d);
        }
      }
      (trig ? triggers : solids).push(brush);
    }
    W.planes = Float64Array.from(planes); W.solids = solids; W.triggers = triggers;
    grid();
  }
  // the solid brushes by 256-unit columns, so a move only looks at the brushes near it
  const CELL = 256;
  const cellKey = (x, y) => (x + 8192) * 16384 + (y + 8192);
  function grid() {
    const cells = new Map(), big = [];
    W.solids.forEach((b, i) => {
      if (![0, 1].every(a => isFinite(b.lo[a]) && isFinite(b.hi[a]))) { big.push(i); return; }
      const x0 = Math.floor(b.lo[0] / CELL), x1 = Math.floor(b.hi[0] / CELL), y0 = Math.floor(b.lo[1] / CELL), y1 = Math.floor(b.hi[1] / CELL);
      if ((x1 - x0 + 1) * (y1 - y0 + 1) > 4096) { big.push(i); return; }
      for (let x = x0; x <= x1; x++) for (let y = y0; y <= y1; y++) {
        const k = cellKey(x, y); let c = cells.get(k);
        if (!c) cells.set(k, c = []);
        c.push(i);
      }
    });
    W.cells = cells; W.big = big; W.stamp = new Uint32Array(W.solids.length); W.mark = 0;
  }
  function near(lo, hi) {  // the solid brushes in the columns the box [lo, hi] reaches
    if (++W.mark >= 0xFFFFFFFF) { W.stamp.fill(0); W.mark = 1; }
    const m = W.mark, out = W.big.slice();
    const x0 = Math.floor(lo[0] / CELL), x1 = Math.floor(hi[0] / CELL), y0 = Math.floor(lo[1] / CELL), y1 = Math.floor(hi[1] / CELL);
    for (let x = x0; x <= x1; x++) for (let y = y0; y <= y1; y++) {
      const c = W.cells.get(cellKey(x, y)); if (!c) continue;
      for (const i of c) if (W.stamp[i] !== m) { W.stamp[i] = m; out.push(i); }
    }
    return out;
  }

  // ---------------------------------------------------------------- collision
  // The player's box (mins, maxs about its origin) moved from s to e through the solid brushes: how far it gets
  // (fraction), where it ends (end), and the plane it stopped against (normal), as the game's box trace does it.
  function trace(s, e, h) {
    const tr = { fraction: 1, normal: null, startsolid: false, allsolid: false, end: null };
    const lo = [0, 1, 2].map(i => Math.min(s[i], e[i]) + h.mins[i] - 1), hi = [0, 1, 2].map(i => Math.max(s[i], e[i]) + h.maxs[i] + 1);
    for (const i of near(lo, hi)) {
      const b = W.solids[i];
      if (b.lo[0] > hi[0] || b.hi[0] < lo[0] || b.lo[1] > hi[1] || b.hi[1] < lo[1] || b.lo[2] > hi[2] || b.hi[2] < lo[2]) continue;
      clipToBrush(b, s, e, h, tr);
      if (tr.allsolid) break;
    }
    const f = tr.fraction;
    tr.end = f === 1 ? e.slice() : [s[0] + f * (e[0] - s[0]), s[1] + f * (e[1] - s[1]), s[2] + f * (e[2] - s[2])];
    return tr;
  }
  function clipToBrush(b, s, e, h, tr) {  // CM_ClipBoxToBrush: each side moved out by the box
    const P = W.planes, mn = h.mins, mx = h.maxs;
    let enter = -1, leave = 1, at = -1, getout = false, startout = false;
    for (let k = b.first, last = b.first + b.count; k < last; k++) {
      const o = 4 * k, nx = P[o], ny = P[o + 1], nz = P[o + 2];
      const dist = P[o + 3] - ((nx < 0 ? mx[0] : mn[0]) * nx + (ny < 0 ? mx[1] : mn[1]) * ny + (nz < 0 ? mx[2] : mn[2]) * nz);
      const d1 = s[0] * nx + s[1] * ny + s[2] * nz - dist, d2 = e[0] * nx + e[1] * ny + e[2] * nz - dist;
      if (d2 > 0) getout = true;
      if (d1 > 0) startout = true;
      if (d1 > 0 && (d2 >= DIST_EPSILON || d2 >= d1 - PARALLEL)) return;  // in front of this side all the way: misses the brush
      if (d1 <= 0 && d2 <= 0) continue;  // behind it all the way
      if (d1 > d2) { const f = Math.max(0, (d1 - DIST_EPSILON) / (d1 - d2)); if (f > enter) { enter = f; at = o; } }
      else { const f = Math.min(1, (d1 + DIST_EPSILON) / (d1 - d2)); if (f < leave) leave = f; }
    }
    if (!startout) { tr.startsolid = true; if (!getout) { tr.allsolid = true; tr.fraction = 0; } return; }
    if (enter < leave && enter > -1 && enter < tr.fraction) { tr.fraction = Math.max(0, enter); tr.normal = [P[at], P[at + 1], P[at + 2]]; }
  }
  function inside(b, pos, h) {  // does the box at pos overlap the brush
    const P = W.planes, mn = h.mins, mx = h.maxs;
    for (let k = b.first, last = b.first + b.count; k < last; k++) {
      const o = 4 * k, nx = P[o], ny = P[o + 1], nz = P[o + 2];
      const dist = P[o + 3] - ((nx < 0 ? mx[0] : mn[0]) * nx + (ny < 0 ? mx[1] : mn[1]) * ny + (nz < 0 ? mx[2] : mn[2]) * nz);
      if (pos[0] * nx + pos[1] * ny + pos[2] * nz - dist > 0) return false;
    }
    return true;
  }

  // ---------------------------------------------------------------- movement (CGameMovement)
  const hull = st => st.ducked ? DUCKED : HULL;
  const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  function clampVelocity(v, M) { for (let i = 0; i < 3; i++) v[i] = Math.max(-M.maxvelocity, Math.min(M.maxvelocity, v[i])); }
  // wish direction (horizontal, unit) and speed from the view yaw and the keys (CheckParameters, then the move)
  function wish(yaw, fmove, smove, maxspeed) {
    const s2 = fmove * fmove + smove * smove;
    if (s2 > maxspeed * maxspeed && s2 > 0) { const r = maxspeed / Math.sqrt(s2); fmove *= r; smove *= r; }
    const y = yaw / DEG, wx = Math.cos(y) * fmove + Math.sin(y) * smove, wy = Math.sin(y) * fmove - Math.cos(y) * smove;
    const speed = Math.hypot(wx, wy);
    return speed === 0 ? [0, 0, 0] : [wx / speed, wy / speed, Math.min(speed, maxspeed)];
  }
  function clipVelocity(v, n, overbounce) {
    const back = dot(v, n) * overbounce, out = [v[0] - n[0] * back, v[1] - n[1] * back, v[2] - n[2] * back];
    const adjust = dot(out, n);
    if (adjust < 0) for (let i = 0; i < 3; i++) out[i] -= n[i] * adjust;
    return out;
  }
  // TryPlayerMove: move for the tick, sliding along whatever the box hits (up to four times). What that costs
  // in speed adds to st.lost, unless all it touched was floor (a landing keeps the speed along the ground):
  // walls, ramps, their edges and ceilings count. Riding a ramp costs next to nothing, as the part into the
  // ramp is only gravity's.
  function slide(st, M) {
    let time = M.dt, all = 0, original = st.vel.slice(), wall = false;
    const primal = st.vel.slice(), planes = [];
    for (let bump = 0; bump < 4; bump++) {
      if (!st.vel[0] && !st.vel[1] && !st.vel[2]) break;
      const end = [st.pos[0] + st.vel[0] * time, st.pos[1] + st.vel[1] * time, st.pos[2] + st.vel[2] * time];
      const t = trace(st.pos, end, hull(st));
      all += t.fraction;
      if (t.allsolid) { st.vel = [0, 0, 0]; wall = true; break; }
      if (t.fraction > 0) {
        if (t.fraction === 1) {  // the game checks the end isn't stuck in something
          const stuck = trace(t.end, t.end, hull(st));
          if (stuck.startsolid || stuck.fraction !== 1) { st.vel = [0, 0, 0]; wall = true; break; }
        }
        st.pos = t.end; original = st.vel.slice(); planes.length = 0;
      }
      if (t.fraction === 1) break;
      time -= time * t.fraction;
      if (planes.length >= MAX_CLIP_PLANES) { st.vel = [0, 0, 0]; wall = true; break; }
      if (t.normal[2] < 0.7) wall = true;
      planes.push(t.normal);
      if (planes.length === 1 && !st.ground) {  // in the air against one surface: along it (off steep ones by sv_bounce)
        const n = planes[0];
        st.vel = clipVelocity(original, n, n[2] > 0.7 ? 1 : 1 + M.bounce * (1 - st.friction));
        original = st.vel.slice();
      } else {
        let i;
        for (i = 0; i < planes.length; i++) {
          st.vel = clipVelocity(original, planes[i], 1);
          let j;
          for (j = 0; j < planes.length; j++) if (j !== i && dot(st.vel, planes[j]) < 0) break;
          if (j === planes.length) break;
        }
        if (i === planes.length) {  // into a crease: along it, or stop
          if (planes.length !== 2) { st.vel = [0, 0, 0]; break; }
          const a = planes[0], b = planes[1];
          let d = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
          const dl = Math.hypot(...d); d = dl ? d.map(c => c / dl) : d;
          const s = dot(d, st.vel); st.vel = d.map(c => c * s);
        }
        if (dot(st.vel, primal) <= 0) { st.vel = [0, 0, 0]; break; }
      }
    }
    if (all === 0) { st.vel = [0, 0, 0]; wall = true; }
    if (wall) st.lost += Math.max(0, Math.hypot(...primal) - Math.hypot(...st.vel));
  }
  function friction(st, M) {
    const v = st.vel, speed = Math.hypot(v[0], v[1], v[2]);
    if (speed < 0.1) return;
    const drop = st.ground ? Math.max(speed, M.stopspeed) * M.friction * st.friction * M.dt : 0;
    const k = Math.max(0, speed - drop) / speed;
    if (k !== 1) for (let i = 0; i < 3; i++) v[i] *= k;
  }
  function stayOnGround(st, M) {  // down a step or a slope with the ground
    let t = trace(st.pos, [st.pos[0], st.pos[1], st.pos[2] + 2], hull(st));
    t = trace(t.end, [st.pos[0], st.pos[1], st.pos[2] - M.stepsize], hull(st));
    if (t.fraction > 0 && t.fraction < 1 && !t.startsolid && t.normal && t.normal[2] >= 0.7 && Math.abs(st.pos[2] - t.end[2]) > 0.5 / 32) st.pos = t.end;
  }
  function stepMove(st, M) {  // the plain slide, or up a step, slide, back down: whichever goes further
    const pos0 = st.pos.slice(), vel0 = st.vel.slice(), lost0 = st.lost;
    slide(st, M);
    const downPos = st.pos, downVel = st.vel, downLost = st.lost;
    st.pos = pos0.slice(); st.vel = vel0.slice(); st.lost = lost0;
    let t = trace(st.pos, [st.pos[0], st.pos[1], st.pos[2] + M.stepsize + DIST_EPSILON], hull(st));
    if (!t.startsolid && !t.allsolid) st.pos = t.end;
    slide(st, M);
    t = trace(st.pos, [st.pos[0], st.pos[1], st.pos[2] - M.stepsize - DIST_EPSILON], hull(st));
    if (!t.normal || t.normal[2] < 0.7) { st.pos = downPos; st.vel = downVel; st.lost = downLost; return; }
    if (!t.startsolid && !t.allsolid) st.pos = t.end;
    const dd = (downPos[0] - pos0[0]) ** 2 + (downPos[1] - pos0[1]) ** 2, du = (st.pos[0] - pos0[0]) ** 2 + (st.pos[1] - pos0[1]) ** 2;
    if (dd > du) { st.pos = downPos; st.vel = downVel; st.lost = downLost; } else st.vel[2] = downVel[2];
  }
  function walkMove(st, M, yaw, fmove, smove) {
    if (st.stamina > 0) {  // CS:S: tired, the ground slows you (as Goldsrc did at 70 fps)
      const k = Math.pow(staminaRatio(st.stamina), M.dt * 70);
      st.vel[0] *= k; st.vel[1] *= k;
    }
    const [wx, wy, w0] = wish(yaw, fmove, smove, M.maxspeed), ws = st.crop ? w0 * DUCK_MUL : w0, v = st.vel;
    v[2] = 0;
    const add = ws - (v[0] * wx + v[1] * wy);  // Accelerate
    if (add > 0) { const a = Math.min(M.accelerate * M.dt * ws * st.friction, add); v[0] += a * wx; v[1] += a * wy; }
    v[2] = 0;
    for (let i = 0; i < 3; i++) v[i] += st.base[i];
    if (Math.hypot(v[0], v[1], v[2]) < 1) { st.vel = [0, 0, 0]; for (let i = 0; i < 3; i++) st.vel[i] -= st.base[i]; return; }
    const dest = [st.pos[0] + v[0] * M.dt, st.pos[1] + v[1] * M.dt, st.pos[2]];
    const t = trace(st.pos, dest, hull(st));
    if (t.fraction === 1) st.pos = t.end; else stepMove(st, M);
    for (let i = 0; i < 3; i++) st.vel[i] -= st.base[i];
    stayOnGround(st, M);
  }
  function airMove(st, M, yaw, fmove, smove) {
    const [wx, wy, ws] = wish(yaw, fmove, smove, M.maxspeed), v = st.vel;
    const add = Math.min(ws, 30) - (v[0] * wx + v[1] * wy);  // AirAccelerate: the add-speed capped at 30
    if (add > 0) { const a = Math.min(M.airaccelerate * ws * M.dt * st.friction, add); v[0] += a * wx; v[1] += a * wy; }
    for (let i = 0; i < 3; i++) v[i] += st.base[i];
    slide(st, M);
    for (let i = 0; i < 3; i++) st.vel[i] -= st.base[i];
  }
  function categorize(st) {  // on the ground, or in the air (a surf ramp is too steep to stand on)
    if (st.vel[2] > NON_JUMP_VELOCITY) { st.ground = false; return; }
    const t = trace(st.pos, [st.pos[0], st.pos[1], st.pos[2] - 2], hull(st));
    if (t.fraction === 1 || !t.normal || t.normal[2] < 0.7) {
      st.ground = false;
      if (st.vel[2] > 0) st.friction = 0.25;
    } else {
      st.ground = true; st.vel[2] = 0; st.friction = 1;
    }
  }
  // CS:S ducking, as the runs show it: in the air at once, the feet coming up 8.5 units (and down again to stand,
  // when there's room); on the ground after 0.4 s held, and up again 0.2 s after letting go. On the ground the
  // keys move you at a third of the speed while ducked, ducking or holding duck (judged before this tick's change).
  const AIR_DUCK = 8.5;
  function duck(st, M, buttons) {
    const want = !!(buttons & IN.duck), pressed = want && !(st.old & IN.duck), released = !want && (st.old & IN.duck);
    st.crop = st.ground && (st.ducked || !!st.trans || want);
    if (st.trans) st.elapsed += 1000 * M.dt;
    const room = p => !trace(p, p, HULL).startsolid;
    if (want && !st.ducked) {
      if (!st.ground) { st.pos[2] += AIR_DUCK; st.ducked = true; st.trans = null; }
      else if (pressed || st.trans !== "down") { st.trans = "down"; st.elapsed = 0; }
      else if (st.elapsed > 400) { st.ducked = true; st.trans = null; }
    } else if (!want && st.ducked) {
      if (!st.ground) {
        const to = [st.pos[0], st.pos[1], st.pos[2] - AIR_DUCK];
        if (room(to)) { st.pos = to; st.ducked = false; }
        st.trans = null;
      } else if (released || st.trans !== "up") { st.trans = "up"; st.elapsed = 0; }
      else if (st.elapsed > 200 && room(st.pos)) { st.ducked = false; st.trans = null; }
    } else st.trans = null;
  }
  function jump(st, M) {
    if (!st.ground || (st.old & IN.jump)) return;  // in the air, or held since the last tick (no auto-jump)
    if (!M.bunnyhop) {
      // sv_enablebunnyhopping 0: the jump cuts the speed to 1.1 x maxspeed. As runs show it: the horizontal velocity is
      // scaled so the speed with StartGravity's vertical (3D) is that; the vertical isn't scaled.
      const cap = Math.fround(1.1) * M.maxspeed, s = Math.hypot(st.vel[0], st.vel[1], st.vel[2]);
      if (cap > 0 && s > cap) { st.vel[0] *= cap / s; st.vel[1] *= cap / s; }
    }
    st.ground = false;
    if (st.ducked) st.vel[2] = JUMP_SPEED; else st.vel[2] += JUMP_SPEED;
    if (st.stamina > 0) st.vel[2] *= staminaRatio(st.stamina);  // tired: a lower jump
    st.stamina = STAMINA_JUMP;
    st.vel[2] -= M.gravity * 0.5 * M.dt;  // FinishGravity
    clampVelocity(st.vel, M);
  }
  // one tick of FullWalkMove, then the triggers the player touches (st.lost: the speed hitting things cost it)
  function tick(st, M, yaw, fmove, smove, buttons) {
    st.lost = 0;
    if (st.stamina > 0) st.stamina = Math.max(0, st.stamina - 1000 * M.dt);  // ReduceTimers
    duck(st, M, buttons);
    st.vel[2] -= M.gravity * 0.5 * M.dt;  // StartGravity
    st.vel[2] += st.base[2] * M.dt; st.base[2] = 0;
    clampVelocity(st.vel, M);
    if (buttons & IN.jump) jump(st, M);
    if (st.ground) { st.vel[2] = 0; friction(st, M); }
    clampVelocity(st.vel, M);
    if (st.ground) walkMove(st, M, yaw, fmove, smove); else airMove(st, M, yaw, fmove, smove);
    categorize(st);
    clampVelocity(st.vel, M);
    st.vel[2] -= M.gravity * 0.5 * M.dt;  // FinishGravity
    clampVelocity(st.vel, M);
    if (st.ground) st.vel[2] = 0;
    st.old = buttons;
    return touch(st);
  }
  function touch(st) {
    const h = hull(st), base = [0, 0, 0];
    let pushed = false;
    for (const b of W.triggers) {
      if (b.lo[0] > st.pos[0] + h.maxs[0] || b.hi[0] < st.pos[0] + h.mins[0] || b.lo[1] > st.pos[1] + h.maxs[1] ||
        b.hi[1] < st.pos[1] + h.mins[1] || b.lo[2] > st.pos[2] + h.maxs[2] || b.hi[2] < st.pos[2] + h.mins[2] || !inside(b, st.pos, h)) continue;
      const t = b.trig;
      if (t.kind === "teleport") {  // to the destination (or as far from it as from the landmark), keeping the speed
        st.pos = t.landmark ? st.pos.map((c, i) => t.to[i] + c - t.landmark[i]) : t.to.slice();
        st.ground = false; st.base = [0, 0, 0];
        return "teleport";
      }
      if (t.once) { for (let i = 0; i < 3; i++) st.vel[i] += t.vel[i]; continue; }
      for (let i = 0; i < 3; i++) base[i] += t.vel[i];
      if (t.vel[2] > 0 && st.ground) { st.ground = false; st.pos[2] += 1; }
      pushed = true;
    }
    st.base = pushed ? base : [0, 0, 0];
    return pushed ? "push" : null;
  }
  // a straight auto-strafe tick's sidemove (the plugin's AutoStrafe_Side: physics.py's straight_sidemove): A
  // (negative) when the heading is to the left of the velocity, D otherwise; the held keys without a side key or speed.
  // With a swing (degrees; NaN: perfect), the side played the tick before (prev) is kept until the direction is that
  // far past the heading.
  function straightSide(st, smove, heading, swing, prev) {
    if (!smove || Math.hypot(st.vel[0], st.vel[1]) < 1) return smove;
    const err = ((heading - Math.atan2(st.vel[1], st.vel[0]) * DEG + 180) % 360 + 360) % 360 - 180, size = Math.abs(smove);
    if (!isNaN(swing) && prev) return prev < 0 ? (err < -swing ? size : -size) : (err > swing ? -size : size);
    return err > 0 ? -size : size;
  }
  // A smooth-swinging straight tick in the air (the plugin's AutoStrafe_Smooth: physics.py's smooth_swing_input):
  // [forwardmove, sidemove, yaw], or null (on the ground, without a side key or speed: a straight tick). The turn rate
  // changes by at most a step a tick, a sine's at close 0 and two whole turns at close 1; it speeds up, holds, or brakes
  // in time to stop at the edge of the sweep. prevDir: where it was going as the tick before started (null: unknown).
  function smoothSwing(st, smove, heading, swing, close, prevDir, prevSide) {
    const size = Math.abs(smove), v = Math.hypot(st.vel[0], st.vel[1]);
    if (!size || v < 1 || st.ground) return null;
    const wrap = a => ((a + 180) % 360 + 360) % 360 - 180;
    const dir = Math.atan2(st.vel[1], st.vel[0]) * DEG, d = wrap(dir - heading), most = Math.atan(30 / v) * DEG;
    const sine = Math.min(most * most / swing, 2 * most), step = sine + close * (2 * most - sine);
    const way = prevSide > 0 ? -1 : prevSide < 0 ? 1 : d < 0 ? 1 : -1;  // 1: turning left (A)
    const u = prevDir === null ? 0 : way * wrap(dir - prevDir), ahead = swing - way * d;
    const stop = w => { const n = w > 0 ? Math.floor(w / step) : 0; return n * w - step * n * (n + 1) / 2; };
    const faster = Math.min(most, u + step);
    const w = ahead >= faster + stop(faster) ? faster : u >= step && ahead >= u + stop(u) ? u : u - step;
    const turn = way * Math.max(-most, Math.min(most, w));
    const cosT = Math.max(0, Math.min(1, (30 - v * Math.tan(Math.abs(turn) / DEG)) / v));
    return [size * cosT, (turn > 0 ? -size : size) * Math.sqrt(1 - cosT * cosT), dir];
  }
  // the yaw an auto-strafe tick plays (the plugin's AutoStrafe_Yaw: physics.py's strafe_yaw), or null
  function strafeYaw(st, M, fmove, smove, offset, turn, buttons) {
    if (!fmove && !smove) return null;
    let speed = Math.hypot(st.vel[0], st.vel[1]);
    if (speed < 1) return null;
    let ws = wish(0, fmove, smove, M.maxspeed)[2], cstar;
    if (st.ground && !((buttons & IN.jump) && !(st.old & IN.jump))) {  // a tick that jumps counts as air
      if (st.ducked) ws *= DUCK_MUL;
      speed -= Math.max(speed, M.stopspeed) * M.friction * M.dt;
      if (speed < 1) return null;
      cstar = Math.max(ws - M.accelerate * ws * M.dt, 0);
    } else cstar = Math.max(30 - M.airaccelerate * ws * M.dt, 0);
    const theta = Math.acos(Math.min(1, cstar / speed)) * DEG + offset, alpha = Math.atan2(-smove, fmove) * DEG;
    let side;
    if (alpha > 0.5 && alpha < 179.5) side = 1;
    else if (alpha < -0.5 && alpha > -179.5) side = -1;
    else if (turn !== 0) side = turn > 0 ? 1 : -1;
    else if (theta === 0) side = 1;
    else return null;
    return Math.atan2(st.vel[1], st.vel[0]) * DEG + side * theta - alpha;
  }

  // ---------------------------------------------------------------- the inputs and where to start
  // this timeline's inputs per tick, as the baker makes them (yaw unwrapped; strafe: the auto-strafe offset or NaN;
  // heading: a straight range's, or NaN)
  function inputsNow() {
    const n = S.norm.length, yaw = new Float64Array(n), fwd = new Float64Array(n), side = new Float64Array(n);
    const buttons = new Int32Array(n), strafe = new Float64Array(n).fill(NaN), heading = new Float64Array(n).fill(NaN);
    const swing = new Float64Array(n).fill(NaN), smooth = new Float64Array(n).fill(NaN);
    for (let t = 0; t < n; t++) yaw[t] = valueAt(S.norm.yaw, t);
    for (const r of S.norm.keys) {
      const bit = IN[r.key] !== undefined ? IN[r.key] : /^bit\d+$/.test(r.key) ? 2 ** +r.key.slice(3) | 0 : 0;
      const sg = SIGN[r.key] || [0, 0], mag = r.value !== undefined && r.value !== null ? +r.value : S.norm.move_speed;
      for (let t = Math.max(0, r.from); t < Math.min(n, r.to); t++) { buttons[t] |= bit; fwd[t] += sg[0] * mag; side[t] += sg[1] * mag; }
    }
    for (const r of S.norm.strafe || []) {
      for (let t = Math.max(0, r.from); t < Math.min(n, r.to); t++) {
        strafe[t] = +r.offset || 0;
        if (r.heading !== undefined && r.heading !== null) heading[t] = +r.heading;
        if (r.swing !== undefined && r.swing !== null) swing[t] = +r.swing;
        // a swing's gain: how close to perfect it turns, 0 to 1 (physics.py's swing_close)
        if (r.gain !== undefined && r.gain !== null) smooth[t] = 1 - 2 * Math.sqrt(Math.max(0, 1 - r.gain / 100));
      }
    }
    return { n, yaw, fwd, side, buttons, strafe, heading, swing, smooth, autobhop: !!S.norm.autobhop };
  }
  // the first tick whose inputs differ from what the path played (auto-strafe ticks: the game chose the yaw; straight
  // ones also A or D)
  function firstChange(r, inp) {
    const n = Math.min(r.x.length, inp.n), AD = IN.moveleft | IN.moveright;
    const startAir = !!(S.doc && S.doc.start && S.doc.start.on_ground === false);
    const WASD = AD | IN.forward | IN.back;
    for (let t = 0; t < n; t++) {
      const straight = !isNaN(inp.heading[t]);
      const smooth = !isNaN(inp.smooth[t]) && t > 0 && !r.on_ground[t - 1] && Math.abs(inp.side[t]) > 0;
      const side = straight ? Math.abs(Math.abs(r.side[t]) - Math.abs(inp.side[t])) : Math.abs(r.side[t] - inp.side[t]);
      let want = inp.buttons[t];  // what the game plays: auto bhop lets a held jump go in the air
      if (inp.autobhop && (want & IN.jump) && (t > 0 ? !r.on_ground[t - 1] : startAir)) want &= ~IN.jump;
      const btn = smooth ? (r.buttons[t] & ~WASD) !== (want & ~WASD)
        : straight ? (r.buttons[t] & ~AD) !== (want & ~AD) || !!(r.buttons[t] & AD) !== !!(want & AD)
          : r.buttons[t] !== want;
      if (btn || (!smooth && (Math.abs(r.fwd[t] - inp.fwd[t]) > 1e-3 || side > 1e-3))) return t;
      if (isNaN(inp.strafe[t]) && Math.abs(((inp.yaw[t] - r.yaw[t]) % 360 + 540) % 360 - 180) > 2e-3) return t;
      if (!isNaN(inp.strafe[t]) && t > 0 && strafeChanged(r, inp, t)) return t;
    }
    return n;
  }
  // An auto-strafe tick whose settings (offset, heading, swing) aren't the ones the path played with: the keys and
  // keyframes are the same, but what the game picks from the path's own state the tick before (A or D, and the aim)
  // isn't what it played there.
  function strafeChanged(r, inp, t) {
    const j = t - 1, st = { vel: [r.vx[j], r.vy[j], r.vz[j]], ground: !!r.on_ground[j], ducked: !!r.ducked[j], old: r.buttons[j] };
    if (!isNaN(inp.smooth[t])) {  // a smooth swing: the W, A or D and how hard, and the aim along the velocity
      const sw = smoothSwing(st, inp.side[t], inp.heading[t], inp.swing[t], inp.smooth[t],
        j > 0 ? Math.atan2(r.vy[j - 1], r.vx[j - 1]) * DEG : null, r.side[j]);
      if (sw) return Math.abs(sw[0] - r.fwd[t]) > 0.5 || Math.abs(sw[1] - r.side[t]) > 0.5
        || Math.abs(((sw[2] - r.yaw[t]) % 360 + 540) % 360 - 180) > 0.01;
    }
    let smove = inp.side[t];
    if (!isNaN(inp.heading[t])) {
      smove = straightSide(st, smove, inp.heading[t], inp.swing[t], r.side[j]);
      if (Math.sign(smove) !== Math.sign(r.side[t])) return true;
    }
    const M = Object.assign({}, DEFAULT_MOVE, r.move || {});
    const aim = strafeYaw(st, M, inp.fwd[t], smove, inp.strafe[t], inp.yaw[t] - inp.yaw[j], r.buttons[t]);
    return Math.abs((((aim === null ? inp.yaw[t] : aim) - r.yaw[t]) % 360 + 540) % 360 - 180) > 0.01;
  }
  // stamina after tick k - 1 of a path: the runs start rested, and each jump the path made costs it
  function staminaAt(r, k, dt) {
    let ms = 0;
    for (let t = 0; t < k; t++) {
      if (ms > 0) ms = Math.max(0, ms - 1000 * dt);
      const was = t > 0 ? r.on_ground[t - 1] : 1, held = t > 0 && (r.buttons[t - 1] & IN.jump);
      if (was && (r.buttons[t] & IN.jump) && !held && !r.on_ground[t]) ms = STAMINA_JUMP;
    }
    return ms;
  }
  // a duck under way after tick k - 1 (the path holding duck on the ground and not ducked yet, or the other way)
  function duckAt(r, k, dt) {
    const j = k - 1, want = !!(r.buttons[j] & IN.duck);
    if (!r.on_ground[j] || want === !!r.ducked[j]) return { trans: null, elapsed: 0 };
    let since = j;
    while (since > 0 && !!(r.buttons[since - 1] & IN.duck) === want) since--;
    return { trans: want ? "down" : "up", elapsed: (j - since) * 1000 * dt };
  }
  function startState(r, k, stamina) {  // stamina: when the caller already knows it
    if (k > 0) {
      const j = k - 1, dt = (r.move && r.move.dt) || DEFAULT_MOVE.dt;
      return { pos: [r.x[j], r.y[j], r.z[j]], vel: [r.vx[j], r.vy[j], r.vz[j]], ground: !!r.on_ground[j], ducked: !!r.ducked[j],
        base: [r.base_vx[j], r.base_vy[j], r.base_vz[j]], friction: 1, old: r.buttons[j],
        stamina: stamina !== undefined ? stamina : staminaAt(r, k, dt), ...duckAt(r, k, dt) };
    }
    const s = S.doc && S.doc.start;  // from tick 0: the timeline's own start (when it's written in it)
    if (!s || typeof s !== "object" || !Array.isArray(s.origin)) return null;
    return { pos: s.origin.map(Number), vel: Array.isArray(s.velocity) ? s.velocity.map(Number) : [0, 0, 0], ground: s.on_ground !== false,
      ducked: false, base: [0, 0, 0], friction: 1, old: 0, stamina: 0, trans: null, elapsed: 0 };
  }
  // what changed since the prediction was made: the inputs (S.ver), the exact paths, the map, whether it's wanted
  const keyNow = simulate => [S.norm, S.ver, S.pv, S.run, W.ready, W.name, simulate];
  function begin(simulate) {
    R = null; J.job = null; J.changed = new Map(); J.n = 0;
    if (!S.norm) return;
    const inp = inputsNow();
    J.n = inp.n;
    let best = null;
    for (const r of [S.pv, S.run]) {
      if (!r || !r.fwd || !r.x || !r.x.length) continue;
      const k = firstChange(r, inp);
      J.changed.set(r, k);
      if (!best || k > best.k) best = { r, k };
    }
    if (!simulate || !W.ready || !best || best.k >= inp.n) return;  // not wanted, no map, nothing to go on, or nothing changed
    const st = startState(best.r, best.k);
    if (!st) return;
    const M = Object.assign({}, DEFAULT_MOVE, best.r.move || {});
    R = { from: best.k, ref: best.r, x: [], y: [], z: [], vx: [], vy: [], vz: [], hspeed: [], ground: [], lost: [], events: [], done: false,
      yaw: [], side: [], buttons: [] };  // what it plays (auto-strafe: its own aim, straight: its A or D, auto bhop: its jumps)
    J.job = { inp, st, M, t: best.k, prevYaw: best.k > 0 ? inp.yaw[best.k - 1] : inp.yaw[0],
      prevSide: best.k > 0 ? best.r.side[best.k - 1] : 0,  // the side the path played the tick before (a swing keeps it)
      prevDir: best.k > 1 ? Math.atan2(best.r.vy[best.k - 2], best.r.vx[best.k - 2]) * DEG : null };  // and where it went then
  }
  function work(budget) {
    const j = J.job; if (!j || !R) return;
    const stop = performance.now() + budget, { inp, st, M } = j;
    while (j.t < inp.n) {
      const t = j.t;
      let yaw = inp.yaw[t], fmove = inp.fwd[t], smove = inp.side[t], buttons = inp.buttons[t];
      const dirNow = Math.atan2(st.vel[1], st.vel[0]) * DEG;
      const sw = !isNaN(inp.strafe[t]) && !isNaN(inp.smooth[t])
        ? smoothSwing(st, smove, inp.heading[t], inp.swing[t], inp.smooth[t], j.prevDir, j.prevSide) : null;
      if (sw) {  // a smooth swing: the keys are the wish direction (some W, and A or D), the view along the velocity
        [fmove, smove, yaw] = sw;
        buttons = (buttons & ~(IN.forward | IN.back | IN.moveleft | IN.moveright)) | (fmove > 0 ? IN.forward : 0) | (smove < 0 ? IN.moveleft : IN.moveright);
        if (inp.autobhop && (buttons & IN.jump) && !st.ground) buttons &= ~IN.jump;
      } else if (!isNaN(inp.strafe[t])) {
        if (!isNaN(inp.heading[t])) {  // straight: A or D, whichever steers towards the heading
          const was = smove;
          smove = straightSide(st, smove, inp.heading[t], inp.swing[t], j.prevSide);
          if (smove !== was) buttons = (buttons & ~(IN.moveleft | IN.moveright)) | (smove < 0 ? IN.moveleft : IN.moveright);
        }
        if (inp.autobhop && (buttons & IN.jump) && !st.ground) buttons &= ~IN.jump;  // first: it decides whether the tick jumps
        const auto = strafeYaw(st, M, inp.fwd[t], smove, inp.strafe[t], inp.yaw[t] - j.prevYaw, buttons);
        if (auto !== null) yaw = auto;
      } else if (inp.autobhop && (buttons & IN.jump) && !st.ground) {
        buttons &= ~IN.jump;  // auto bhop (the plugin's AutoBhop_Apply): a held jump let go in the air, so landing jumps
      }
      j.prevYaw = inp.yaw[t]; j.prevSide = smove; j.prevDir = dirNow;
      const ev = tick(st, M, yaw, fmove, smove, buttons);
      R.x.push(st.pos[0]); R.y.push(st.pos[1]); R.z.push(st.pos[2]);
      R.vx.push(st.vel[0]); R.vy.push(st.vel[1]); R.vz.push(st.vel[2]);
      R.hspeed.push(Math.hypot(st.vel[0], st.vel[1])); R.ground.push(st.ground ? 1 : 0); R.lost.push(st.lost);
      R.yaw.push(yaw); R.side.push(smove); R.buttons.push(buttons);
      if (ev) R.events.push([t, ev]);
      j.t++;
      if (((j.t - R.from) & 63) === 0 && performance.now() > stop) break;
    }
    R.done = j.t >= inp.n;
    R.version = (R.version || 0) + 1;
  }
  function schedule() {
    if (J.wait) return;
    let raf, to;
    const go = () => {
      cancelAnimationFrame(raf); clearTimeout(to); J.wait = null;
      if (!R || R.done) return;
      work(6);
      render();
      if (R && !R.done) schedule();
    };
    J.wait = true; raf = requestAnimationFrame(go); to = setTimeout(go, 40);
  }
  // The page calls this whenever it draws: start again when something changed, carry on otherwise. Without
  // simulate (the prediction turned off) it still tells which exact paths these inputs differ from.
  function update(simulate = true) {
    if (!S.norm) return;
    if (simulate && W.name !== S.norm.map && !W.loading) load(S.norm.map);
    const key = keyNow(simulate);
    if (J.key && key.every((k, i) => k === J.key[i])) return;
    J.key = key;
    begin(simulate);
    if (R) { work(8); schedule(); }  // the first ticks now, in this frame
  }

  // For checking the movement against the game (from the console): replay a run's own inputs from tick k on,
  // with err how far each tick ends from where the game put the player.
  function replay(r, k, ticks) {
    if (!W.ready) return null;
    const st = startState(r, k); if (!st) return null;
    const M = Object.assign({}, DEFAULT_MOVE, r.move || {}), out = [];
    for (let t = k; t < Math.min(r.x.length, k + ticks); t++) {
      const ev = tick(st, M, r.yaw[t], r.fwd[t], r.side[t], r.buttons[t]);
      out.push({ t, pos: st.pos.slice(), vel: st.vel.slice(), ground: st.ground, ev, lost: st.lost, err: Math.hypot(st.pos[0] - r.x[t], st.pos[1] - r.y[t], st.pos[2] - r.z[t]) });
    }
    return out;
  }

  // The speed an exact path (a run or a preview) lost hitting things, per tick: each tick played again from the
  // state the game left after the tick before, with the inputs it played. A tick that doesn't come out where
  // the game put the player, at the game's velocity (something here the game doesn't have, or the other way,
  // or a rounding the game didn't make), counts as nothing.
  // Worked out a slice at a time, the page drawn again as it goes: { lost, t: ticks done so far, done }.
  const LOSS = new WeakMap();
  function losses(r) {
    if (!S.norm || !r || !r.fwd || !r.x || !r.x.length) return null;
    if (W.name !== S.norm.map) { if (!W.loading) load(S.norm.map); return null; }
    if (!W.ready) return null;
    let c = LOSS.get(r);
    if (!c || c.world !== W.gen) { c = { lost: new Float32Array(r.x.length), t: 0, stamina: 0, world: W.gen, done: false }; LOSS.set(r, c); }
    if (!c.done && !c.wait) {
      const go = () => { c.wait = false; lossWork(r, c, 6); if (!c.done) { c.wait = true; setTimeout(go, 16); } render(); };
      lossWork(r, c, 4);
      if (!c.done) { c.wait = true; setTimeout(go, 16); }
    }
    return c;
  }
  function lossWork(r, c, budget) {
    const stop = performance.now() + budget, M = Object.assign({}, DEFAULT_MOVE, r.move || {}), n = r.x.length;
    while (c.t < n) {
      const t = c.t;
      if (t > 0) {  // tick 0 starts from the start state, not a tick of the path
        const st = startState(r, t, c.stamina);
        tick(st, M, r.yaw[t], r.fwd[t], r.side[t], r.buttons[t]);
        const off = Math.hypot(st.pos[0] - r.x[t], st.pos[1] - r.y[t], st.pos[2] - r.z[t]);
        const dv = Math.hypot(st.vel[0] - r.vx[t], st.vel[1] - r.vy[t], st.vel[2] - r.vz[t]);
        c.lost[t] = off < 1 && dv < 2 ? st.lost : 0;
      }
      if (c.stamina > 0) c.stamina = Math.max(0, c.stamina - 1000 * M.dt);  // as staminaAt, a tick at a time
      const was = t > 0 ? r.on_ground[t - 1] : 1, held = t > 0 && (r.buttons[t - 1] & IN.jump);
      if (was && (r.buttons[t] & IN.jump) && !held && !r.on_ground[t]) c.stamina = STAMINA_JUMP;
      c.t++;
      if ((c.t & 63) === 0 && performance.now() > stop) break;
    }
    c.done = c.t >= n;
  }

  window.Predict = {
    update, losses,
    result: () => R,
    // what to say about it: loading, why there's none, or where it starts
    status: () => W.loading ? "loading the map's brushes…" : W.error ? `no prediction: ${W.error}` : !R ? "" :
      `predicted from tick ${R.from}${R.done ? "" : "…"}`,
    // whether these inputs differ from what an exact path played (so it isn't their result, whatever the hash says yet)
    differs: r => !!(r && J.changed && J.changed.has(r) && J.changed.get(r) < Math.min(r.x.length, J.n)),
    // whether an exact path played these very inputs (all of them it covers), whatever its hash says
    matches: r => !!(r && J.changed && J.changed.has(r) && J.changed.get(r) >= Math.min(r.x.length, J.n)),
    world: W, replay,
  };
})();
