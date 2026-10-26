// 3D view: the map drawn from its .bsp file (textured and lit, with its sky and props), with the run's path and
// the player at the editor's tick, in the viewer ("3D" next to its "Top" button, or V). It adds its own "3D"
// button to the viewer's #viewmodes. The Export page's preview draws its frames with View3D.snapshot.
//
// The map comes from the editor's /api/map, /api/maptex, /api/maptrig, /api/mapprops and /api/mapfile
// (tasurf/bspmap.py and gamefiles.py, rust/src/bspmap.rs and gamefiles.rs).
//
// It reads the page's state (S, VIS, PM) and calls a few of its functions (render, setTick, pvFresh, saveVis, and
// the path marks': markPath, pathMarks, markAt, markGrab, markDrag, markDrop, markPoint).
"use strict";
(() => {
  const V = {
    on: false, gl: null, prog: null, map: null, mapName: null, loading: null, error: null,
    mode: "follow", orbit: { yaw: null, pitch: 22, dist: 450 }, free: { pos: [0, 0, 0], yaw: 0, pitch: 25, speed: 900 },
    keys: new Set(), hover: false, drag: null, last: 0, mvp: null, big: false,
    tex: null, texLoading: null,  // the textured look of the map (Textured), once loaded
    trig: null, trigLoading: null,  // the map's triggers (Triggers), once loaded
  };

  // ---------------------------------------------------------------- UI (inside the Visuals toolbox)
  const style = document.createElement("style");
  style.textContent = `
    #view3d { position: absolute; inset: 0; width: 100%; height: 100%; display: block; cursor: grab; }
    #view3d[hidden], #v3dbar[hidden], #v3dmsg[hidden] { display: none; }
    #v3dbar { position: absolute; left: 8px; top: 8px; right: 8px; display: flex; gap: 8px; align-items: center; pointer-events: none;
      font-size: 12px; color: var(--dim); }
    #v3dbar .seg { pointer-events: auto; box-shadow: 0 2px 8px rgba(0,0,0,.35); }
    #v3dbar .tip { display: inline-flex; align-items: center; min-width: 0; height: 24px; padding: 0 9px; border-radius: 12px;
      background: rgba(12, 13, 16, .72); border: 1px solid rgba(255, 255, 255, .06); color: var(--text); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    #v3dbar .tip:empty { display: none; }
    #v3dtrigs { max-height: calc(100% - 44px); overflow: auto; }
    #v3dtrigs .sw { width: 10px; height: 10px; border-radius: 2px; flex: none; }
    #v3dtrigs .n { margin-left: auto; padding-left: 14px; color: var(--dim); font-variant-numeric: tabular-nums; }
    #v3dtrigs .popfoot { display: flex; padding: 6px 6px 2px; margin-top: 4px; border-top: 1px solid var(--line); }
    #v3dlabel { position: absolute; pointer-events: none; padding: 3px 8px; border-radius: 4px; background: rgba(12, 13, 16, .92);
      border: 1px solid var(--line); box-shadow: 0 4px 12px rgba(0,0,0,.4); color: var(--text); font-size: 12px; white-space: nowrap; }
    #v3dmsg { position: absolute; inset: 0; display: grid; place-items: center; color: var(--dim); pointer-events: none; text-align: center; padding: 20px; }
`;
  document.head.appendChild(style);
  const wrap = $("pathwrap");
  const cv = document.createElement("canvas"); cv.id = "view3d"; cv.hidden = true;
  const bar = document.createElement("div"); bar.id = "v3dbar"; bar.hidden = true;
  bar.innerHTML = `<span class="seg"><button data-m="follow" title="Orbit the player at the editor's tick: drag (or hold the right button) to turn, wheel to zoom, double-click: behind the player">Follow</button>` +
    `<button data-m="eye" title="Through the player's eyes at the editor's tick (the yaw the game played)">Eye</button>` +
    `<button data-m="free" title="Fly as in noclip: hold the right button to look (the mouse turns the view, as in the game), WASD to move (Q/E down/up, Shift faster), wheel for speed">Free</button></span>` +
    `<span class="seg"><button data-look="tex" title="The map's textures and baked lighting, as in the game (off: plain colours, floors grey and surf ramps blue)">Textured</button>` +
    `<button data-look="trig" title="The map's triggers as see-through volumes, coloured by type (teleports, pushes, zones, ...); point at one for its name">Triggers</button>` +
    `<button data-look="trigmenu" title="Which types of trigger to draw">${typeof icon === "function" ? icon("chevron-down") : "▾"}</button></span>` +
    `<span class="tip" id="v3dtip"></span>`;
  const msg = document.createElement("div"); msg.id = "v3dmsg"; msg.hidden = true;
  const trigs = document.createElement("div"); trigs.id = "v3dtrigs"; trigs.className = "pop"; trigs.hidden = true;  // the trigger types
  const label = document.createElement("div"); label.id = "v3dlabel"; label.hidden = true;  // the trigger pointed at
  wrap.append(cv, bar, msg, trigs, label);
  const toggle = document.createElement("button");
  toggle.id = "v3dtoggle"; toggle.textContent = "3D";
  toggle.title = "The map in 3D from its .bsp file, with the run and the player at the tick (V)";
  $("viewmodes").appendChild(toggle);
  if (VIS.v3dtex === undefined) VIS.v3dtex = true;  // textured unless turned off
  if (!Array.isArray(VIS.v3dtrigOff)) VIS.v3dtrigOff = ["trigger_soundscape"];  // the trigger types not drawn
  const set3D = on => { VIS.v3d = on; saveVis(); apply(); };
  toggle.onclick = () => set3D(true);
  $("viewtop").onclick = () => set3D(false);
  bar.addEventListener("click", e => {
    const b = e.target.closest("button"); if (!b) return;
    const look = b.dataset.look;
    if (look === "tex") { VIS.v3dtex = !VIS.v3dtex; saveVis(); requestDraw(); }
    else if (look === "trig") { VIS.v3dtrig = !VIS.v3dtrig; saveVis(); requestDraw(); }
    else if (look === "trigmenu") {
      trigs.hidden = !trigs.hidden;
      if (!trigs.hidden) { trigs.style.left = `${b.getBoundingClientRect().left - wrap.getBoundingClientRect().left}px`; triggerMenu(); }
    } else setMode(b.dataset.m);
  });
  // the menu: a type's box shows or hides it (and turns Triggers on); All and None
  trigs.addEventListener("change", e => {
    const c = e.target.dataset.cls; if (c === undefined) return;
    const off = new Set(VIS.v3dtrigOff); if (e.target.checked) off.delete(c); else off.add(c);
    VIS.v3dtrigOff = [...off]; VIS.v3dtrig = true; saveVis(); requestDraw();
  });
  trigs.addEventListener("click", e => {
    const b = e.target.closest("[data-all]"); if (!b || !V.trig || !V.trig.classes) return;
    const off = new Set(VIS.v3dtrigOff);
    for (const c of V.trig.classes.keys()) { if (b.dataset.all === "1") off.delete(c); else off.add(c); }
    VIS.v3dtrigOff = [...off]; VIS.v3dtrig = true; saveVis(); triggerMenu(); requestDraw();
  });
  document.addEventListener("mousedown", e => { if (!trigs.hidden && !e.target.closest("#v3dtrigs, [data-look=trigmenu]")) trigs.hidden = true; });

  function setMode(m) {
    if (m === "free" && V.mode !== "free") {  // fly on from where the camera is now
      const c = camera();
      if (c) { V.free.pos = c.eye.slice(); V.free.yaw = c.yaw; V.free.pitch = c.pitch; }
    }
    V.mode = m; requestDraw();
  }
  function apply() {
    V.on = !!VIS.v3d;
    cv.hidden = bar.hidden = !V.on;
    toggle.classList.toggle("on", V.on); $("viewtop").classList.toggle("on", !V.on);
    PV.el.style.visibility = V.on ? "hidden" : "";
    $("pathlegend").style.visibility = V.on ? "hidden" : "";
    if (V.on) init(); else msg.hidden = true;
    render();  // the page redraws (the top view comes back, or stops drawing behind this)
  }

  // ---------------------------------------------------------------- WebGL
  const MAP_VS = `#version 300 es
    in vec3 a_pos; uniform mat4 u_mvp; out vec3 v_pos;
    void main() { v_pos = a_pos; gl_Position = u_mvp * vec4(a_pos, 1.0); }`;
  const MAP_FS = `#version 300 es
    precision highp float;
    in vec3 v_pos; uniform vec3 u_eye; out vec4 o;
    void main() {
      vec3 n = normalize(cross(dFdx(v_pos), dFdy(v_pos)));
      float nz = abs(n.z);
      vec3 c = nz > 0.7 ? vec3(0.47, 0.49, 0.53)   // floors (and ceilings): you can stand there
             : nz > 0.05 ? vec3(0.25, 0.55, 0.80)  // surf ramps: too steep to stand on
             : vec3(0.31, 0.33, 0.38);             // walls
      float light = 0.42 + 0.58 * abs(dot(n, normalize(vec3(0.35, 0.25, 0.9))));
      vec3 g = abs(fract(v_pos / 256.0 + 0.5) - 0.5) / max(fwidth(v_pos / 256.0), vec3(1e-4));
      float grid = 1.0 - clamp(min(min(g.x, g.y), g.z), 0.0, 1.0);  // a faint 256-unit grid
      vec3 col = c * light * (1.0 - 0.14 * grid);
      float fog = 1.0 - exp(-distance(v_pos, u_eye) / 9000.0);
      o = vec4(mix(col, vec3(0.08, 0.09, 0.11), fog * 0.75), 1.0);
    }`;
  // lines as screen-space ribbons, so they keep their width at any distance
  const LINE_VS = `#version 300 es
    in vec3 a_pos; in vec3 a_other; in float a_side; in vec4 a_col; in float a_width; in float a_tick;
    uniform mat4 u_mvp; uniform vec2 u_half; uniform float u_dpr; uniform float u_alpha; uniform float u_tick;
    out vec4 v_col;
    void main() {
      vec4 p = u_mvp * vec4(a_pos, 1.0), q = u_mvp * vec4(a_other, 1.0);
      vec2 ps = p.xy / max(p.w, 1e-3) * u_half, qs = q.xy / max(q.w, 1e-3) * u_half;
      vec2 d = qs - ps; d = length(d) > 1e-4 ? normalize(d) : vec2(1.0, 0.0);
      vec2 off = vec2(-d.y, d.x) * a_side * a_width * u_dpr * 0.5;
      gl_Position = p + vec4(off / u_half * p.w, 0.0, 0.0);
      gl_Position.z -= 0.0004 * gl_Position.w;  // just in front of the surface it runs along
      float dim = a_tick >= 0.0 && a_tick > u_tick ? 0.4 : 1.0;  // after the editor's tick: dimmer
      v_col = vec4(a_col.rgb, a_col.a * u_alpha * dim);
    }`;
  const LINE_FS = `#version 300 es
    precision highp float; in vec4 v_col; out vec4 o; void main() { o = v_col; }`;
  // Textured: the material's texture (texture positions come in texels), blended with a second one on
  // displacements that have one, times the baked light. The lightmap atlas holds half the light, so
  // surfaces can be lit up to twice their texture's brightness (the game's "overbright").
  // triggers: see-through faces in their type's colour
  const TRIG_VS = `#version 300 es
    in vec3 a_pos; in vec4 a_col; uniform mat4 u_mvp; out vec4 v_col;
    void main() { v_col = a_col; gl_Position = u_mvp * vec4(a_pos, 1.0); }`;
  const TRIG_FS = `#version 300 es
    precision highp float; in vec4 v_col; out vec4 o; void main() { o = v_col; }`;
  const TEX_VS = `#version 300 es
    in vec3 a_pos; in vec2 a_uv; in vec2 a_lm; in float a_blend;
    uniform mat4 u_mvp; uniform vec2 u_texel; uniform mat3 u_xform;
    out vec2 v_uv; out vec2 v_lm; out float v_blend;
    void main() {
      v_uv = (u_xform * vec3(a_uv * u_texel, 1.0)).xy; v_lm = a_lm; v_blend = clamp(a_blend / 255.0, 0.0, 1.0);
      gl_Position = u_mvp * vec4(a_pos, 1.0);
    }`;
  const TEX_FS = `#version 300 es
    precision highp float;
    in vec2 v_uv; in vec2 v_lm; in float v_blend;
    uniform sampler2D u_base, u_base2, u_light;
    uniform vec4 u_color;  // the tint, and the alpha when see-through
    uniform vec4 u_opt;    // x: blend in the second texture, y: unlit, z: alpha test (0: none), w: see-through
    out vec4 o;
    void main() {
      vec4 c = texture(u_base, v_uv);
      if (u_opt.x > 0.5) c = mix(c, texture(u_base2, v_uv), v_blend);
      if (c.a < u_opt.z) discard;
      vec3 light = u_opt.y > 0.5 ? vec3(1.0) : texture(u_light, v_lm).rgb * 2.0;
      o = vec4(c.rgb * u_color.rgb * light, u_opt.w > 0.5 ? c.a * u_color.a : 1.0);
    }`;

  // the sky: a cube round the eye, drawn first behind everything; a side half as tall as it's wide covers the top
  // half and its last row stretches down (u_half 2), as in the game
  const SKY_VS = `#version 300 es
    in vec3 a_pos; in vec2 a_uv; uniform mat4 u_mvp; uniform vec3 u_eye; uniform float u_half; out vec2 v_uv;
    void main() { v_uv = vec2(a_uv.x, a_uv.y * u_half); gl_Position = u_mvp * vec4(u_eye + a_pos * 2048.0, 1.0); }`;
  const SKY_FS = `#version 300 es
    precision highp float; in vec2 v_uv; uniform sampler2D u_tex; out vec4 o; void main() { o = vec4(texture(u_tex, v_uv).rgb, 1.0); }`;
  // props: a model's triangles at each of its places (per instance the columns of the place's rotation and its
  // origin), lit by a light from above at about the map's own brightness (u_level); u_opt as Textured's
  const PROP_VS = `#version 300 es
    in vec3 a_pos; in vec3 a_nrm; in vec2 a_uv; in vec3 a_c0; in vec3 a_c1; in vec3 a_c2; in vec3 a_org;
    uniform mat4 u_mvp; uniform mat3 u_xform; out vec2 v_uv; out vec3 v_n;
    void main() {
      v_uv = (u_xform * vec3(a_uv, 1.0)).xy; v_n = a_c0 * a_nrm.x + a_c1 * a_nrm.y + a_c2 * a_nrm.z;
      gl_Position = u_mvp * vec4(a_c0 * a_pos.x + a_c1 * a_pos.y + a_c2 * a_pos.z + a_org, 1.0);
    }`;
  const PROP_FS = `#version 300 es
    precision highp float;
    in vec2 v_uv; in vec3 v_n;
    uniform sampler2D u_base; uniform vec4 u_color; uniform vec4 u_opt; uniform float u_level;
    out vec4 o;
    void main() {
      vec4 c = texture(u_base, v_uv);
      if (c.a < u_opt.z) discard;
      float h = dot(normalize(v_n + vec3(0.0, 0.0, 1e-5)), normalize(vec3(0.35, 0.25, 0.9))) * 0.5 + 0.5;  // half-Lambert, as the game's models
      vec3 light = u_opt.y > 0.5 ? vec3(1.0) : vec3(u_level * (0.7 + 0.45 * h * h));
      o = vec4(c.rgb * u_color.rgb * light, u_opt.w > 0.5 ? c.a * u_color.a : 1.0);
    }`;

  function compile(gl, vs, fs) {
    const p = gl.createProgram();
    for (const [type, src] of [[gl.VERTEX_SHADER, vs], [gl.FRAGMENT_SHADER, fs]]) {
      const s = gl.createShader(type); gl.shaderSource(s, src); gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
      gl.attachShader(p, s);
    }
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
    return p;
  }
  function init() {
    if (V.gl || V.error) return;
    const gl = cv.getContext("webgl2", { antialias: true });
    if (!gl) { V.error = "The 3D view needs WebGL2, which this browser doesn't have."; return; }
    try {
      V.mapProg = compile(gl, MAP_VS, MAP_FS);
      V.lineProg = compile(gl, LINE_VS, LINE_FS);
      V.texProg = compile(gl, TEX_VS, TEX_FS);
      V.trigProg = compile(gl, TRIG_VS, TRIG_FS);
      V.skyProg = compile(gl, SKY_VS, SKY_FS);
      V.propProg = compile(gl, PROP_VS, PROP_FS);
    } catch (e) { V.error = "3D view: " + e.message; return; }
    const U = (p, names) => Object.fromEntries(names.map(n => [n, gl.getUniformLocation(p, n)]));
    V.mapU = U(V.mapProg, ["u_mvp", "u_eye"]);
    V.lineU = U(V.lineProg, ["u_mvp", "u_half", "u_dpr", "u_alpha", "u_tick"]);
    V.texU = U(V.texProg, ["u_mvp", "u_texel", "u_xform", "u_base", "u_base2", "u_light", "u_color", "u_opt"]);
    V.trigU = U(V.trigProg, ["u_mvp"]);
    V.skyU = U(V.skyProg, ["u_mvp", "u_eye", "u_half", "u_tex"]);
    V.propU = U(V.propProg, ["u_mvp", "u_xform", "u_base", "u_color", "u_opt", "u_level"]);
    V.s3tc = gl.getExtension("WEBGL_compressed_texture_s3tc");  // DXT textures straight to the GPU
    V.aniso = gl.getExtension("EXT_texture_filter_anisotropic");
    V.white = gl.createTexture(); gl.bindTexture(gl.TEXTURE_2D, V.white);  // for materials without a texture
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([255, 255, 255, 255]));
    V.idx = gl.createBuffer(); V.idxCap = 0;
    V.gl = gl;
    V.paths = lineSlot(); V.marks = lineSlot(); V.inputs = lineSlot();
    V.skyVao = skyCube();
  }
  // Lines: per segment 4 vertices of LF floats (pos 3, other 3, side 1, colour 4, width 1, tick 1), drawn as two
  // triangles through one shared index buffer. Each slot has its buffer and vertex array, set up once.
  const LF = 13;
  function lineSlot() {
    const gl = V.gl, buf = gl.createBuffer(), vao = gl.createVertexArray();
    gl.bindVertexArray(vao); gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    for (const [n, size, off] of [["a_pos", 3, 0], ["a_other", 3, 12], ["a_side", 1, 24], ["a_col", 4, 28], ["a_width", 1, 44], ["a_tick", 1, 48]]) {
      const l = gl.getAttribLocation(V.lineProg, n);
      if (l >= 0) { gl.enableVertexAttribArray(l); gl.vertexAttribPointer(l, size, gl.FLOAT, false, LF * 4, off); }
    }
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, V.idx);
    gl.bindVertexArray(null);
    return { buf, vao, count: 0 };
  }
  function fillSlot(slot, verts, usage) {
    const gl = V.gl, segs = verts.length / (LF * 4);
    if (segs > V.idxCap) {  // the shared indices: 0 1 2, 2 1 3 per segment (the same buffer, so the arrays keep it)
      V.idxCap = Math.max(segs, V.idxCap * 2, 1024);
      const ix = new Uint32Array(V.idxCap * 6);
      for (let i = 0; i < V.idxCap; i++) { const b = i * 4, o = i * 6; ix[o] = b; ix[o + 1] = b + 1; ix[o + 2] = b + 2; ix[o + 3] = b + 2; ix[o + 4] = b + 1; ix[o + 5] = b + 3; }
      gl.bindVertexArray(null); gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, V.idx); gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, ix, gl.STATIC_DRAW);
    }
    gl.bindBuffer(gl.ARRAY_BUFFER, slot.buf); gl.bufferData(gl.ARRAY_BUFFER, verts, usage);
    slot.count = segs * 6;
  }

  // ---------------------------------------------------------------- the map
  async function loadMap(name) {
    V.mapName = name; V.loading = name; V.error = null; requestDraw();
    try {
      const r = await fetch("/api/map?name=" + encodeURIComponent(name));
      if (!r.ok) { const j = await r.json().catch(() => ({})); throw new Error(j.error || `HTTP ${r.status}`); }
      const buf = await r.arrayBuffer();
      if (V.mapName !== name) return;  // another timeline was opened meanwhile
      const head = new DataView(buf, 0, 40);
      if (String.fromCharCode(...new Uint8Array(buf, 0, 4)) !== "TASM" || head.getUint32(4, true) !== 1) throw new Error("unexpected map data");
      const nv = head.getUint32(8, true), ni = head.getUint32(12, true);
      const b = [16, 20, 24, 28, 32, 36].map(o => head.getFloat32(o, true));
      const gl = V.gl;
      const vao = gl.createVertexArray(); gl.bindVertexArray(vao);
      const vbo = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, vbo); gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(buf, 40, nv * 3), gl.STATIC_DRAW);
      const loc = gl.getAttribLocation(V.mapProg, "a_pos");
      gl.enableVertexAttribArray(loc); gl.vertexAttribPointer(loc, 3, gl.FLOAT, false, 12, 0);
      const ibo = gl.createBuffer(); gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo); gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, new Uint32Array(buf, 40 + nv * 12, ni), gl.STATIC_DRAW);
      gl.bindVertexArray(null);
      disposeTextured(V.tex); V.tex = null;
      disposeTriggers(V.trig); V.trig = null; label.hidden = true;
      if (V.map) { gl.deleteVertexArray(V.map.vao); gl.deleteBuffer(V.map.vbo); gl.deleteBuffer(V.map.ibo); }
      V.map = { vao, vbo, ibo, count: ni, mins: b.slice(0, 3), maxs: b.slice(3), tris: ni / 3, nv,
                idx: new Uint32Array(buf, 40 + nv * 12, ni).slice() };  // idx: sorted by material for Textured
      V.loading = null;
      if (V.mode === "free" && !player()) overview();
    } catch (e) {
      if (V.mapName === name) { V.loading = null; V.map = null; V.error = `Can't show ${name} in 3D: ${e.message}`; }
    }
    requestDraw();
  }

  // ---------------------------------------------------------------- the textured look (Textured)
  // The map as the game draws it: each face's material (its .vmt) and texture (.vtf), found where the game
  // finds them (/api/mapfile: the map's own files, then the game's), lit by the map's baked lighting
  // (/api/maptex: per vertex where it is on its texture and on the lighting atlas, per triangle its
  // material). Until a material has loaded, or if it can't be found, it shows its average colour.
  async function loadTextured(name) {
    V.texLoading = name; requestDraw();
    let t;
    try {
      const r = await fetch("/api/maptex?name=" + encodeURIComponent(name));
      if (!r.ok) { const j = await r.json().catch(() => ({})); throw new Error(j.error || `HTTP ${r.status}`); }
      const buf = await r.arrayBuffer();
      if (V.mapName !== name || !V.map) { V.texLoading = null; requestDraw(); return; }
      t = buildTextured(readTextured(buf), name);
    } catch (e) { t = { name, error: e.message }; }
    V.texLoading = null;
    if (V.mapName === name) { disposeTextured(V.tex); V.tex = t; if (!t.error) { loadMaterials(t); loadProps(t); } }
    else disposeTextured(t);  // another map was opened meanwhile
    requestDraw();
  }
  function readTextured(buf) {  // the TAST data (see tasurf/bspmap.py)
    const dv = new DataView(buf), u8 = new Uint8Array(buf);
    if (String.fromCharCode(...u8.subarray(0, 4)) !== "TAST" || dv.getUint32(4, true) !== 1) throw new Error("unexpected textured map data");
    const [nv, nt, nm, aw, ah, lighting] = [8, 12, 16, 20, 24, 28].map(o => dv.getUint32(o, true));
    if (nv !== V.map.nv || nt * 3 !== V.map.count) throw new Error("the textured data doesn't match the map");
    let o = 32;
    const uv = new Float32Array(buf, o, nv * 2); o += nv * 8;
    const lm = new Float32Array(buf, o, nv * 2); o += nv * 8;
    const blend = new Float32Array(buf, o, nv); o += nv * 4;
    const mat = new Uint16Array(buf, o, nt); o += Math.ceil(nt / 2) * 4;
    const mats = [];
    for (let k = 0; k < nm; k++, o += 24) {
      mats.push({ refl: [0, 4, 8].map(i => dv.getFloat32(o + i, true)), w: dv.getInt32(o + 12, true), h: dv.getInt32(o + 16, true), len: dv.getUint32(o + 20, true) });
    }
    for (const m of mats) { m.name = latin1(u8.subarray(o, o + m.len)); o += m.len; }
    o = Math.ceil(o / 4) * 4;
    return { nt, uv, lm, blend, mat, mats, aw, ah, lighting, atlas: new Uint8Array(buf, o, aw * ah * 4) };
  }
  const latin1 = u => { let s = ""; for (let i = 0; i < u.length; i += 8192) s += String.fromCharCode.apply(null, u.subarray(i, i + 8192)); return s; };
  function buildTextured(d, name) {
    const gl = V.gl, P = V.texProg, bufs = [];
    const vao = gl.createVertexArray(); gl.bindVertexArray(vao);
    const attrib = (n, buf, data, size) => {
      gl.bindBuffer(gl.ARRAY_BUFFER, buf);
      if (data) { gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW); bufs.push(buf); }
      const l = gl.getAttribLocation(P, n);
      if (l >= 0) { gl.enableVertexAttribArray(l); gl.vertexAttribPointer(l, size, gl.FLOAT, false, 0, 0); }
    };
    attrib("a_pos", V.map.vbo, null, 3);  // the map's own positions
    attrib("a_uv", gl.createBuffer(), d.uv, 2); attrib("a_lm", gl.createBuffer(), d.lm, 2); attrib("a_blend", gl.createBuffer(), d.blend, 1);
    // the triangles sorted by material, so each material is one draw (the last group: triangles without one)
    const n = d.mats.length + 1, cnt = new Uint32Array(n), group = k => Math.min(d.mat[k], n - 1);
    for (let k = 0; k < d.nt; k++) cnt[group(k)]++;
    const start = new Uint32Array(n);
    for (let k = 1; k < n; k++) start[k] = start[k - 1] + cnt[k - 1];
    const at = start.slice(), sorted = new Uint32Array(d.nt * 3), src = V.map.idx;
    for (let k = 0; k < d.nt; k++) { const o = at[group(k)]++ * 3; sorted[o] = src[3 * k]; sorted[o + 1] = src[3 * k + 1]; sorted[o + 2] = src[3 * k + 2]; }
    const ibo = gl.createBuffer(); bufs.push(ibo);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo); gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, sorted, gl.STATIC_DRAW);
    gl.bindVertexArray(null);
    const groups = [], used = [];
    for (let k = 0; k < n; k++) {
      if (!cnt[k]) continue;
      const info = d.mats[k] || { name: "", refl: [0.5, 0.5, 0.5], w: 0, h: 0 }, m = {
        name: info.name.toLowerCase().replace(/\\/g, "/").replace(/^\/+/, ""), state: k < n - 1 ? "wait" : "missing",
        color: [...info.refl.map(x => Math.pow(Math.min(1, Math.max(0, x)), 1 / 2.2)), 1],  // its average colour
        texel: [1 / (info.w > 0 ? info.w : 256), 1 / (info.h > 0 ? info.h : 256)], xform: IDENTITY,
        tex: null, tex2: null, unlit: false, test: 0, mode: "opaque",
      };
      if (m.color[0] + m.color[1] + m.color[2] < 0.01) m.color = [0.5, 0.5, 0.5, 1];
      groups.push({ m, first: start[k] * 12, count: cnt[k] * 3 });
      if (m.state === "wait") used.push(m);
    }
    const light = lightAtlas(d);
    return { name, vao, bufs, groups, used, light: light.tex, note: light.note, level: light.level, loaded: 0, missing: 0, cache: new Map(), vmts: new Map(), dead: false,
             vaos: [], sky: null, props: null, propMats: new Map(), propNote: "", extrasDone: false };
  }
  // The lighting atlas, from the map's luxels (r, g, b and an exponent: linear light r * 2^e / 255) to what
  // the game keeps for its lightmaps: the light in gamma (2.2), halved (the shader doubles it back). level: how
  // bright the map's lit places are (where 70% of its lit luxels are darker), for lighting the props alike.
  function lightAtlas(d) {
    const gl = V.gl, max = gl.getParameter(gl.MAX_TEXTURE_SIZE), hist = new Uint32Array(256);
    let px = d.atlas, w = d.aw, h = d.ah, note = d.lighting ? "" : "the map has no baked lighting", level = 1;
    if (w > max || h > max) { px = new Uint8Array([128, 128, 128, 255]); w = h = 1; note = "the map's lighting is too big for this graphics card"; }
    else {
      const lut = [];  // per exponent: the 256 values
      for (let i = 0; i < px.length; i += 4) {
        const e = (px[i + 3] << 24 >> 24) + 128;
        let t = lut[e];
        if (!t) {
          t = lut[e] = new Uint8Array(256);
          const k = Math.pow(2, e - 128) / 255;
          for (let c = 0; c < 256; c++) t[c] = Math.min(255, Math.round(Math.pow(c * k, 1 / 2.2) * 127.5));
        }
        px[i] = t[px[i]]; px[i + 1] = t[px[i + 1]]; px[i + 2] = t[px[i + 2]]; px[i + 3] = 255;
        hist[Math.max(px[i], px[i + 1], px[i + 2])]++;
      }
      const lit = px.length / 4 - hist[0];  // not the atlas's unused (black) corners
      if (d.lighting && lit > 0) {
        let k = 1; for (let n = 0; k < 255 && (n += hist[k]) < lit * 0.7; k++);
        level = Math.min(1.6, Math.max(0.35, k / 127.5));
      }
    }
    const tex = gl.createTexture(); gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, px);
    for (const [k, v] of [[gl.TEXTURE_MIN_FILTER, gl.LINEAR], [gl.TEXTURE_MAG_FILTER, gl.LINEAR], [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE], [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE]]) gl.texParameteri(gl.TEXTURE_2D, k, v);
    return { tex, note, level };
  }
  function disposeTextured(t) {
    if (!t || t.error) return;
    const gl = V.gl; t.dead = true;
    gl.deleteVertexArray(t.vao); for (const a of t.vaos) gl.deleteVertexArray(a); for (const b of t.bufs) gl.deleteBuffer(b); gl.deleteTexture(t.light);
    for (const p of t.cache.values()) p.then(x => { if (x) gl.deleteTexture(x.tex); });
  }
  const IDENTITY = [1, 0, 0, 0, 1, 0, 0, 0, 1];

  // the map's files: a few at a time, each material once
  async function loadMaterials(t) {
    const queue = t.used.slice();
    const worker = async () => {
      while (queue.length && !t.dead) {
        const m = queue.shift();
        try { await loadMaterial(t, m); } catch (e) { m.state = "missing"; }
        if (m.state !== "done") t.missing++;
        t.loaded++; requestDraw();
      }
    };
    await Promise.all([1, 2, 3, 4, 5, 6].map(worker));
  }
  async function mapFile(t, path) {
    const r = await fetch(`/api/mapfile?name=${encodeURIComponent(t.name)}&path=${encodeURIComponent(path)}`);
    return r.ok ? r.arrayBuffer() : null;
  }
  // a path in the game's files: lower case, forward slashes, under materials/, with the extension
  function matPath(p, ext) {
    p = p.trim().toLowerCase().replace(/\\/g, "/").replace(/\/+/g, "/").replace(/^\//, "");
    if (!p.startsWith("materials/")) p = "materials/" + p;
    return p.endsWith("." + ext) ? p : `${p}.${ext}`;
  }
  async function loadMaterial(t, m) {
    const mat = await readMaterial(t, matPath(m.name, "vmt"), 0);
    if (!mat) { m.state = "missing"; return; }
    const p = mat.params, sh = mat.shader;
    const num = (k, d) => { const v = parseFloat(p[k]); return Number.isFinite(v) ? v : d; };
    // tool materials the game never draws (triggers, clips, nodraw, sky, ...) say so to the map compiler; older
    // compilers didn't always mark their faces
    if (Object.keys(p).some(k => HIDDEN_TOOL.test(k) && num(k, 1) !== 0)) { m.mode = "hidden"; m.state = "done"; return; }
    if (/^(water|refract|windowimposter)$/.test(sh)) {  // see-through surfaces drawn with their tint
      const c = sh === "water" ? parseColor(p["$fogcolor"]) || [0.2, 0.3, 0.35] : parseColor(p["$refracttint"]) || [0.75, 0.82, 0.9];
      m.color = [...c, sh === "water" ? 0.6 : 0.3]; m.mode = "blend"; m.unlit = true; m.state = "done";
      return;
    }
    m.unlit = /^(unlit|sprite|wireframe|cable)|modulate$/.test(sh);
    m.mode = /modulate$/.test(sh) ? "mod" : num("$additive", 0) ? "add" : num("$translucent", 0) || num("$alpha", 1) < 1 ? "blend" : "opaque";
    if (num("$alphatest", 0)) m.test = num("$alphatestreference", 0.5) || 0.5;
    m.xform = textureTransform(p["$basetexturetransform"]);
    const base = p["$basetexture"], tint = parseColor(p["$color"]);
    if (base) {
      const tx = await texture(t, matPath(base, "vtf"));
      if (tx) { m.tex = tx.tex; m.texel = [1 / tx.w, 1 / tx.h]; m.color = [1, 1, 1, 1]; }
      if (tx && p["$basetexture2"]) { const t2 = await texture(t, matPath(p["$basetexture2"], "vtf")); if (t2) m.tex2 = t2.tex; }
    }
    if (tint) m.color = [m.color[0] * tint[0], m.color[1] * tint[1], m.color[2] * tint[2], 1];
    m.color[3] = num("$alpha", 1);
    m.state = base && !m.tex ? "missing" : "done";
  }
  const HIDDEN_TOOL = /^%compile(nodraw|trigger|clip|playerclip|npcclip|grenadeclip|droneclip|sky|2dsky|skip|hint|origin|fog|ladder|blocklos|invisible)$/;
  // A material (KeyValues text): its shader and parameters, with the blocks the game would apply on DirectX 9;
  // a "patch" material is the one it includes with its own parameters inserted or replaced.
  async function readMaterial(t, path, depth) {
    if (!t.vmts.has(path)) t.vmts.set(path, mapFile(t, path).then(b => b && parseKV(latin1(new Uint8Array(b)))));  // once per map
    const kv = await t.vmts.get(path);
    if (!kv) return null;
    const params = { ...kv.keys };
    for (const [n, b] of kv.blocks) if (/^>=\s*dx9/.test(n) || (/_dx9$/.test(n) && !/_hdr_/.test(n))) Object.assign(params, b.keys);
    if (kv.shader !== "patch") return { shader: kv.shader, params };
    if (!params.include || depth >= 4) return null;
    const inc = await readMaterial(t, matPath(params.include, "vmt"), depth + 1);
    if (inc) for (const [n, b] of kv.blocks) if (n === "insert" || n === "replace") Object.assign(inc.params, b.keys);
    return inc;
  }
  function parseKV(text) {
    const toks = [], n = text.length;  // "{", "}", or { s: a string, q: whether it was quoted }
    for (let i = 0; i < n;) {
      const c = text.charCodeAt(i);
      if (c <= 32 || c === 0xfeff || c === 0xef || c === 0xbb || c === 0xbf) i++;  // spaces, a byte order mark
      else if (c === 47 && text.charCodeAt(i + 1) === 47) { const e = text.indexOf("\n", i); i = e < 0 ? n : e; }
      else if (c === 123 || c === 125) { toks.push(text[i]); i++; }
      else if (c === 34) { const e = text.indexOf('"', i + 1), end = e < 0 ? n : e; toks.push({ s: text.slice(i + 1, end), q: true }); i = end + 1; }
      else if (c === 91) { const e = text.indexOf("]", i), end = e < 0 ? n : e + 1; toks.push({ s: text.slice(i, end) }); i = end; }  // a condition: [$WIN32 || $OSX]
      else { let e = i; while (e < n && text.charCodeAt(e) > 32 && !'{}"'.includes(text[e])) e++; toks.push({ s: text.slice(i, e) }); i = e; }
    }
    // a condition is never quoted, as in the game's reader: a quoted "[.1 .1 .1]" is a value (a colour)
    const isCond = t => t && t.s !== undefined && !t.q && t.s[0] === "[";
    // [$WIN32], [!$X360], ...: what holds on Windows
    const holds = c => c.slice(1, -1).replace(/\s/g, "").toUpperCase().split("||").some(s => { const on = /^!?\$(WIN32|WINDOWS)$/.test(s); return s[0] === "!" ? !on : on; });
    let p = 1;
    const block = () => {
      const out = { keys: {}, blocks: [] };
      while (p < toks.length) {
        const k = toks[p++];
        if (k === "}") break;
        if (k === "{") { block(); continue; }
        if (isCond(toks[p])) p++;
        const v = toks[p];
        if (v === "{") { p++; out.blocks.push([k.s.toLowerCase(), block()]); continue; }
        if (!v || v === "}") break;
        p++;
        if (isCond(toks[p]) && !holds(toks[p++].s)) continue;
        out.keys[k.s.toLowerCase()] = v.s;
      }
      return out;
    };
    const shader = toks[0] && toks[0].s !== undefined ? toks[0].s.toLowerCase() : "";
    if (toks[1] !== "{") return { shader, keys: {}, blocks: [] };
    p = 2;
    return { shader, ...block() };
  }
  function parseColor(v) {  // "[r g b]" (0 to 1) or "{r g b}" (0 to 255)
    if (!v) return null;
    const s = v.trim(), x = s.replace(/[[\]{}]/g, " ").trim().split(/\s+/).map(Number);
    if (x.length < 3 || x.slice(0, 3).some(c => !Number.isFinite(c))) return null;
    const k = s[0] === "{" ? 1 / 255 : 1;
    return x.slice(0, 3).map(c => Math.max(0, c * k));
  }
  function textureTransform(v) {  // "center cx cy scale sx sy rotate r translate tx ty"
    if (!v) return IDENTITY;
    const w = v.toLowerCase().split(/\s+/);
    const get = (key, d) => { const i = w.indexOf(key); return d.map((x, j) => { const y = i < 0 ? NaN : Number(w[i + 1 + j]); return Number.isFinite(y) ? y : x; }); };
    const [cx, cy] = get("center", [0.5, 0.5]), [sx, sy] = get("scale", [1, 1]), [r] = get("rotate", [0]), [tx, ty] = get("translate", [0, 0]);
    const a = r * Math.PI / 180, c = Math.cos(a), s = Math.sin(a);
    const m00 = c * sx, m01 = -s * sy, m10 = s * sx, m11 = c * sy;  // uv: rotated and scaled about the centre, then moved
    return [m00, m10, 0, m01, m11, 0, cx + tx - (m00 * cx + m01 * cy), cy + ty - (m10 * cx + m11 * cy), 1];
  }

  // ---- textures (.vtf): DXT ones go to the GPU as they are, the rest as RGBA
  function texture(t, path) {
    if (!t.cache.has(path)) {
      t.cache.set(path, (async () => {
        const buf = await mapFile(t, path);
        if (!buf || t.dead) return null;
        try { return uploadVTF(buf); } catch (e) { return null; }
      })());
    }
    return t.cache.get(path);
  }
  const DXT = { 13: ["dxt1", 8], 20: ["dxt1a", 8], 14: ["dxt3", 16], 15: ["dxt5", 16] };
  const BYTES = { 0: 4, 1: 4, 2: 3, 3: 3, 4: 2, 5: 1, 6: 2, 8: 1, 9: 3, 10: 3, 11: 4, 12: 4, 16: 4, 17: 2, 18: 2, 19: 2, 21: 2, 22: 2, 23: 4, 24: 8, 25: 8, 26: 4 };
  const imageSize = (f, w, h) => DXT[f] ? Math.max(1, (w + 3) >> 2) * Math.max(1, (h + 3) >> 2) * DXT[f][1] : w * h * (BYTES[f] || 0);
  function uploadVTF(buf) {
    const dv = new DataView(buf), u8 = new Uint8Array(buf);
    if (buf.byteLength < 80 || dv.getUint32(0, true) !== 0x00465456) throw new Error("not a texture");
    const minor = dv.getUint32(8, true), w = dv.getUint16(16, true), h = dv.getUint16(18, true), flags = dv.getUint32(20, true);
    const frames = Math.max(1, dv.getUint16(24, true)), first = dv.getUint16(26, true), fmt = dv.getInt32(52, true), mips = Math.max(1, u8[56]);
    const depth = minor >= 2 ? Math.max(1, dv.getUint16(63, true)) : 1;
    const faces = flags & 0x4000 ? (minor < 5 && first !== 0xffff ? 7 : 6) : 1;  // an environment map's sides
    let data = -1;
    if (minor >= 3) {  // the image is one of the resources listed after the header
      for (let k = 0, nr = dv.getUint32(68, true); k < nr && 88 + 8 * k <= buf.byteLength; k++) {
        const o = 80 + 8 * k;
        if (u8[o] === 0x30 && u8[o + 1] === 0 && u8[o + 2] === 0) data = dv.getUint32(o + 4, true);
      }
    } else {
      const lowFmt = dv.getInt32(57, true);
      data = dv.getUint32(12, true) + (lowFmt >= 0 ? imageSize(lowFmt, u8[61], u8[62]) : 0);  // after the thumbnail
    }
    if (data < 0 || !w || !h || !imageSize(fmt, w, h)) throw new Error("unsupported texture");
    const levels = [];  // largest first; stored smallest first, each with every frame, face and slice
    for (let m = mips - 1, o = data; m >= 0; m--) {
      const mw = Math.max(1, w >> m), mh = Math.max(1, h >> m), size = imageSize(fmt, mw, mh);
      levels[m] = [o, mw, mh, size];
      o += size * frames * faces * Math.max(1, depth >> m);
      if (o > buf.byteLength) throw new Error("truncated texture");
    }
    const gl = V.gl, tex = gl.createTexture(), dxt = DXT[fmt];
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    let mipmapped = mips > 1;
    if (dxt && V.s3tc && w % 4 === 0 && h % 4 === 0) {
      const s = V.s3tc, internal = { dxt1: s.COMPRESSED_RGB_S3TC_DXT1_EXT, dxt1a: s.COMPRESSED_RGBA_S3TC_DXT1_EXT, dxt3: s.COMPRESSED_RGBA_S3TC_DXT3_EXT, dxt5: s.COMPRESSED_RGBA_S3TC_DXT5_EXT }[dxt[0]];
      levels.forEach(([o, mw, mh, size], m) => gl.compressedTexImage2D(gl.TEXTURE_2D, m, internal, mw, mh, 0, new Uint8Array(buf, o, size)));
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAX_LEVEL, mips - 1);
    } else {
      const [o, , , size] = levels[0], src = new Uint8Array(buf, o, size);
      const px = dxt ? decodeDXT(dxt[0], src, w, h) : toRGBA(fmt, src, w, h);
      if (!px) { gl.deleteTexture(tex); throw new Error("unsupported texture format " + fmt); }
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, px);
      gl.generateMipmap(gl.TEXTURE_2D); mipmapped = true;
    }
    if (gl.getError() !== gl.NO_ERROR) { gl.deleteTexture(tex); throw new Error("the graphics card refused the texture"); }
    const point = flags & 0x1;  // point sampling (pixel art)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, point ? (mipmapped ? gl.NEAREST_MIPMAP_NEAREST : gl.NEAREST) : (mipmapped ? gl.LINEAR_MIPMAP_LINEAR : gl.LINEAR));
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, point ? gl.NEAREST : gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, flags & 0x4 ? gl.CLAMP_TO_EDGE : gl.REPEAT);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, flags & 0x8 ? gl.CLAMP_TO_EDGE : gl.REPEAT);
    if (V.aniso && mipmapped && !point) gl.texParameterf(gl.TEXTURE_2D, V.aniso.TEXTURE_MAX_ANISOTROPY_EXT, Math.min(8, gl.getParameter(V.aniso.MAX_TEXTURE_MAX_ANISOTROPY_EXT)));
    return { tex, w, h };
  }
  function decodeDXT(kind, src, w, h) {  // for a card without DXT support, or sizes it won't take
    const out = new Uint8Array(w * h * 4), bw = Math.max(1, (w + 3) >> 2), bh = Math.max(1, (h + 3) >> 2), bs = kind === "dxt1" || kind === "dxt1a" ? 8 : 16;
    const rgb = v => [(v >> 11 & 31) * 255 / 31, (v >> 5 & 63) * 255 / 63, (v & 31) * 255 / 31];
    for (let by = 0; by < bh; by++) for (let bx = 0; bx < bw; bx++) {
      const o = (by * bw + bx) * bs, c = o + bs - 8;
      if (c + 8 > src.length) return out;
      const c0 = src[c] | src[c + 1] << 8, c1 = src[c + 2] | src[c + 3] << 8, a = rgb(c0), b = rgb(c1), four = bs === 16 || c0 > c1;
      const pal = [a, b, a.map((x, i) => four ? (2 * x + b[i]) / 3 : (x + b[i]) / 2), four ? a.map((x, i) => (x + 2 * b[i]) / 3) : [0, 0, 0]];
      let alphas = null;
      if (kind === "dxt5") {
        const a0 = src[o], a1 = src[o + 1];
        alphas = a0 > a1 ? [a0, a1, ...[1, 2, 3, 4, 5, 6].map(k => ((7 - k) * a0 + k * a1) / 7)] : [a0, a1, ...[1, 2, 3, 4].map(k => ((5 - k) * a0 + k * a1) / 5), 0, 255];
      }
      const lo = src[o + 2] | src[o + 3] << 8 | src[o + 4] << 16, hi = src[o + 5] | src[o + 6] << 8 | src[o + 7] << 16;
      for (let k = 0; k < 16; k++) {
        const x = bx * 4 + (k & 3), y = by * 4 + (k >> 2);
        if (x >= w || y >= h) continue;
        const sel = src[c + 4 + (k >> 2)] >> ((k & 3) * 2) & 3, q = (y * w + x) * 4;
        out[q] = pal[sel][0]; out[q + 1] = pal[sel][1]; out[q + 2] = pal[sel][2];
        out[q + 3] = kind === "dxt1a" && !four && sel === 3 ? 0 : kind === "dxt3" ? (src[o + (k >> 1)] >> ((k & 1) * 4) & 15) * 17
          : alphas ? alphas[(k < 8 ? lo >> 3 * k : hi >> 3 * (k - 8)) & 7] : 255;
      }
    }
    return out;
  }
  const half = v => { const e = v >> 10 & 31, m = v & 1023; return (v >> 15 ? -1 : 1) * (e ? e === 31 ? (m ? NaN : Infinity) : (1 + m / 1024) * 2 ** (e - 15) : m * 2 ** -24); };
  function toRGBA(fmt, s, w, h) {  // the uncompressed formats, as RGBA bytes
    const n = w * h, o = new Uint8Array(n * 4), u16 = i => s[2 * i] | s[2 * i + 1] << 8;
    const each = f => { for (let i = 0; i < n; i++) f(i, i * 4); };
    const set = (q, r, g, b, a) => { o[q] = r; o[q + 1] = g; o[q + 2] = b; o[q + 3] = a; };
    switch (fmt) {
      case 0: case 23: case 26: o.set(s.subarray(0, n * 4)); break;                         // RGBA8888 (and normal maps)
      case 1: each((i, q) => set(q, s[4 * i + 3], s[4 * i + 2], s[4 * i + 1], s[4 * i])); break;  // ABGR8888
      case 2: case 9: each((i, q) => set(q, s[3 * i], s[3 * i + 1], s[3 * i + 2], 255)); break;   // RGB888
      case 3: case 10: each((i, q) => set(q, s[3 * i + 2], s[3 * i + 1], s[3 * i], 255)); break;  // BGR888
      case 4: each((i, q) => { const v = u16(i); set(q, (v & 31) * 255 / 31, (v >> 5 & 63) * 255 / 63, (v >> 11) * 255 / 31, 255); }); break;  // RGB565
      case 17: each((i, q) => { const v = u16(i); set(q, (v >> 11) * 255 / 31, (v >> 5 & 63) * 255 / 63, (v & 31) * 255 / 31, 255); }); break; // BGR565
      case 5: each((i, q) => set(q, s[i], s[i], s[i], 255)); break;                               // I8
      case 6: each((i, q) => set(q, s[2 * i], s[2 * i], s[2 * i], s[2 * i + 1])); break;          // IA88
      case 8: each((i, q) => set(q, 0, 0, 0, s[i])); break;                                       // A8
      case 11: each((i, q) => set(q, s[4 * i + 1], s[4 * i + 2], s[4 * i + 3], s[4 * i])); break; // ARGB8888
      case 12: each((i, q) => set(q, s[4 * i + 2], s[4 * i + 1], s[4 * i], s[4 * i + 3])); break; // BGRA8888
      case 16: each((i, q) => set(q, s[4 * i + 2], s[4 * i + 1], s[4 * i], 255)); break;          // BGRX8888
      case 18: case 21: each((i, q) => { const v = u16(i); set(q, (v >> 10 & 31) * 255 / 31, (v >> 5 & 31) * 255 / 31, (v & 31) * 255 / 31, fmt === 21 ? (v >> 15) * 255 : 255); }); break;
      case 19: each((i, q) => { const v = u16(i); set(q, (v >> 8 & 15) * 17, (v >> 4 & 15) * 17, (v & 15) * 17, (v >> 12) * 17); }); break;  // BGRA4444
      case 22: each((i, q) => set(q, s[2 * i], s[2 * i + 1], 0, 255)); break;                     // UV88
      case 24: each((i, q) => { const c = k => half(u16(4 * i + k)); set(q, ...[0, 1, 2].map(k => Math.pow(Math.min(1, Math.max(0, c(k))), 1 / 2.2) * 255), Math.min(1, Math.max(0, c(3))) * 255); }); break;  // RGBA16161616F (linear)
      case 25: each((i, q) => set(q, s[8 * i + 1], s[8 * i + 3], s[8 * i + 5], s[8 * i + 7])); break;  // RGBA16161616
      default: return null;
    }
    return o;
  }

  // ---------------------------------------------------------------- the sky and the props (with Textured)
  // What the map shows besides its brushes (/api/mapprops): its sky, six textures round everything
  // (materials/skybox/<skyname><side>.vmt), and its props, the models placed on it (static props and prop_*
  // entities), each from its .mdl (materials, skins), .vvd (vertices) and .vtx (triangles) as the game finds them.
  // The game lights models by what it works out round them, which the map's lightmaps don't hold: here a light
  // from above at about the map's brightness. Both load after the textured map, and show as they arrive.
  async function loadProps(t) {
    let info;
    try {
      const r = await fetch("/api/mapprops?name=" + encodeURIComponent(t.name));
      info = await r.json();
      if (!r.ok || info.error) throw new Error(info.error || `HTTP ${r.status}`);
    } catch (e) { t.propNote = "no sky or props: " + e.message; t.extrasDone = true; requestDraw(); return; }
    if (t.dead) return;
    const sky = loadSky(t, info.sky || "sky_urb01").catch(() => {});  // the game's own when the map names none
    const places = info.models.map(() => new Map());  // per model, per skin: the places (12 floats each)
    for (const [m, x, y, z, pitch, yaw, roll, skin] of info.props) {
      const bySkin = places[m]; if (!bySkin) continue;
      if (!bySkin.has(skin)) bySkin.set(skin, []);
      bySkin.get(skin).push(...placement(x, y, z, pitch, yaw, roll));
    }
    t.props = { groups: [], loaded: 0, missing: 0, total: info.models.length };
    const queue = info.models.map((path, i) => i);
    const worker = async () => {
      while (queue.length && !t.dead) {
        const i = queue.shift();
        let ok = false;
        try { ok = await loadModel(t, info.models[i], places[i]); } catch (e) { ok = false; }
        if (!ok) t.props.missing++;
        t.props.loaded++; requestDraw();
      }
    };
    await Promise.all([sky, ...[1, 2, 3, 4].map(worker)]);
    t.extrasDone = true; requestDraw();
  }
  // the sides in the game's order (Quake's tables): each its file's suffix, and how s, t in [-1, 1] across it make
  // its corners (an axis k > 0 is b[k - 1], k < 0 is -b[-k - 1], of b = [s, t, 1]): rt +x, lf -x, bk +y, ft -y
  const SKY = [["rt", [3, -1, 2]], ["lf", [-3, 1, 2]], ["bk", [1, 3, 2]], ["ft", [-1, -3, 2]], ["up", [-2, -1, 3]], ["dn", [2, -1, -3]]];
  function skyCube() {
    const gl = V.gl, v = [];
    for (const [, axes] of SKY) {
      for (const [s, t] of [[-1, -1], [1, -1], [1, 1], [-1, -1], [1, 1], [-1, 1]]) {
        const b = [s, t, 1];
        v.push(...axes.map(k => k > 0 ? b[k - 1] : -b[-k - 1]), (s + 1) / 2, 1 - (t + 1) / 2);
      }
    }
    const vao = gl.createVertexArray(); gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer()); gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(v), gl.STATIC_DRAW);
    for (const [n, size, off] of [["a_pos", 3, 0], ["a_uv", 2, 12]]) {
      const l = gl.getAttribLocation(V.skyProg, n);
      if (l >= 0) { gl.enableVertexAttribArray(l); gl.vertexAttribPointer(l, size, gl.FLOAT, false, 20, off); }
    }
    gl.bindVertexArray(null);
    return vao;
  }
  async function loadSky(t, name) {
    const sides = await Promise.all(SKY.map(async ([side]) => {
      const mat = await readMaterial(t, matPath(`skybox/${name}${side}`, "vmt"), 0);
      for (const k of ["$basetexture", "$hdrcompressedtexture", "$hdrbasetexture"]) {  // HDR only: its colours without the HDR scale
        const tx = mat && mat.params[k] ? await texture(t, matPath(mat.params[k], "vtf")) : null;
        if (!tx || t.dead) continue;
        const gl = V.gl; gl.bindTexture(gl.TEXTURE_2D, tx.tex);  // no seams at the edges
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE); gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        return { tex: tx.tex, half: side !== "up" && side !== "dn" && tx.h * 2 === tx.w ? 2 : 1 };
      }
      return null;
    }));
    if (!t.dead) { t.sky = sides; requestDraw(); }
  }
  // a prop's place: its rotation's columns (the game's AngleMatrix of pitch, yaw, roll) and its origin
  function placement(x, y, z, pitch, yaw, roll) {
    const [sp, cp, sy, cy, sr, cr] = [pitch, pitch, yaw, yaw, roll, roll].map((a, i) => (i % 2 ? Math.cos : Math.sin)(rad(a)));
    return [cp * cy, cp * sy, -sp, sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, sr * cp, cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp, x, y, z];
  }
  // A model on the GPU: its vertices and triangles once, and per skin its places and its meshes' materials.
  // false if its files aren't there or can't be read.
  async function loadModel(t, path, bySkin) {
    const base = path.replace(/\.mdl$/, "");
    const [mdl, vvd] = await Promise.all([mapFile(t, path), mapFile(t, base + ".vvd")]);
    let vtx = null;
    for (const ext of [".dx90.vtx", ".dx80.vtx", ".sw.vtx"]) if (mdl && vvd && !vtx) vtx = await mapFile(t, base + ext);
    if (!vtx || t.dead) return false;
    const md = readModel(mdl, vvd, vtx);
    if (!md.index.length) return false;
    const skins = [...bySkin.keys()], mats = await Promise.all(skins.map(skin => Promise.all(md.meshes.map(me => {
      const fam = md.skins[skin] || md.skins[0], k = fam && fam[me.slot] !== undefined ? fam[me.slot] : me.slot;
      return propMaterial(t, md.textures[k] || "", md.cds);
    }))));
    if (t.dead) return false;
    const gl = V.gl, P = V.propProg, vbo = gl.createBuffer(), ibo = gl.createBuffer();
    t.bufs.push(vbo, ibo);
    gl.bindVertexArray(null);
    gl.bindBuffer(gl.ARRAY_BUFFER, vbo); gl.bufferData(gl.ARRAY_BUFFER, md.verts, gl.STATIC_DRAW);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo); gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, md.index, gl.STATIC_DRAW);
    skins.forEach((skin, s) => {
      const at = new Float32Array(bySkin.get(skin)), inst = gl.createBuffer(), vao = gl.createVertexArray();
      t.bufs.push(inst); t.vaos.push(vao);
      gl.bindVertexArray(vao);
      gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
      for (const [n, size, off] of [["a_pos", 3, 0], ["a_nrm", 3, 12], ["a_uv", 2, 24]]) {
        const l = gl.getAttribLocation(P, n);
        if (l >= 0) { gl.enableVertexAttribArray(l); gl.vertexAttribPointer(l, size, gl.FLOAT, false, 32, off); }
      }
      gl.bindBuffer(gl.ARRAY_BUFFER, inst); gl.bufferData(gl.ARRAY_BUFFER, at, gl.STATIC_DRAW);
      for (const [n, off] of [["a_c0", 0], ["a_c1", 12], ["a_c2", 24], ["a_org", 36]]) {
        const l = gl.getAttribLocation(P, n);
        if (l >= 0) { gl.enableVertexAttribArray(l); gl.vertexAttribPointer(l, 3, gl.FLOAT, false, 48, off); gl.vertexAttribDivisor(l, 1); }
      }
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo);
      gl.bindVertexArray(null);
      t.props.groups.push({ vao, places: at.length / 12, meshes: md.meshes.map((me, k) => ({ first: me.first * 4, count: me.count, m: mats[s][k] })) });
    });
    return true;
  }
  // a model's material: from the first of the model's folders that has it (as the game looks), once per map; grey
  // if none has it
  function propMaterial(t, name, cds) {
    const key = [name, ...cds].join("\n");
    if (!t.propMats.has(key)) {
      t.propMats.set(key, (async () => {
        const m = { name: "", state: "missing", color: [0.6, 0.6, 0.6, 1], texel: [1, 1], xform: IDENTITY, tex: null, tex2: null, unlit: false, test: 0, mode: "opaque" };
        for (const cd of cds.length ? cds : [""]) {
          const path = matPath(cd + name, "vmt");
          if (!name || !await readMaterial(t, path, 0)) continue;
          m.name = path;
          try { await loadMaterial(t, m); } catch (e) { m.state = "missing"; }
          break;
        }
        return m;
      })());
    }
    return t.propMats.get(key);
  }
  // A model from its three files (Valve's studio.h and optimize.h): LOD 0 of its default body (each body part's
  // first model) as vertices (position, normal, texture position: 8 floats each) and triangles, per mesh its range
  // of them and its material slot; and its materials (names, and the folders to look in) and skins (per skin, per
  // slot the material).
  function readModel(mdl, vvd, vtx) {
    const M = new DataView(mdl), D = new DataView(vvd), X = new DataView(vtx), mu = new Uint8Array(mdl);
    const i32 = (dv, o) => dv.getInt32(o, true);
    if (i32(M, 0) !== 0x54534449 || i32(D, 0) !== 0x56534449 || i32(X, 0) !== 7) throw new Error("unsupported model");  // IDST, IDSV, version 7
    const str = o => { let e = o; while (e < mu.length && mu[e]) e++; return latin1(mu.subarray(o, e)); };
    const textures = [], cds = [], skins = [], refs = i32(M, 220);
    for (let k = 0, at = i32(M, 208); k < i32(M, 204); k++) textures.push(str(at + 64 * k + i32(M, at + 64 * k)));
    for (let k = 0, at = i32(M, 216); k < i32(M, 212); k++) cds.push(str(i32(M, at + 4 * k)));
    for (let f = 0, at = i32(M, 228); f < i32(M, 224); f++) {
      const s = []; for (let k = 0; k < refs; k++) s.push(M.getInt16(at + 2 * (f * refs + k), true)); skins.push(s);
    }
    // LOD 0's vertices: the file's, or as its fixups put them together
    const fixes = i32(D, 48), fixAt = i32(D, 52), vertAt = i32(D, 56), order = [];
    if (fixes > 0) {
      for (let k = 0; k < fixes; k++) { const o = fixAt + 12 * k; for (let j = 0, src = i32(D, o + 4); j < i32(D, o + 8); j++) order.push(src + j); }
    } else for (let k = 0; k < i32(D, 16); k++) order.push(k);
    const verts = new Float32Array(order.length * 8);
    order.forEach((src, k) => { for (let j = 0; j < 8; j++) verts[8 * k + j] = D.getFloat32(vertAt + 48 * src + 16 + 4 * j, true); });
    const index = [], meshes = [], nv = order.length;
    for (let b = 0, parts = Math.min(i32(M, 232), i32(X, 28)); b < parts; b++) {
      const bp = i32(M, 236) + 16 * b, xb = i32(X, 32) + 8 * b;
      if (i32(M, bp + 4) < 1 || i32(X, xb) < 1) continue;
      const mo = bp + i32(M, bp + 12), xm = xb + i32(X, xb + 4);  // the body part's first model
      if (i32(X, xm) < 1) continue;
      const xl = xm + i32(X, xm + 4), first = i32(M, mo + 84) / 48;  // its LOD 0; where its vertices start
      for (let k = 0, n = Math.min(i32(M, mo + 72), i32(X, xl)); k < n; k++) {
        const me = mo + i32(M, mo + 76) + 116 * k, xme = xl + i32(X, xl + 4) + 9 * k, start = index.length, off = first + i32(M, me + 12);
        for (let g = 0; g < i32(X, xme); g++) {
          const sg = xme + i32(X, xme + 4) + 25 * g, vAt = sg + i32(X, sg + 4), iAt = sg + i32(X, sg + 12);
          const vert = j => off + X.getUint16(vAt + 9 * X.getUint16(iAt + 2 * j, true) + 4, true);  // a strip group's vertex: the mesh's
          const tri = (a, b2, c) => { if (a < nv && b2 < nv && c < nv && a !== b2 && b2 !== c && a !== c) index.push(a, b2, c); };
          for (let s = 0; s < i32(X, sg + 16); s++) {
            const st = sg + i32(X, sg + 20) + 27 * s, i0 = i32(X, st + 4), ni = i32(X, st);
            if (X.getUint8(st + 18) & 2) for (let j = 2; j < ni; j++) tri(vert(i0 + j - 2), vert(i0 + j - 1), vert(i0 + j));  // a strip
            else for (let j = 0; j + 2 < ni; j += 3) tri(vert(i0 + j), vert(i0 + j + 1), vert(i0 + j + 2));
          }
        }
        meshes.push({ slot: i32(M, me), first: start, count: index.length - start });
      }
    }
    return { verts, index: Uint32Array.from(index), meshes, textures, cds, skins };
  }
  function drawSky(sky, eye) {
    const gl = V.gl, u = V.skyU;
    gl.disable(gl.DEPTH_TEST); gl.depthMask(false);
    gl.useProgram(V.skyProg);
    gl.uniformMatrix4fv(u.u_mvp, false, V.mvp); gl.uniform3fv(u.u_eye, eye); gl.uniform1i(u.u_tex, 0);
    gl.activeTexture(gl.TEXTURE0); gl.bindVertexArray(V.skyVao);
    sky.forEach((side, k) => { if (side) { gl.bindTexture(gl.TEXTURE_2D, side.tex); gl.uniform1f(u.u_half, side.half); gl.drawArrays(gl.TRIANGLES, k * 6, 6); } });
    gl.depthMask(true); gl.enable(gl.DEPTH_TEST);
  }
  function drawProps(T, seeThrough) {
    const gl = V.gl, u = V.propU;
    gl.useProgram(V.propProg);
    gl.uniformMatrix4fv(u.u_mvp, false, V.mvp); gl.uniform1i(u.u_base, 0); gl.uniform1f(u.u_level, T.level);
    gl.activeTexture(gl.TEXTURE0);
    for (const g of T.props.groups) {
      gl.bindVertexArray(g.vao);
      for (const me of g.meshes) {
        const m = me.m;
        if (m.mode === "hidden" || (m.mode !== "opaque") !== seeThrough || !me.count) continue;
        if (seeThrough) blendFor(m);
        gl.bindTexture(gl.TEXTURE_2D, m.tex || V.white);
        gl.uniformMatrix3fv(u.u_xform, false, m.xform); gl.uniform4fv(u.u_color, m.color);
        gl.uniform4f(u.u_opt, 0, m.unlit ? 1 : 0, m.test, m.mode === "blend" ? 1 : 0);
        gl.drawElementsInstanced(gl.TRIANGLES, me.count, gl.UNSIGNED_INT, me.first, g.places);
      }
    }
  }
  // how far the sky and the props have come, for the view's line
  function propTip(T) {
    const P = T.props;
    return T.propNote ? ` · ${T.propNote}` : !P ? "" : P.loaded < P.total ? ` · props ${P.loaded}/${P.total}` : P.missing ? ` · ${P.missing} of ${P.total} prop models not found` : "";
  }

  // ---------------------------------------------------------------- triggers (Triggers)
  // The map's triggers as see-through volumes coloured by type (/api/maptrig: each trigger's brushes as
  // polygons), only the types chosen in the menu beside the button. Pointing at one shows its type and name.
  const TRIGGER_COLOURS = {
    trigger_teleport: [0.72, 0.42, 1], trigger_teleport_relative: [0.85, 0.55, 1], trigger_push: [0.25, 0.95, 0.45],
    trigger_hurt: [1, 0.28, 0.22], trigger_multiple: [1, 0.7, 0.2], trigger_once: [1, 0.92, 0.3], trigger_gravity: [0.3, 0.85, 1],
    trigger_soundscape: [0.62, 0.64, 0.68], func_buyzone: [0.45, 0.85, 0.65], func_bomb_target: [1, 0.5, 0.35], func_hostage_rescue: [0.5, 0.75, 1],
  };
  function triggerColour(cls) {  // others: a colour from the name, the same every time
    if (TRIGGER_COLOURS[cls]) return TRIGGER_COLOURS[cls];
    let h = 0; for (const ch of cls) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
    const hue = (h % 360) / 60, x = 1 - Math.abs(hue % 2 - 1);
    return [[1, x, 0], [x, 1, 0], [0, 1, x], [0, x, 1], [x, 0, 1], [1, 0, x]][Math.floor(hue) % 6].map(v => 0.35 + 0.6 * v);
  }
  const trigShown = cls => !VIS.v3dtrigOff.includes(cls);
  async function loadTriggers(name) {
    V.trigLoading = name; requestDraw();
    let t;
    try {
      const r = await fetch("/api/maptrig?name=" + encodeURIComponent(name));
      if (!r.ok) { const j = await r.json().catch(() => ({})); throw new Error(j.error || `HTTP ${r.status}`); }
      t = readTriggers(await r.arrayBuffer());
    } catch (e) { t = { error: e.message }; }
    t.name = name; V.trigLoading = null;
    if (V.mapName === name) { disposeTriggers(V.trig); V.trig = t; if (!trigs.hidden) triggerMenu(); }
    requestDraw();
  }
  function readTriggers(buf) {  // the TATR data (see tasurf/bspmap.py)
    const dv = new DataView(buf), u8 = new Uint8Array(buf);
    if (String.fromCharCode(...u8.subarray(0, 4)) !== "TATR" || dv.getUint32(4, true) !== 1) throw new Error("unexpected trigger data");
    const [nt, np, nv] = [8, 12, 16].map(o => dv.getUint32(o, true));
    const list = [], classes = new Map();
    let o = 20 + 16 * nt;
    for (let k = 0; k < nt; k++) {
      const [first, count, lc, ln] = [0, 4, 8, 12].map(i => dv.getUint32(20 + 16 * k + i, true));
      const cls = latin1(u8.subarray(o, o + lc)), name = latin1(u8.subarray(o + lc, o + lc + ln));
      o += lc + ln;
      list.push({ cls, name, first, count, colour: triggerColour(cls) });
      classes.set(cls, (classes.get(cls) || 0) + 1);
    }
    o = Math.ceil(o / 4) * 4;
    const counts = new Uint32Array(buf, o, np), pos = new Float32Array(buf, o + 4 * np, nv * 3), start = new Uint32Array(np + 1);
    for (let k = 0; k < np; k++) start[k + 1] = start[k] + counts[k];
    for (const tr of list) {  // its bounds, to find the one pointed at quickly
      tr.lo = [Infinity, Infinity, Infinity]; tr.hi = [-Infinity, -Infinity, -Infinity];
      for (let v = 3 * start[tr.first]; v < 3 * start[tr.first + tr.count]; v += 3) {
        for (let i = 0; i < 3; i++) { tr.lo[i] = Math.min(tr.lo[i], pos[v + i]); tr.hi[i] = Math.max(tr.hi[i], pos[v + i]); }
      }
    }
    return { list, classes, start, pos, key: null, shown: [] };
  }
  // what's drawn: built again only when the chosen types change
  function triggerGeometry(T) {
    const key = VIS.v3dtrigOff.join("\n");
    if (key === T.key) return;
    T.key = key; T.shown = T.list.filter(tr => trigShown(tr.cls));
    const gl = V.gl, { start, pos } = T, P = v => [pos[3 * v], pos[3 * v + 1], pos[3 * v + 2]];
    let tris = 0, segs = 0;
    for (const tr of T.shown) for (let p = tr.first; p < tr.first + tr.count; p++) { tris += start[p + 1] - start[p] - 2; segs += start[p + 1] - start[p]; }
    const fill = new Float32Array(tris * 3 * 7), edges = new Float32Array(segs * 4 * LF);
    let f = 0, e = 0;
    for (const tr of T.shown) {
      const face = [...tr.colour, 0.16], edge = [...tr.colour, 0.9];
      for (let p = tr.first; p < tr.first + tr.count; p++) {
        const a = start[p], n = start[p + 1] - a;
        for (let k = 1; k + 1 < n; k++) for (const v of [a, a + k, a + k + 1]) { fill.set(P(v), f); fill.set(face, f + 3); f += 7; }
        for (let k = 0; k < n; k++) putSeg(edges, e++, P(a + k), P(a + (k + 1) % n), edge, 1.5, -1);
      }
    }
    if (!T.fill) {
      T.fill = { buf: gl.createBuffer(), vao: gl.createVertexArray(), count: 0 };
      gl.bindVertexArray(T.fill.vao); gl.bindBuffer(gl.ARRAY_BUFFER, T.fill.buf);
      for (const [n, size, off] of [["a_pos", 3, 0], ["a_col", 4, 12]]) {
        const l = gl.getAttribLocation(V.trigProg, n);
        if (l >= 0) { gl.enableVertexAttribArray(l); gl.vertexAttribPointer(l, size, gl.FLOAT, false, 28, off); }
      }
      gl.bindVertexArray(null);
      T.edges = lineSlot();
    }
    gl.bindBuffer(gl.ARRAY_BUFFER, T.fill.buf); gl.bufferData(gl.ARRAY_BUFFER, fill, gl.STATIC_DRAW); T.fill.count = tris * 3;
    fillSlot(T.edges, edges, gl.STATIC_DRAW);
  }
  function disposeTriggers(T) {
    if (!T || !T.fill) return;
    const gl = V.gl;
    gl.deleteBuffer(T.fill.buf); gl.deleteVertexArray(T.fill.vao); gl.deleteBuffer(T.edges.buf); gl.deleteVertexArray(T.edges.vao);
  }
  function triggerMenu() {
    const T = V.trig && V.trig.name === V.mapName ? V.trig : null;
    if (!T || T.error) {
      if (!T && V.map && !V.trigLoading) loadTriggers(V.mapName);
      trigs.innerHTML = `<span class="grp">${T ? "no triggers: " + esc(T.error) : V.map ? "loading the triggers…" : "open a timeline to see its map"}</span>`;
      return;
    }
    const rows = [...T.classes].sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1));
    trigs.innerHTML = `<span class="grp">Triggers in ${esc(V.mapName)}</span>` + (!rows.length ? `<span class="grp">none</span>` :
      rows.map(([c, n]) => `<label><input type="checkbox" data-cls="${esc(c)}"${trigShown(c) ? " checked" : ""}>` +
        `<span class="sw" style="background: rgb(${triggerColour(c).map(v => Math.round(v * 255))})"></span>${esc(c)}<span class="n">${n}</span></label>`).join("") +
      `<div class="popfoot"><span class="seg"><button data-all="1">All</button><button data-all="0">None</button></span></div>`);
  }
  // pointing at a trigger names it: the nearest one the ray through the mouse hits (checked once a frame)
  cv.addEventListener("mousemove", e => {
    if (!VIS.v3dtrig || V.drag || V.look) { label.hidden = true; return; }
    const r = cv.getBoundingClientRect();
    V.mouse = [e.clientX - r.left, e.clientY - r.top, r.width, r.height];
    V.pickWait = V.pickWait || later(pointTrigger, 50, () => { V.pickWait = null; });
  });
  cv.addEventListener("mouseleave", () => { label.hidden = true; });
  function pointTrigger() {
    const T = V.trig, m = V.mouse, c = V.cam;
    if (!VIS.v3dtrig || !T || !T.shown || !m || !c || T.name !== V.mapName) { label.hidden = true; return; }
    const { f, s, u } = basis(c), x = (m[0] / m[2]) * 2 - 1, y = 1 - (m[1] / m[3]) * 2, k = 0.75;  // k: tan of half the vertical view
    const d = [0, 1, 2].map(i => f[i] + s[i] * x * k * c.aspect + u[i] * y * k);
    let best = null, near = Infinity;
    for (const tr of T.shown) {
      if (!rayHitsBox(c.eye, d, tr.lo, tr.hi, near)) continue;
      for (let p = tr.first; p < tr.first + tr.count; p++) { const t = rayHitsPolygon(c.eye, d, T, p); if (t < near) { near = t; best = tr; } }
    }
    if (!best) { label.hidden = true; return; }
    label.textContent = best.name ? `${best.cls} · ${best.name}` : best.cls;
    label.style.left = `${m[0] + 14}px`; label.style.top = `${m[1] + 12}px`; label.hidden = false;
  }
  function rayHitsBox(o, d, lo, hi, far) {
    let t0 = 0, t1 = far;
    for (let i = 0; i < 3; i++) {
      let a = (lo[i] - o[i]) / d[i], b = (hi[i] - o[i]) / d[i];
      if (a > b) [a, b] = [b, a];
      t0 = Math.max(t0, a); t1 = Math.min(t1, b);
      if (t0 > t1) return false;
    }
    return true;
  }
  function rayHitsPolygon(o, d, T, p) {  // how far along the ray it hits the (convex) polygon; Infinity if it doesn't
    const a = T.start[p], n = T.start[p + 1] - a, q = T.pos, v = i => 3 * (a + i % n);
    let nx = 0, ny = 0, nz = 0;  // its normal (Newell's method)
    for (let i = 0; i < n; i++) {
      const j = v(i), k = v(i + 1);
      nx += (q[j + 1] - q[k + 1]) * (q[j + 2] + q[k + 2]); ny += (q[j + 2] - q[k + 2]) * (q[j] + q[k]); nz += (q[j] - q[k]) * (q[j + 1] + q[k + 1]);
    }
    const den = nx * d[0] + ny * d[1] + nz * d[2];
    if (Math.abs(den) < 1e-9) return Infinity;
    const j0 = v(0), t = (nx * (q[j0] - o[0]) + ny * (q[j0 + 1] - o[1]) + nz * (q[j0 + 2] - o[2])) / den;
    if (!(t > 0)) return Infinity;
    const h = [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
    for (let i = 0; i < n; i++) {  // on the inner side of every edge
      const j = v(i), k = v(i + 1), ex = q[k] - q[j], ey = q[k + 1] - q[j + 1], ez = q[k + 2] - q[j + 2];
      const hx = h[0] - q[j], hy = h[1] - q[j + 1], hz = h[2] - q[j + 2];
      if ((ey * hz - ez * hy) * nx + (ez * hx - ex * hz) * ny + (ex * hy - ey * hx) * nz < 0) return Infinity;
    }
    return t;
  }

  // ---------------------------------------------------------------- what to draw
  function boxRun() { return pvCurrent() ? S.pv : newestPath(); }
  // The editor's tick, or while playing the exact time between ticks (the page's playTime).
  function now() { const ft = window.playTime ? playTime() : null; return ft === null ? S.tick : ft; }
  // The player at that time, from the run (or the fresh preview). Between two ticks the position and the
  // view are blended, as the game draws between its ticks; never across a teleport.
  function player() {
    const r = boxRun(), ft = now(), t = Math.floor(ft), f = ft - t;
    if (!r || t >= r.x.length) return null;
    // the pitch replays show (the page's view pitch, else the run's own)
    const pitchAt = i => typeof replayPitchAt === "function" ? replayPitchAt(i)
      : S.preview && S.preview.pitch ? S.preview.pitch[Math.min(i, S.preview.pitch.length - 1)] : 0;
    // the yaw replays show: the camera, auto-strafe's zigzag smoothed (the page's cameraYaws)
    const yaws = typeof cameraYaws === "function" ? cameraYaws(r) : r.yaw;
    let pos = [r.x[t], r.y[t], r.z[t]], yaw = yaws ? yaws[t] : 0, pitch = pitchAt(t);
    if (f > 0 && t + 1 < r.x.length) {
      const next = [r.x[t + 1], r.y[t + 1], r.z[t + 1]];
      if (Math.hypot(next[0] - pos[0], next[1] - pos[1], next[2] - pos[2]) <= (r.tick_interval || 0.015) * 3500 * 2.5) {
        pos = pos.map((p, i) => p + (next[i] - p) * f);
        if (yaws) yaw += (((yaws[t + 1] - yaw) % 360 + 540) % 360 - 180) * f;  // the short way round
        pitch += (pitchAt(t + 1) - pitch) * f;
      }
    }
    return { pos, yaw, pitch, ducked: !!(r.ducked && r.ducked[t]) };
  }
  const STOPS = [[0, 64, 255], [0, 255, 255], [0, 255, 0], [255, 255, 0], [255, 0, 0]];
  function speedRGB(v, lo, hi) {  // the same scale as the top view's speedColor
    const t = hi > lo ? Math.min(1, Math.max(0, (v - lo) / (hi - lo))) : 0;
    const s = t * 4, k = Math.min(3, Math.floor(s)), f = s - k;
    return STOPS[k].map((a, i) => (a + (STOPS[k + 1][i] - a) * f) / 255);
  }
  // a ribbon segment a-b into v at segment slot i: 4 vertices of LF floats; tick -1 is never dimmed
  function putSeg(v, i, a, b, col, width, tick) {
    let o = i * 4 * LF;
    for (const [p, q, s] of [[a, b, 1], [a, b, -1], [b, a, -1], [b, a, 1]]) {
      v[o] = p[0]; v[o + 1] = p[1]; v[o + 2] = p[2]; v[o + 3] = q[0]; v[o + 4] = q[1]; v[o + 5] = q[2]; v[o + 6] = s;
      v[o + 7] = col[0]; v[o + 8] = col[1]; v[o + 9] = col[2]; v[o + 10] = col[3]; v[o + 11] = width; v[o + 12] = tick;
      o += LF;
    }
  }
  // The paths: built again only when the runs or what's shown change, not when the tick or the camera moves.
  function buildPaths() {
    const fresh = pvCurrent(), e = guess();  // the instant prediction, or the server's estimate
    const hits = pathHits();  // speed lost hitting things: red posts, taller for more
    const key = [S.run, S.ghost, S.pv, e && e.x, e && e.x.length, fresh, VIS.run, VIS.ghost, VIS.est, hits];
    if (V.pathKey && key.every((k, i) => k === V.pathKey[i])) return;
    V.pathKey = key;
    const jump = r => (r.tick_interval || 0.015) * 3500 * 2.5;  // further in a tick than any speed: a teleport
    const parts = [];  // [run, colour (fixed or from the speed), width, dimmed after the tick]
    if (S.ghost && !fresh && VIS.ghost) parts.push([S.ghost, [0.6, 0.6, 0.6, 0.55], 1.5, false]);
    if (S.run && VIS.run) parts.push(fresh ? [S.run, [0.67, 0.67, 0.67, 0.6], 1.5, false] : [S.run, null, 3, true]);
    if (S.pv) parts.push(fresh ? [S.pv, null, 3.5, true] : [S.pv, [1, 1, 1, 0.4], 1.5, false]);
    // the prediction in speed colours (the estimate in orange)
    if (e && !fresh && VIS.est) parts.push([{ x: e.x, y: e.y, z: e.z, hspeed: e.hspeed, tick_interval: S.run && S.run.tick_interval },
      e.predicted ? null : [1, 0.7, 0.28, 1], 3, false]);
    const r0 = boxRun();
    let n = (r0 && r0.x.length ? 1 : 0) + hits.length;
    for (const [r] of parts) n += Math.max(0, r.x.length - 1);
    const v = new Float32Array(n * 4 * LF);
    let i = 0;
    for (const [r, col, width, dimmed] of parts) {
      const lo = col ? 0 : Math.min(...r.hspeed), hi = col ? 0 : Math.max(...r.hspeed), lim = jump(r);
      for (let t = 1; t < r.x.length; t++) {
        const a = [r.x[t - 1], r.y[t - 1], r.z[t - 1]], b = [r.x[t], r.y[t], r.z[t]];
        if (Math.hypot(b[0] - a[0], b[1] - a[1], b[2] - a[2]) > lim) continue;
        putSeg(v, i++, a, b, col || [...speedRGB(r.hspeed[t], lo, hi), 1], width, dimmed ? t : -1);
      }
    }
    if (r0 && r0.x.length) {  // the start: a green post
      const p = [r0.x[0], r0.y[0], r0.z[0]];
      putSeg(v, i++, p, [p[0], p[1], p[2] + 96], [0.35, 0.85, 0.55, 1], 3, -1);
    }
    for (const h of hits) putSeg(v, i++, [h.x, h.y, h.z], [h.x, h.y, h.z + Math.min(80, 10 + Math.sqrt(h.lost) * 4)], [1, 0.31, 0.31, 1], 2.5, -1);
    fillSlot(V.paths, v.subarray(0, i * 4 * LF), V.gl.STATIC_DRAW);
  }
  const BOX_EDGES = [[[0, 0, 0], [1, 0, 0]], [[1, 0, 0], [1, 1, 0]], [[1, 1, 0], [0, 1, 0]], [[0, 1, 0], [0, 0, 0]],
    [[0, 0, 1], [1, 0, 1]], [[1, 0, 1], [1, 1, 1]], [[1, 1, 1], [0, 1, 1]], [[0, 1, 1], [0, 0, 1]],
    [[0, 0, 0], [0, 0, 1]], [[1, 0, 0], [1, 0, 1]], [[1, 1, 0], [1, 1, 1]], [[0, 1, 0], [0, 1, 1]]];
  const markVerts = new Float32Array(13 * 4 * LF);
  function buildMarks() {  // the player's box (32 x 32 x 72, 54 ducked) at the tick and where it looks
    const pl = player();
    if (!pl || !VIS.box || V.mode === "eye") { V.marks.count = 0; V.markKey = null; return; }
    const key = `${pl.pos}|${pl.yaw}|${pl.pitch}|${pl.ducked}`;
    if (key === V.markKey) return;
    V.markKey = key;
    const [x, y, z] = pl.pos, h = pl.ducked ? 54 : 72, c = [1, 0.82, 0.4, 1];
    const P = (i, j, k) => [x + (i ? 16 : -16), y + (j ? 16 : -16), z + (k ? h : 0)];
    BOX_EDGES.forEach(([a, b], i) => putSeg(markVerts, i, P(...a), P(...b), c, 2, -1));
    const eye = [x, y, z + (pl.ducked ? 46 : 64)], f = dir(pl.yaw, pl.pitch);
    putSeg(markVerts, 12, eye, [eye[0] + f[0] * 64, eye[1] + f[1] * 64, eye[2] + f[2] * 64], [1, 0.55, 0.2, 1], 2.5, -1);
    fillSlot(V.marks, markVerts, V.gl.DYNAMIC_DRAW);
  }

  // The hand-made inputs on the path (the page's marks, see "the inputs on the path" in index.html): a yaw
  // keyframe as a post with an arrow on top pointing its yaw, a key going down as a shorter post in the key's
  // colour, coming up as a faint one. Built again when the inputs, the path or what's lit change.
  const POST = { yaw: 56, tap: 36, press: 28, release: 22 };
  const hexRGB = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16) / 255);
  function buildInputMarks() {
    const path = S.norm && VIS.marks ? markPath() : null;
    const key = [path, S.norm && S.norm.yaw, S.norm && S.norm.keys, S.sel, PM.ver];
    if (V.inputKey && key.every((k, i) => k === V.inputKey[i])) return;
    V.inputKey = key; V.inputPts = [];
    if (!path || !path.x.length) { V.inputs.count = 0; return; }
    const last = new Map(), segs = [];
    for (const m of pathMarks(path)) {
      const t = m.tick, foot = [path.x[t], path.y[t], path.z[t]], lit = markLit(m);
      const group = m.kind === "yaw" ? "" : m.range.key, prev = last.get(group);
      if (!lit && prev && Math.hypot(foot[0] - prev[0], foot[1] - prev[1], foot[2] - prev[2]) < 48) continue;  // crowded: every few
      last.set(group, foot);
      const top = [foot[0], foot[1], foot[2] + POST[m.kind]], w = lit ? 4 : 2.5;
      const col = [...hexRGB(markSelected(m) ? "#ffd166" : markColor(m)), m.kind === "release" ? 0.55 : 1];
      segs.push([foot, top, col, w]);
      const pt = { m, top };
      if (m.kind === "yaw") {
        const a = m.key[1] * Math.PI / 180, ux = Math.cos(a), uy = Math.sin(a), tip = [top[0] + ux * 48, top[1] + uy * 48, top[2]];
        segs.push([top, tip, col, w],
          [tip, [tip[0] - ux * 12 - uy * 8, tip[1] - uy * 12 + ux * 8, tip[2]], col, w],
          [tip, [tip[0] - ux * 12 + uy * 8, tip[1] - uy * 12 - ux * 8, tip[2]], col, w]);
        pt.tip = tip;
      }
      V.inputPts.push(pt);
    }
    const v = new Float32Array(segs.length * 4 * LF);
    segs.forEach(([a, b, c, w], i) => putSeg(v, i, a, b, c, w, -1));
    fillSlot(V.inputs, v, V.gl.DYNAMIC_DRAW);
  }
  // where the marks are on screen now, for markAt
  function inputHits() {
    const out = [];
    for (const p of V.inputPts || []) {
      const q = toScreen(p.top); if (!q) continue;
      const h = { m: p.m, x: q[0], y: q[1] };
      const r = p.tip && toScreen(p.tip);
      if (r) { h.tx = r[0]; h.ty = r[1]; }
      out.push(h);
    }
    return out;
  }

  // ---------------------------------------------------------------- camera
  const rad = d => d * Math.PI / 180;
  function dir(yaw, pitch) {  // Source angles: yaw from +x towards +y, pitch positive looks down
    const y = rad(yaw), p = rad(pitch);
    return [Math.cos(p) * Math.cos(y), Math.cos(p) * Math.sin(y), -Math.sin(p)];
  }
  function overview() {
    const m = V.map; if (!m) return;
    const c = [0, 1, 2].map(i => (m.mins[i] + m.maxs[i]) / 2);
    const size = Math.hypot(m.maxs[0] - m.mins[0], m.maxs[1] - m.mins[1]);
    V.free.yaw = 45; V.free.pitch = 35;
    const f = dir(V.free.yaw, V.free.pitch);
    V.free.pos = [c[0] - f[0] * size * 0.6, c[1] - f[1] * size * 0.6, c[2] - f[2] * size * 0.6];
  }
  function camera() {
    const pl = player();
    if (V.mode === "eye" && pl) {
      const eye = [pl.pos[0], pl.pos[1], pl.pos[2] + (pl.ducked ? 46 : 64)];
      return { eye, yaw: pl.yaw, pitch: pl.pitch };
    }
    if (V.mode === "follow" && pl) {
      if (V.orbit.yaw === null) V.orbit.yaw = pl.yaw;  // start behind the player
      const f = dir(V.orbit.yaw, V.orbit.pitch), t = [pl.pos[0], pl.pos[1], pl.pos[2] + 36];
      return { eye: [t[0] - f[0] * V.orbit.dist, t[1] - f[1] * V.orbit.dist, t[2] - f[2] * V.orbit.dist], yaw: V.orbit.yaw, pitch: V.orbit.pitch };
    }
    if (!pl && V.mode !== "free" && V.map && !V.freeSet) { overview(); V.freeSet = true; }
    return { eye: V.free.pos, yaw: V.free.yaw, pitch: V.free.pitch };
  }
  function basis(c) {  // the camera's forward, right and up
    const f = dir(c.yaw, c.pitch), up = [0, 0, 1];
    let s = [f[1] * up[2] - f[2] * up[1], f[2] * up[0] - f[0] * up[2], f[0] * up[1] - f[1] * up[0]];
    const sl = Math.hypot(...s); s = sl > 1e-6 ? s.map(x => x / sl) : [0, -1, 0];
    return { f, s, u: [s[1] * f[2] - s[2] * f[1], s[2] * f[0] - s[0] * f[2], s[0] * f[1] - s[1] * f[0]] };
  }
  function matrices(c, aspect) {
    const { f, s, u } = basis(c);
    const e = c.eye, dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    const view = [s[0], u[0], -f[0], 0, s[1], u[1], -f[1], 0, s[2], u[2], -f[2], 0, -dot(s, e), -dot(u, e), dot(f, e), 1];
    // the game's field of view: 90 degrees across at 4:3
    const fy = 1 / Math.tan(Math.atan(Math.tan(rad(45)) * 3 / 4)), near = 4, far = 65536, nf = 1 / (near - far);
    const proj = [fy / aspect, 0, 0, 0, 0, fy, 0, 0, 0, 0, (far + near) * nf, -1, 0, 0, 2 * far * near * nf, 0];
    const m = new Float32Array(16);
    for (let col = 0; col < 4; col++) for (let row = 0; row < 4; row++) {
      let v = 0; for (let k = 0; k < 4; k++) v += proj[k * 4 + row] * view[col * 4 + k]; m[col * 4 + row] = v;
    }
    return m;
  }

  // ---------------------------------------------------------------- drawing
  function draw() {
    if (!V.on || !VIS.open) return;
    init();
    const want = S.norm && S.norm.map;
    if (V.gl && want && want !== V.mapName) loadMap(want);
    const T = VIS.v3dtex && V.map && V.tex && V.tex.name === V.mapName ? V.tex : null;
    if (VIS.v3dtex && V.map && !V.texLoading && !T) loadTextured(V.mapName);
    const TR = VIS.v3dtrig && V.map && V.trig && V.trig.name === V.mapName && !V.trig.error ? V.trig : null;
    if (VIS.v3dtrig && V.map && !V.trigLoading && !(V.trig && V.trig.name === V.mapName)) loadTriggers(V.mapName);
    if (TR && V.gl) triggerGeometry(TR);
    if (!VIS.v3dtrig) label.hidden = true;
    const trigTip = !VIS.v3dtrig || !V.map ? "" : V.trigLoading ? " · loading the triggers…" : V.trig && V.trig.error ? ` · no triggers: ${V.trig.error}` :
      TR ? ` · ${TR.shown.length} of ${TR.list.length} triggers` : "";
    const msgText = V.error || (V.loading ? `loading ${V.loading}…` : want ? "" : "open a timeline to see its map");
    const texTip = !T ? (VIS.v3dtex && V.texLoading ? " · loading the textures…" : "") : T.error ? ` · no textures: ${T.error}` :
      (T.loaded < T.used.length ? ` · textures ${T.loaded}/${T.used.length}` : T.missing ? ` · ${T.missing} of ${T.used.length} materials not found` : "") + (T.note ? ` · ${T.note}` : "") + propTip(T);
    const tip = V.map ? `${V.mapName} · ${Math.round(V.map.tris).toLocaleString()} triangles` + texTip + trigTip +
      (V.mode === "free" ? ` · hold right mouse to look, WASD to fly · ${Math.round(V.free.speed)} u/s` : "") + (!player() ? " · no run at this tick" : "") : "";
    const ui = `${msgText}\n${tip}\n${V.mode}\n${!!VIS.v3dtex}\n${!!VIS.v3dtrig}`;
    if (ui !== V.ui) {  // only when something in it changed
      V.ui = ui;
      msg.hidden = !msgText; msg.textContent = msgText;
      $("v3dtip").textContent = tip;
      bar.querySelectorAll("[data-m]").forEach(b => b.classList.toggle("on", b.dataset.m === V.mode));
      bar.querySelector("[data-look=tex]").classList.toggle("on", !!VIS.v3dtex);
      bar.querySelector("[data-look=trig]").classList.toggle("on", !!VIS.v3dtrig);
    }
    const gl = V.gl; if (!gl) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1), w = Math.max(1, Math.round(cv.clientWidth * dpr)), h = Math.max(1, Math.round(cv.clientHeight * dpr));
    if (cv.width !== w || cv.height !== h) { cv.width = w; cv.height = h; }
    gl.viewport(0, 0, w, h);
    gl.clearColor(0.08, 0.09, 0.11, 1); gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    const c = camera();
    V.mvp = matrices(c, w / h);
    V.cam = { ...c, aspect: w / h };
    gl.enable(gl.DEPTH_TEST);
    if (V.map) {
      // the backs of surfaces are left out: from behind a wall you see through it, as in Hammer
      gl.enable(gl.CULL_FACE); gl.cullFace(gl.BACK);
      if (T && !T.error) drawTextured(T, c.eye);
      else {
        gl.useProgram(V.mapProg);
        gl.uniformMatrix4fv(V.mapU.u_mvp, false, V.mvp);
        gl.uniform3fv(V.mapU.u_eye, c.eye);
        gl.bindVertexArray(V.map.vao);
        gl.drawElements(gl.TRIANGLES, V.map.count, gl.UNSIGNED_INT, 0);
      }
      gl.disable(gl.CULL_FACE);
      if (TR) {  // see from inside and out, over the map, without hiding what's behind
        gl.enable(gl.BLEND); gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA); gl.depthMask(false);
        gl.useProgram(V.trigProg); gl.uniformMatrix4fv(V.trigU.u_mvp, false, V.mvp);
        gl.bindVertexArray(TR.fill.vao); gl.drawArrays(gl.TRIANGLES, 0, TR.fill.count);
        gl.depthMask(true); gl.disable(gl.BLEND);
      }
    }
    buildPaths(); buildMarks(); buildInputMarks();
    gl.enable(gl.BLEND); gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.useProgram(V.lineProg);
    gl.uniformMatrix4fv(V.lineU.u_mvp, false, V.mvp);
    gl.uniform2f(V.lineU.u_half, w / 2, h / 2);
    gl.uniform1f(V.lineU.u_dpr, dpr);
    gl.uniform1f(V.lineU.u_tick, now());
    // twice: faintly through the map (so the run is never lost behind a wall), then where it's in view
    for (const [depth, alpha] of [[false, 0.3], [true, 1]]) {
      if (depth) gl.enable(gl.DEPTH_TEST); else gl.disable(gl.DEPTH_TEST);
      gl.uniform1f(V.lineU.u_alpha, alpha);
      for (const slot of depth && TR ? [V.paths, V.inputs, V.marks, TR.edges] : [V.paths, V.inputs, V.marks]) {  // trigger edges: only where in view
        if (!slot.count) continue;
        gl.bindVertexArray(slot.vao); gl.drawElements(gl.TRIANGLES, slot.count, gl.UNSIGNED_INT, 0);
      }
    }
    gl.bindVertexArray(null);
    gl.disable(gl.BLEND);
  }
  // Textured: the sky behind everything, then one draw per material, the map's and the props' (props from both
  // sides: their models' faces aren't wound as the map's); the solid ones first, then the see-through ones over
  // them (without writing depth, so they don't hide each other)
  function drawTextured(T, eye) {
    const gl = V.gl, cull = gl.isEnabled(gl.CULL_FACE);
    if (T.sky) { gl.disable(gl.CULL_FACE); drawSky(T.sky, eye); }
    for (const seeThrough of [false, true]) {
      if (seeThrough) { gl.enable(gl.BLEND); gl.depthMask(false); }
      if (cull) gl.enable(gl.CULL_FACE);
      drawWorld(T, seeThrough);
      gl.disable(gl.CULL_FACE);
      if (T.props) drawProps(T, seeThrough);
    }
    if (cull) gl.enable(gl.CULL_FACE);
    gl.depthMask(true); gl.disable(gl.BLEND);
    gl.activeTexture(gl.TEXTURE0);
  }
  function drawWorld(T, seeThrough) {
    const gl = V.gl, u = V.texU;
    gl.useProgram(V.texProg);
    gl.uniformMatrix4fv(u.u_mvp, false, V.mvp);
    gl.uniform1i(u.u_base, 0); gl.uniform1i(u.u_base2, 1); gl.uniform1i(u.u_light, 2);
    gl.activeTexture(gl.TEXTURE2); gl.bindTexture(gl.TEXTURE_2D, T.light);
    gl.bindVertexArray(T.vao);
    for (const g of T.groups) {
      const m = g.m;
      if (m.mode === "hidden" || (m.mode !== "opaque") !== seeThrough) continue;
      if (seeThrough) blendFor(m);
      gl.activeTexture(gl.TEXTURE0); gl.bindTexture(gl.TEXTURE_2D, m.tex || V.white);
      gl.activeTexture(gl.TEXTURE1); gl.bindTexture(gl.TEXTURE_2D, m.tex2 || V.white);
      gl.uniform2fv(u.u_texel, m.texel); gl.uniformMatrix3fv(u.u_xform, false, m.xform); gl.uniform4fv(u.u_color, m.color);
      gl.uniform4f(u.u_opt, m.tex2 ? 1 : 0, m.unlit ? 1 : 0, m.test, m.mode === "blend" ? 1 : 0);
      gl.drawElements(gl.TRIANGLES, g.count, gl.UNSIGNED_INT, g.first);
    }
    gl.activeTexture(gl.TEXTURE0);
  }
  function blendFor(m) {  // the colour only: what's drawn stays opaque, so the page never shows through it
    const gl = V.gl;
    if (m.mode === "add") gl.blendFuncSeparate(gl.ONE, gl.ONE, gl.ZERO, gl.ONE);
    else if (m.mode === "mod") gl.blendFuncSeparate(gl.DST_COLOR, gl.SRC_COLOR, gl.ZERO, gl.ONE);  // darkens and brightens what's behind
    else gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ZERO, gl.ONE);
  }
  // Runs fn once: on the next animation frame, or after ms if frames are paused (a hidden window). Whichever
  // comes first cancels the other; a leftover one would run fn again, and for flying, where each step asks
  // for the next, that doubled the steps every frame (thousands of draws a frame within a second).
  function later(fn, ms, done) {
    let raf, to;
    const go = () => { cancelAnimationFrame(raf); clearTimeout(to); done(); fn(); };
    raf = requestAnimationFrame(go); to = setTimeout(go, ms);
    return true;
  }
  // input asks for a frame instead of drawing at once: a fast mouse sends far more events than the screen shows
  function requestDraw() { V.drawWait = V.drawWait || later(draw, 50, () => { V.drawWait = null; }); }

  // ---------------------------------------------------------------- input
  // Hold the right button to look around as in the game: the cursor locks and the raw mouse turns the
  // view (no pointer acceleration), at the game's default sensitivity (3 x m_yaw 0.022 degrees a count).
  const SENS = 0.066;
  cv.addEventListener("mousedown", e => {
    e.preventDefault();
    if (e.button === 0 && V.inputPts) {  // a mark: drag it along the path (or turn a keyframe by its arrow)
      const r = cv.getBoundingClientRect(), h = markAt(inputHits(), e.clientX - r.left, e.clientY - r.top);
      if (h) { label.hidden = true; markGrab(h.m, h.part, "3d"); return; }
    }
    if (e.button === 2 && V.mode !== "eye") {
      V.look = true;
      try {
        const p = cv.requestPointerLock({ unadjustedMovement: true });
        if (p && p.catch) p.catch(() => { try { cv.requestPointerLock(); } catch (err) { /* no lock: drag instead */ } });
      } catch (err) { try { cv.requestPointerLock(); } catch (err2) { /* no lock: drag instead */ } }
    }
    V.drag = { x: e.clientX, y: e.clientY, moved: false, button: e.button };
    cv.style.cursor = "grabbing";
  });
  document.addEventListener("pointerlockchange", () => { if (document.pointerLockElement !== cv) V.look = false; });
  const turn = (dx, dy, k) => {
    if (V.mode === "follow") { V.orbit.yaw -= dx * k; V.orbit.pitch = Math.max(-85, Math.min(85, V.orbit.pitch + dy * k)); }
    else if (V.mode === "free") { V.free.yaw -= dx * k; V.free.pitch = Math.max(-89, Math.min(89, V.free.pitch + dy * k)); }
  };
  cv.addEventListener("contextmenu", e => e.preventDefault());
  window.addEventListener("mousemove", e => {
    if (PM.drag && PM.drag.view === "3d") {  // a mark: along the path at the mark's height, or round it
      const r = cv.getBoundingClientRect(), path = markPath(); if (!path) return;
      const h = POST[PM.drag.m.kind], z = path.z[PM.drag.m.tick] + h;
      markDrag(e.clientX - r.left, e.clientY - r.top, t => toScreen([path.x[t], path.y[t], path.z[t] + h]), (x, y) => groundAt(x, y, z), e.shiftKey, e.altKey);
      return;
    }
    if (V.look && document.pointerLockElement === cv) { turn(e.movementX, e.movementY, SENS); requestDraw(); return; }
    const d = V.drag; if (!d) return;
    const dx = e.clientX - d.x, dy = e.clientY - d.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) d.moved = true;
    d.x = e.clientX; d.y = e.clientY;
    turn(dx, dy, V.mode === "follow" ? 0.3 : 0.25);
    requestDraw();
  });
  window.addEventListener("mouseup", e => {
    if (PM.drag && PM.drag.view === "3d") { markDrop(); return; }
    if (e.button === 2 && V.look) { V.look = false; if (document.pointerLockElement === cv) document.exitPointerLock(); }
    const d = V.drag; if (!d) return;
    V.drag = null; cv.style.cursor = "grab";
    if (!d.moved && d.button === 0) pick(e);
  });
  // pointing at a mark lights it and says what dragging it does
  cv.addEventListener("mousemove", e => {
    if (V.drag || V.look || PM.drag) return;
    const r = cv.getBoundingClientRect(), x = e.clientX - r.left, y = e.clientY - r.top, h = markAt(inputHits(), x, y);
    if (h) label.hidden = true;
    markPoint(h, x, y, cv, requestDraw);
  });
  cv.addEventListener("mouseleave", () => { if (!PM.drag) markPoint(null, 0, 0, cv, requestDraw); });
  cv.addEventListener("dblclick", () => { const pl = player(); if (pl) { V.orbit.yaw = pl.yaw; V.orbit.pitch = 22; requestDraw(); } });
  cv.addEventListener("wheel", e => {
    e.preventDefault();
    if (PM.drag && PM.drag.bend) { bendWheel(e.deltaY, PM.drag); return; }  // while bending a keyframe: how far it reaches
    if (V.mode === "follow") V.orbit.dist = Math.max(48, Math.min(30000, V.orbit.dist * Math.exp(e.deltaY * 0.0015)));
    else if (V.mode === "free") V.free.speed = Math.max(50, Math.min(20000, V.free.speed * Math.exp(-e.deltaY * 0.0015)));
    requestDraw();
  }, { passive: false });
  // a point of the map on screen (in the view's CSS pixels), or null behind the camera
  function toScreen(p) {
    const m = V.mvp; if (!m) return null;
    const w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15]; if (w <= 0) return null;
    return [((m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12]) / w * 0.5 + 0.5) * cv.clientWidth,
      (0.5 - (m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13]) / w * 0.5) * cv.clientHeight];
  }
  // where the ray through the mouse meets the level z ([x, y], or null)
  function groundAt(mx, my, z) {
    const c = V.cam; if (!c) return null;
    const { f, s, u } = basis(c), x = (mx / cv.clientWidth) * 2 - 1, y = 1 - (my / cv.clientHeight) * 2, k = 0.75;  // k: tan of half the vertical view
    const d = [0, 1, 2].map(i => f[i] + s[i] * x * k * c.aspect + u[i] * y * k);
    if (Math.abs(d[2]) < 1e-6) return null;
    const t = (z - c.eye[2]) / d[2];
    return t > 0 ? [c.eye[0] + d[0] * t, c.eye[1] + d[1] * t] : null;
  }
  // a click on the path picks that tick
  function pick(e) {
    const r = boxRun(); if (!r || !V.mvp) return;
    const rect = cv.getBoundingClientRect(), mx = e.clientX - rect.left, my = e.clientY - rect.top;
    let best = -1, bd = 14;
    for (let t = 0; t < r.x.length; t++) {
      const q = toScreen([r.x[t], r.y[t], r.z[t]]); if (!q) continue;
      const dd = Math.hypot(q[0] - mx, q[1] - my); if (dd < bd) { bd = dd; best = t; }
    }
    if (best >= 0) (window.moveTick || setTick)(best);
  }
  // Flying (Free): W A S D, Q/E, Shift. A key pressed while the mouse is over the view (or locked to it, or
  // with the right button held) belongs to the view until it's released, wherever the mouse goes meanwhile:
  // the editor never sees it, not even its auto-repeat (which would toggle keys on the timeline).
  cv.addEventListener("mouseenter", () => { V.hover = true; });
  cv.addEventListener("mouseleave", () => { V.hover = false; });
  const FLY = new Set(["w", "a", "s", "d", "q", "e", "shift"]);
  const inView = () => V.hover || document.pointerLockElement === cv || V.look || (V.drag && V.drag.button === 2);
  window.addEventListener("keydown", e => {
    const k = e.key.toLowerCase();
    if (!FLY.has(k)) return;
    if (!V.keys.has(k)) {
      if (!V.on || V.mode !== "free" || !inView() || e.ctrlKey || e.metaKey || e.altKey) return;
      if (!V.keys.size) { V.last = performance.now(); nextFly(); }
      V.keys.add(k);
    }
    e.preventDefault(); e.stopImmediatePropagation();
  }, true);
  window.addEventListener("keyup", e => {
    const k = e.key.toLowerCase();
    if (V.keys.delete(k)) { e.preventDefault(); e.stopImmediatePropagation(); }
  }, true);
  window.addEventListener("blur", () => V.keys.clear());
  // the next step on the next animation frame, or after 20 ms if frames are paused (a hidden window)
  function nextFly() { V.flyWait = V.flyWait || later(fly, 20, () => { V.flyWait = null; }); }
  function fly() {
    if (!V.keys.size || V.mode !== "free") return;
    const now = performance.now(), dt = Math.min(0.1, (now - V.last) / 1000); V.last = now;
    const f = dir(V.free.yaw, V.free.pitch), r = [Math.sin(rad(V.free.yaw)), -Math.cos(rad(V.free.yaw)), 0];
    const k = V.keys, sp = V.free.speed * (k.has("shift") ? 4 : 1) * dt;
    const mv = [0, 0, 0], add = (v, s) => { for (let i = 0; i < 3; i++) mv[i] += v[i] * s; };
    if (k.has("w")) add(f, 1); if (k.has("s")) add(f, -1); if (k.has("d")) add(r, 1); if (k.has("a")) add(r, -1);
    if (k.has("e")) mv[2] += 1; if (k.has("q")) mv[2] -= 1;
    for (let i = 0; i < 3; i++) V.free.pos[i] += mv[i] * sp;
    draw();
    nextFly();
  }

  // ---------------------------------------------------------------- the Export page's preview
  // The replay's eye at tick t (a fraction between two ticks is blended, as the game draws between them): where the
  // bot is, looking along the camera replays show (the page's cameraYaws and replay pitch). null without a run.
  function eyeAt(t) {
    const r = boxRun(); if (!r || !r.x || !r.x.length) return null;
    const i = Math.max(0, Math.min(r.x.length - 1, Math.floor(t))), f = Math.min(1, Math.max(0, t - i));
    const yaws = typeof cameraYaws === "function" ? cameraYaws(r) : r.yaw;
    const pitchAt = k => typeof replayPitchAt === "function" ? replayPitchAt(k) : 0;
    const eyeZ = k => r.z[k] + (r.ducked && r.ducked[k] ? 46 : 64);
    let pos = [r.x[i], r.y[i], eyeZ(i)], yaw = yaws ? yaws[i] : 0, pitch = pitchAt(i);
    if (f > 0 && i + 1 < r.x.length) {
      const n = [r.x[i + 1], r.y[i + 1], eyeZ(i + 1)];
      if (Math.hypot(n[0] - pos[0], n[1] - pos[1], n[2] - pos[2]) <= (r.tick_interval || 0.015) * 3500 * 2.5) {  // not across a teleport
        pos = pos.map((p, k) => p + (n[k] - p) * f);
        if (yaws) yaw += (((yaws[i + 1] - yaw) % 360 + 540) % 360 - 180) * f;
        pitch += (pitchAt(i + 1) - pitch) * f;
      }
    }
    return { eye: pos, yaw, pitch };
  }
  // One frame of it, w×h pixels, onto the 2D canvas out: the map as the game draws it (textured), none of the
  // editor's paths and marks (a render shows none, nor the HUD). { ok: drawn, wait: what's still loading (call again
  // in a moment), why: why it can't be drawn }.
  function snapshot(out, t, w, h) {
    init();
    if (!V.gl) return { ok: false, wait: "", why: V.error || "the preview needs WebGL2" };
    const want = S.norm && S.norm.map;
    if (!want) return { ok: false, wait: "", why: "open a timeline" };
    if (want !== V.mapName && !V.loading) loadMap(want);
    if (V.loading || V.mapName !== want) return { ok: false, wait: `loading ${want}…`, why: "" };
    if (!V.map) return { ok: false, wait: "", why: V.error || `can't show ${want} in 3D` };
    const T = V.tex && V.tex.name === V.mapName ? V.tex : null;
    if (!T) { if (!V.texLoading) loadTextured(V.mapName); return { ok: false, wait: "loading the textures…", why: "" }; }
    const c = eyeAt(t);
    if (!c) return { ok: false, wait: "", why: "run the timeline once to preview it" };
    const gl = V.gl, keep = V.mvp;
    if (cv.width !== w) cv.width = w;  // left at this size (the viewer sets its own when it draws), so playing doesn't reallocate it
    if (cv.height !== h) cv.height = h;
    gl.viewport(0, 0, w, h);
    gl.clearColor(0, 0, 0, 0); gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);  // clear: the page puts a sky behind
    V.mvp = matrices(c, w / h);
    gl.enable(gl.DEPTH_TEST); gl.enable(gl.CULL_FACE); gl.cullFace(gl.BACK);
    if (!T.error) drawTextured(T, c.eye);
    else {
      gl.useProgram(V.mapProg); gl.uniformMatrix4fv(V.mapU.u_mvp, false, V.mvp); gl.uniform3fv(V.mapU.u_eye, c.eye);
      gl.bindVertexArray(V.map.vao); gl.drawElements(gl.TRIANGLES, V.map.count, gl.UNSIGNED_INT, 0); gl.bindVertexArray(null);
    }
    gl.disable(gl.CULL_FACE);
    out.getContext("2d").drawImage(cv, 0, 0, out.width, out.height);
    V.mvp = keep;  // the viewer's, for picking things with the mouse
    if (V.on && VIS.open) requestDraw();  // the viewer's own view again
    const P = T.props, left = T.error ? "" : T.loaded < T.used.length ? `textures ${T.loaded}/${T.used.length}…` :
      T.extrasDone ? "" : P && P.loaded < P.total ? `props ${P.loaded}/${P.total}…` : "the sky…";
    return { ok: true, wait: left, why: "", pitch: c.pitch };
  }

  window.View3D = { render: draw, toggle: () => set3D(!V.on), active: () => V.on && VIS.open, state: V, player, snapshot };  // state: for looking into it from the console
  apply();
})();
