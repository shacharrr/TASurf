# Design notes

## Pipeline

```
timeline.json ──bake──▶ <name>.tas ──tas_run──▶ engine ──▶ <name>_NNN.csv ──▶ verify / check / draw
 (keyframes,            (one line              (CS:S gamemovement            (state after
  key ranges)            per tick)              is the simulator)              every tick)
```

* **The timeline** is the editable source. It stores intent (keyframes,
  ranges); yaw is unwrapped.
* **The baker** (Python, and the same in Rust under `rust/`) is a pure function
  from timeline to per-tick inputs. All interpolation lives there, and it has
  unit tests. The same timeline always bakes to the same bytes, from either
  back end.
* **The plugin** is deliberately simple. It only does reset, playback,
  recording, comparison and drawing. It never interpolates, predicts or
  corrects anything.
* **The engine is the simulator.** `tasurf/physics.py` reimplements the air
  movement equations only to sanity-check recorded runs; it never feeds
  anything back into a run.

## Determinism model

A run is a function of: the baked file, the server settings, the start state,
`tas_settle_ticks`, and the map's entity state. The plugin makes everything it
controls identical between runs, and records the rest so differences are
visible.

### One input per executed command

Inputs are applied in `OnPlayerRunCmd` (before the engine processes the
command) and the state is recorded in `OnPlayerRunCmdPost` (after movement,
trigger touches and post-think). Timeline tick N is the Nth user command the
engine **actually executed**. The player's `m_nTickBase` only advances when
`CPlayerMove::RunCommand` runs a command. If a command is dropped
(`sv_maxusrcmdprocessticks`, pause), the tick is not advanced and the same
input is replayed on the next command. Real-time effects such as frame rate,
lag and alt-tabbing therefore change *when* ticks happen, not *what* they
compute. Dropped commands are counted in the `.meta.txt`.

Every field of the command is overwritten: buttons, forward/side/up move,
pitch/yaw/roll, impulse, weapon select, mouse deltas. What you do with the
keyboard or mouse during a run has no effect on it.

With `tas_view_follow 1` (the default), the camera also turns with the
replay: before each tick the player's eye angles are snapped to that tick's
angles with an angles-only `TeleportEntity`. In SDK 2013 that sets the local
and eye angles and touches no position, velocity or triggers. The movement
code reads the command's angles, which are already set. Comparing a replay
with view-follow against a recording made without it is the check that this
stays cosmetic.

On auto-strafe ticks those eye angles can get a smoothed yaw (`@view_yaw`,
plugin 0.15): auto-strafe looks along the velocity for the most speed, and on
a straight range that zigzags by atan(30/speed) a tick. The plugin can't
smooth that well itself, as it only knows the ticks played so far (a causal
filter lags where the turn changes). But runs are bit-identical, so once the
game has played these inputs (the editor's preview after each edit) every
yaw of the run is known: when the editor sends a run, preview or render it
looks for a complete run CSV of this timeline with the same `baked_hash`
(the preview first, then the newest run) and bakes `tasurf/camera.py`'s
camera from its yaw column. That's a centred binomial average (weights
C(48, 24 + k), σ ≈ 3.5 ticks) of the unwrapped yaw on the auto-strafe ranges (or
the whole run, the timeline's `camera_smoothing`),
eased in and out over 16 ticks either side with a smoothstep, each stretch
between jumps of more than 30° in a tick (teleports) on its own. The lines
aren't hashed, so the file's hash, and which runs compare with it, are the
same with them or without. Not for a recording's lead-in (you take over from
the yaw played), and not for plugins older than 0.15 (they refuse unknown
lines). `rust/src/camera.rs` and the page's `cameraRun` (the 3D view's eye
and the light blue line on the yaw row) are the same rule; a Rust test pins
its output to Python's.

### Reset sequence

```
tas_run
  ├─ settle tick(s)   pre:  Start_Apply(settle=true), neutral inputs (no buttons, no move, start angles)
  │                   post: (not recorded)
  └─ tick 0           pre:  Start_Apply(settle=false), then tick 0's inputs   ← last write of position/velocity
     tick 1..N-1      pre:  inputs only
                      post: record
```

`Start_Apply` writes: movetype WALK; duck state fully standing (`m_bDucked`,
`m_bDucking`, `m_bInDuckJump`, duck/jump timers, FL_DUCKING, view offset);
`m_flFallVelocity` 0; origin, angles and velocity (TeleportEntity); base
velocity and FL_BASEVELOCITY; gravity scale; lagged movement; CS:S stamina
(jump/landing slowdown, default 0) and velocity modifier (damage slowdown,
default 1); health when captured; ground entity and FL_ONGROUND.

The stamina reset was added after the stress test caught it. A jump about
0.2 s before `tas_run` left about 1100 ms of stamina, which CS:S applies as a
speed multiplier in `WalkMove`. Tick 0 still matched, because it starts from
zero speed, but runs diverged from tick 1. The implied stamina fell by exactly
15 ms per tick, CS:S's decay rate.

The **settle tick** flushes state that can't be written directly but that the
engine recomputes every tick:

* the previous tick's buttons (`m_nOldButtons`, which gates jumping). After a
  settle tick with no buttons, tick 0 always sees "jump not held";
* ground state and `m_surfaceFriction` for ground starts: the end-of-tick
  `CategorizePosition` sets them from the surface under the start origin;
* `m_bGameCodeMovedPlayer` (whether the previous predicted origin matches).
  This is identical across runs once the settle tick has run from the same
  state.

With `on_ground: "auto"` the settle tick starts airborne and the engine's own
ground check decides. The tick-0 reset then keeps that result.

### Recording live play (`tas_record`)

A recording is a run whose inputs come from the player instead of a file. It
captures the current state as the start and goes through the **same reset**
(settle tick, then the tick-0 reset). From then on, your own commands drive
the engine. The fields playback can't reproduce are zeroed: upmove, roll,
impulse, weapon select and mouse deltas. Every executed command is recorded:
the inputs as the engine processed them, and the resulting state.

The output is the usual run CSV plus a baked file holding the exact float bits
of every input. Start fields that the baked format stores only as decimals
(base velocity, gravity, stamina, ...) are rounded through the same 6-decimal
text before the live reset, so the live recording and its replay reset to
bit-identical states. The recording is then loaded, and the replay is
compared against it like any other run of the same hash. That makes it a
direct test that a replay of real surfing reproduces it.

A recording isn't an editable timeline yet: it has no keyframes. Turning one
into a timeline (simplifying per-tick yaw into keys) belongs to the editor
milestone.

### Hidden state the reset does not cover (known limitations)

* **`m_surfaceFriction` for airborne starts.** It isn't a netprop or a datamap
  field, so SourceMod can't write it without a gamedata offset. It's only
  updated on the ground or while moving up slowly near a surface, so an
  airborne start inherits whatever the previous run left. It multiplies air
  acceleration, but with surf values of `sv_airaccelerate` the 30 u/s
  add-speed cap is reached either way. It matters with low airaccelerate.
  Prefer ground starts. If an airborne-start run ever diverges, this is the
  first suspect.
* **Command-number phase.** Some gamemovement checks (stuck checks, ladder
  checks) run every N commands, keyed to the command number, which the plugin
  doesn't control. Normal surfing never triggers them.
* **Map entity state.** Moving brushes, toggled triggers, `point_servercommand`
  and so on keep their own timers. Static surf maps are fine. The CSV records
  a warning if convars change during a run.
* **Other plugins.** Timers, speedometers that write movement, anti-cheat and
  so on can change inputs or state. Run with only TASurf, or check the echoed
  inputs in the CSV (the in-game comparison reports "processed inputs differ").
* **Rounds.** A round restart respawns the player. The plugin aborts the run
  when the player spawns or dies.

### Checking it

* In game: `tas_run 3` plays three times back to back and compares each run
  with the previous one, bit for bit (origin, velocity, base velocity,
  ground/duck state and processed inputs, every tick). It reports
  "bit-identical" or the first divergent tick and field.
* Offline: `python -m tasurf verify --latest <name>` compares the two newest
  CSVs byte for byte and then cell by cell. The `*_bits` columns make that
  exact.
* `python -m tasurf check <csv>` re-predicts every free-flight air tick from
  the previous one with the Source SDK 2013 order (half gravity, CheckVelocity,
  AirAccelerate with the 30 u/s cap, move, other half of gravity). It also
  checks the perpendicular-strafe identity `new_speed = sqrt(speed² + 900)`.
  Ticks where vz doesn't follow gravity are counted as contact (surface clip,
  trigger) and not checked. Among free-flight ticks, a tick whose velocity is
  untouched but whose position jumped further than any velocity allows counts
  as a **teleport**. A tick whose velocity differs from the prediction by more
  than 30 u/s, more than a whole tick of air acceleration can add, counts as a
  **map event** (wall hit, speed-reset trigger). Only the rest count as model
  mismatches, and only those give exit code 1. On the first real recording
  (surf_beginner, 4,263 ticks) that was 1,817 matches, 6 teleports, 9 events,
  0 mismatches, and 44 perpendicular ticks within 0.0005 u/s of √(speed²+900).

## How the later milestones fit

### Cached checkpoints

`StartState` + `Start_Apply` already is the restore primitive. A checkpoint is
a `StartState` captured in `OnPlayerRunCmdPost` at tick N, plus the state the
settle tick normally flushes, which must then be restored explicitly instead:
the old buttons from tick N-1 (known from the timeline; writable as
`m_nOldButtons`), and the ground entity. On an edit, the baker can report the
first tick whose input changed: compare per-tick lines, or add a per-tick
prefix hash to the baked file. The plugin restores the newest checkpoint before
that tick and plays from there. A checkpointed re-sim must be verified against
a full run with `verify` before it's trusted. The CSV format doesn't change;
runs are spliced as "rows 0..k from cache + new rows".

### Two back ends

The back end exists twice: the Python package `tasurf/` (the reference) and a Rust port in `rust/`,
one module per Python module, built into a single native binary. They are meant to be interchangeable byte
for byte, including error messages, so `rust/src/py.rs` reproduces the Python behaviour the outputs depend
on: `repr` and `%`-style float formatting (correctly rounded, ties to even), `round`, `%` on floats,
CPython 3.12's `math.hypot` (its own fused-multiply-add algorithm, not the C library's), `json.dumps`
layout and escaping, and the messages `json.loads` gives for a broken file. `rust/parity.py` runs both on
copies of the real timelines, recordings and run CSVs (plus ~70 broken timelines and ~350 editor API
calls) and compares every output and every file written.

The desktop app (`rust/app/`, Tauri) is the same editor in a native window: it starts the Rust editor server
inside its own process on 127.0.0.1:8768 and points the window's system web view (WebView2, WKWebView,
WebKitGTK) at it, so the page and its JSON API are exactly what a browser gets. The port is fixed so the page's
`localStorage` (panel sizes, what the viewer shows, auto-preview) survives restarts. Two things a browser does that a closing
window doesn't: the unsaved-changes prompt, which the app runs itself on close (the page answers by navigating
to `/__tasurf_close__`, which the app catches), and links, which the app opens in the system browser instead.

### 3D view

`tasurf/bspmap.py` (and `rust/src/bspmap.rs`, byte for byte the same) reads a map's `.bsp`. It takes the faces of
the world and of brush entities, shifted by the entity's origin, plus displacements. Tool faces (nodraw, sky,
trigger, hint, skip) are dropped, and LZMA-compressed lumps are unpacked. The result is a flat binary triangle
mesh served at `/api/map`. Every triangle is wound counter-clockwise seen from its surface's front, so the
viewer can cull back faces and see through walls from behind them, as Hammer's 3D view does. The front is where a
face's corners run clockwise, the order the game draws it in. Not the plane turned by the face's side flag: in
Source the flag only repeats the plane number's last bit, and the plane already faces the face's way. Turning
those faces again hid about 40% of every map from the front, one side of each ramp and wall. On the 87
installed maps the corners agree with the plane on 99.87% of faces; the rest are zero-area or odd faces from
unusual compilers, where the corner order is what the game draws. A displacement faces where its base face
does, as a whole: a sculpted part bent past square to the face, such as the far side of a folded ramp, still
faces where the rest does. `editor_static/view3d.js` draws it with WebGL2 and no libraries: flat shading by slope from
screen-space derivatives, paths as screen-space ribbons, and everything else from the page's own state. Only
the editor shows it; nothing about runs depends on it.

The textured look comes from a second block of data for the same vertices and triangles, `/api/maptex`
(described at the top of `bspmap.py`): each vertex's position on its texture (from the face's texture
vectors) and on one lighting atlas, each triangle's material, the material names, and the atlas. The atlas
packs every face's lightmap in rows, tallest first, each with a border of its edge luxels so filtering
never reads a neighbour. The luxels stay as the map stores them (RGB with a shared exponent); the page turns
them into what the game keeps for LDR lightmaps (gamma 2.2, halved, doubled back in the shader). A
lightmap is the face's first: its first light style, and with bump maps the first of the four, which is the
unbumped one. Displacements' lightmaps don't follow the plane projection: they're laid over the
displacement's grid, t along the edge from its start corner and s along the other. That orientation came
from the maps themselves: it's the one that makes neighbouring displacements' luxels agree along their
shared edges, by a factor of 50 to 6000 over the other three. Maps with only HDR lighting keep their light
offsets in the HDR copy of the faces (the plain one holds zeros), and maps compiled without lighting draw
fullbright.

Materials and textures come through `/api/mapfile`, which `tasurf/gamefiles.py` (and
`rust/src/gamefiles.rs`) answers the way the game searches: the map's pakfile (a zip in the .bsp, stored or
LZMA, the two kinds the game reads), then the search paths of CS:S's `gameinfo.txt` in order
(`cstrike/custom/*`, the VPKs, the loose folders, `cstrike/download`). Only `materials/....vmt` and `.vtf`,
and the props' `models/....mdl`, `.vvd` and `.vtx`, are served. The page parses the KeyValues materials itself (patch materials, DirectX 9 fallback blocks,
`[$WIN32]` conditions, which are never quoted, so a quoted value such as `"[.1 .1 .1]"` stays a colour) and the textures (VTF 7.0 to 7.5; DXT1/3/5 go to the GPU as they are, the other
formats are converted to RGBA). A material whose `%compile...` keys say the compiler drops it (trigger,
clip, nodraw, sky, ...) isn't drawn: older compilers didn't always mark those faces. LZMA streams of a
known size are read up to that size and no further, so both back ends accept them with or without an end
marker (Python's liblzma rejects the marker in sized streams; lzma-rs stops at the size).

The sky and the props come from `/api/mapprops` (`read_props`), as JSON: worldspawn's `skyname`, the
`sky_camera`'s origin and scale (for a 3D skybox, not drawn yet), the models' paths and each prop's model,
origin, angles and skin. Props are the static ones (the game lump `sprp`, compressed or not; its entry size
differs between versions, so it's taken from the lump's length) and the `prop_*` entities with a model, except
those drawn as nothing (`rendermode` 10) or hidden at the start (`StartDisabled`). `/api/mapfile` serves the
models' files too (`models/....mdl`, `.vvd`, `.vtx`). The page reads them itself: LOD 0 of each body part's
first model, the VVD's vertices (with its fixups), the VTX's strips, and the materials through the model's
skin table and material folders, searched in order as the game does. Each model goes to the GPU once and is
drawn instanced at its places, from both sides (models aren't wound like the map). Static props in the game
are lit per vertex or from the light round them, which the map's lightmaps don't hold, so here they get a
half-Lambert light from above scaled to the map's brightness (where 70% of its lit luxels are darker). The sky
is the six `skybox/<skyname><side>` materials on a cube round the eye, drawn first without depth, laid out by
the Quake tables the engine still uses (`rt` +x, `lf` -x, `bk` +y, `ft` -y), clamped at the edges; a side half
as tall as it's wide covers the top half.

Triggers come from `/api/maptrig` (`read_triggers`, format at the top of `bspmap.py`): every brush entity whose
class starts with `trigger_`, plus the CS:S zones (`func_buyzone`, `func_bomb_target`, ...). Their shapes are
built from their brushes, not their faces. Modern compilers flag trigger faces as never drawn, older ones keep
them as ordinary faces, and either way the brushes are what the game's collision uses. A model's brushes are
found through its own BSP tree (its head node down to the leaves' brush lists). Each brush side is a huge
square on its plane, cut by the brush's other planes the way the map compiler cuts windings. Bevel sides are
left out because they're only there for collision. The leaves' size depends on the leaf lump's version (56
bytes in version 0, 32 after), and on many maps the lump's length fits both. The same entity list keeps
trigger models out of the map mesh: the game never draws them, but on 29 of the installed maps the compiler
had kept their faces unflagged, so they showed as solid walls. The page draws triggers as see-through faces
(no depth writes, both sides) with their edges as ribbons. It names the one under the mouse by casting a ray
against the shown triggers' polygons (bounding boxes first).

Checked on every installed map (85: surf, CS:S's own, test maps): `rust/parity.py` compares the mesh, the
textured data, the triggers, the props and every material, texture and model file each map uses, byte for
byte between the back ends.

### Browser editor (built)

`python -m tasurf editor` serves a local page (127.0.0.1) over a small JSON API in `tasurf/editor.py`. The page
edits a normalised view of the timeline (`editdoc.normalized`); edits are merged back into the JSON
document (`apply_edits`), so start, settings and comments stay as written. Parsing stays in `timeline.py`.
Communication with the game uses files only: the editor writes `requests/request.txt` atomically, and the
plugin takes it every 0.25 s when idle, runs it and writes `requests/<id>.result`. No socket extension is
needed. A newer request replaces an older one: the editor overwrites a preview the game hasn't taken yet,
and the plugin drops an editor preview that's still playing when the next request arrives (its result
says `superseded`). A run at normal speed is never replaced. The plugin takes a request by renaming it to
`request.taken` first, so one the editor writes at the same moment isn't deleted unread.

Tick operations (`editdoc.apply_op`: insert, delete, length, turn) run on the server with the baker's own
interpolation. Keyframes after the edit move by whole ticks with unchanged values, so from the next
keyframe on the timeline bakes to the same inputs. Where an edit cuts a segment, `_pin` adds a key on the
curve: a hold or linear segment just gets that key. A cubic one gets a key with the curve's slope there,
and its end keys get theirs written in: a part of a cubic is a cubic, so both parts are the curve they
were. A cubic segment on the kept side whose slope came from keys beyond the edit gets that slope written
into the key it meets the edit at. A smooth segment can't be cut into two of its kind, so the part that's
kept is written out as one linear key per tick. (Until 2026-10-24 cubic segments were written out tick by
tick too, which left a recording-like row of linear keys with small corners after every insert.) The tests check this bit for bit before the edit and
to 1e-9 degrees after it, for every interpolation mode. Insert holds the yaw over the new ticks with a joint
segment whose ends are joints of width 0, which is exactly flat (until 2026-10-27 it was a `hold` segment,
which the editor's Curve menu doesn't offer, and the keyframe after it couldn't have a joint). Where it's
inserted at a key with a joint, `_pin`'s corner mode writes out the joint's ticks before the key instead of
keeping the joint, and the key after the new ticks gets width 0 too (a non-joint key there: its joint
dropped and the rate the joint had written in as its slope).

A joint segment (straight lines with rounded joints at the keys, through every key) depends on keys a few
segments away: each line is fitted to its neighbours' (six Jacobi passes on the condition that a segment's
line, shifted by the half joints at its ends, arrives exactly at its next key), so an edit's influence fades
within six segments and a fitted slope can be written into its key without changing anything else (the fit
never reads keys' own slopes). `_joint_pin` writes the lines and joint widths of the joint segments within
nine segments of a cut into their keys, so they no longer depend on what the edit changes. A joint key's
slope counts only together with its width, and `_joint_pin` always writes the two together: a slope alone
on a joint key is one a smooth curve left when the key became a joint, and is ignored (`_said_slope`). Keys
keep their kind; the keys a cut adds are joints of width 0. A cut at a key keeps that key's joint (its
outgoing line written in); a cut inside a segment gets one key, and only a joint the cut goes through has
its kept ticks written out. The cut
segment's straight part then runs between other ends, which can move its values in the last bits, so
`_joint_pin` checks that every kept tick of that segment reaches the game as the same float32 (wrapped, as
the baker writes it) and writes the kept ticks out if one doesn't (it hasn't happened in the tests). An
insert in a joint segment adds about three keys. Analysis per edit: the first tick whose input differs from the last run (`_first_difference`), a
free-flight estimate from that tick (`physics.estimate_free_flight`), and the best yaw per air tick
(`physics.best_yaw`, closed form `c* = max(30 − A, 0)`, checked against a brute-force scan in the tests).

Recording import (`editdoc.recording_to_timeline`) unwraps yaw, keeps keyframes only where linear
interpolation can't reproduce the exact float32, and turns button bits into ranges, including
`turnleft`/`turnright` and raw `bitN`. It is lossless because the baker wraps to (−180, 180], which is what
the client sends, and writes decimals that give back the exact float32.

### Fast engine preview (built)

A preview (`preview.sp`) is an ordinary run with `host_timescale` raised. It uses the same reset, the same
inputs and the same engine, and every tick keeps its normal length; only more ticks run per real second.
It plays on the replay bot, the same entity real runs use, so a preview is comparable to them tick for
tick. You're held still meanwhile (`MOVETYPE_NONE`), because with time sped up noclip would fly you 20
times as far. The editor bakes the unsaved document to
`<name>.preview.tas`, sends a `preview` request, and reads back `runs/<name>.preview.csv`. A preview is
fresh when that CSV's `baked_hash` equals the hash of the current document's bake.

Still to verify in game: that a preview at 20× is bit-identical to the same inputs at 1×. Compare a
preview CSV with a real run of the same file using `verify --rows-only`. If `host_timescale` ever
changed results, the preview would be a fast approximation rather than an exact one.

### The replay bot (built)

Runs and previews play on a CS:S bot (`bot.sp`), so the human can edit in noclip (`mode.sp`). A CS:S bot
runs one user command per tick through `CBasePlayer::PlayerRunCommand`, the same path as a human's
commands, and `OnPlayerRunCmd` sees it the same way. The plugin overwrites every field of the bot's
commands, during runs with the timeline's and between runs with nothing, so the bot's AI (also
switched off with `bot_stop`/`bot_zombie` as far as it goes) never moves it. A run resets the bot with
the same `Start_Apply` and settle tick as a player. For the run it gets a player's collision group and
damage back (fall damage changes `m_flVelocityModifier`, so it has to be taken as a player would);
between runs it's frozen (`MOVETYPE_NONE`), non-solid (collision group 2, debris that still touches
triggers) and can't be damaged. It holds no weapon, so it runs at 260 (CS:S's speed without a weapon, as
on KSF; a knife or pistol caps the player's speed at 250 although `m_flMaxspeed` still reads 260); the
maxspeed check compares `m_flMaxspeed` with the start state. The human in edit mode is also in collision group 2, so the bot's
movement traces never hit them.

A bot executes exactly one command per tick, so the dropped-command handling never triggers for it. What
the bot doesn't share with a human: a human's live commands carry client-side history (the client
predicts, batches commands when its frame rate is below the tickrate, and so on). None of that reaches
movement except through the commands themselves, which the recording stores, so a recording should
replay on the bot bit for bit, and that comparison is the test (`tas_run` after `tas_record`).

CS:S adds bots only on maps with a navigation mesh: `bot_add` on a map without one starts
`nav_generate`, which can take minutes and reloads the map. So at map start, if `maps/<map>.nav` doesn't
exist, the plugin writes a minimal one (version 16, sub-version 1 as CS:S writes them: one 64×64 area,
no connections, no hiding spots) and reloads the map once. On map end it sets `bot_quota 0`, so no bot
is added by quota before it has checked the next map; it adds its own bot (`bot_add_t`/`bot_add_ct`)
on your team once you've joined one, which also avoids CS:S's "Game Commencing" restart that a bot on
the other team would cause.

### Instant prediction (built)

Bunnymod XT's TAS editor shows an edit's effect at once by predicting it with its own copy of the movement
code, then replaces the prediction with the exact path a second game instance plays in the background.
TASurf does the same with the game's preview in place of that second instance: `editor_static/predict.js`
predicts in the page, and the preview replaces it.

* **Where it starts.** The page bakes the current inputs itself (the same interpolation as `interp.py`), and
  finds the first tick where they differ from what the newest exact path played (the fresh or stale
  preview, or the last run, whichever agrees longest; auto-strafe ticks compare only their keys). It starts
  from that path's exact state the tick before: position, velocity, ground, duck, base velocity, the
  buttons held, plus stamina and any duck under way, both rebuilt from the path's own jumps and duck
  presses. The run payload carries what that needs: the inputs played, base velocity and the movement
  settings from the run's header (`move`).
* **The map.** `/api/mapclip` (`read_clip`, format at the top of `bspmap.py`) sends every brush of every
  model with its planes, bevels included, and the entity lump. The page keeps the world's player-solid
  brushes (contents `MASK_PLAYERSOLID`) and the solid brush entities (by class, `func_brush` by its Solidity
  and StartDisabled), moved to their entity's origin, in 256-unit columns. `trigger_teleport` (to its
  target, or relative to its landmark) and `trigger_push` (pushdir as angles, times speed, into the base
  velocity) are what it acts on.
* **The movement.** `CGameMovement::FullWalkMove` with CS:S's changes: box traces as
  `CM_ClipBoxToBrush` does them (each side moved out by the box, `DIST_EPSILON` 1/32), `TryPlayerMove`
  (four bumps, the single-plane air case, creases, the stuck check), `StepMove`, `StayOnGround`,
  `CategorizePosition` (no ground above 140 u/s up or below 0.7 normal), friction, ground and air
  acceleration (the add-speed capped at 30), and jumps.
* **CS:S, as the runs show it.**
  * **Stamina:** a jump sets it to 25/19 s, which runs down in real time. While any is left, jumps are
    lower and ground speed is scaled by (100 − 19·s)/100, raised to frametime·70 (the Goldsrc frame rate it
    imitates). Without it every prediction went wrong on the first landing.
  * **Ducking:** in the air it's immediate, lifting the origin by exactly 8.5 (and lowering it again to
    stand, when there's room). On the ground it takes 27 ticks to duck (0.4 s) and 14 to stand (0.2 s).
    While ducked, ducking or holding duck on the ground, the keys move you at a third.
  * **Teleports** keep the player's velocity.
* **Double precision.** A slide along a surface the box is already closer to than `DIST_EPSILON`, after the
  velocity was clipped to it, comes out a hair into it in double precision. The game, in float32, doesn't
  stop on it, so a move of less than 1e-4 into a side counts as along it. Without that, sliding ramps stopped
  dead in about one prediction in ten.
* **Checked against the game.** `Predict.replay(run, k, n)` (from the console) replays a run's own inputs
  from tick k and measures how far each tick ends from where the game put the player. Over 245 starting
  ticks of three runs on surf_beginner (ground, jumps, landings, ducking, ramps, teleports), it stayed within
  a unit for typically 100 to 300 ticks, and never for less than 30. One recording made on a one-player
  server doesn't fit: there CS:S ducks you in the air on every jump, as HL2 does in single player.
* **Cost.** About 15 µs a tick. The page works on it 6 ms a frame, nearest ticks first, and starts again
  whenever the inputs (`S.ver`), the paths or the map change, so a drag shows its effect while you drag.

Not modelled: displacements and props (no collision data in the export yet), moving brushes, map logic
(outputs, filters, speed and gravity changers, `trigger_push` filters), and the game's float32 rounding,
which is why it drifts. True in-engine simulation (running the engine's own movement many ticks a frame
through DHooks/SDKCall with gamedata for 64-bit CS:S) would remove those limits; checkpoints would also cut
the preview's time by re-simulating only from the first edited tick.

### View pitch (built)

A timeline's `view_pitch` track is the pitch replays show. The run's pitch reaches the movement (the
engine flattens and rescales the view direction in float32, so the pitch changes the wish direction in
its last bits), so a cosmetic pitch can't be the run's own. The plugin applies it in `OnPlayerRunCmdPost`,
after the tick's movement, to `m_angEyeAngles[0]`, which clients interpolate and spectators in first person
see, and with `tas_view_follow` to your camera. The next tick sets it from its own command first. The baker
writes it as `@view_pitch` lines after the tick lines (16 values a line, 2 decimals), and the hash leaves
them out. Changing it therefore keeps the run's hash: the preview stays fresh, and runs before and after
still compare. Plugins before 0.9 refuse the lines, so the editor checks the heartbeat's version first. The
page's Look tool makes the track from the exact path, and it's written as linear keyframes within 0.04°.

### Bending the yaw (built)

Dragging a yaw keyframe changes the curve around it smoothly instead of moving one key (which leaves a
corner, or on a recording, where keys are a tick or two apart, a spike). The change fades out towards the
keyframes either side that are at least `BEND.reach` ticks away (20 by default, set with the wheel while
dragging). On a side where the next keyframe is that far, the segment becomes a cubic. Its far key gets
its own slope: the one the curve already had there, so the part beyond is unchanged and joins without a
corner. The dragged key gets the slope the curve had at it. On a side with keys in between, each key moves
by the change times a smoothstep bell, which is flat at both ends. A key's own slope is part of the timeline
format (`[tick, value, interp, slope]`). Both back ends read it, and their tick operations keep it. The
cubic segments between keys that give their slopes don't depend on other keys, so insert and delete no longer
write them out tick by tick.

While a drag is under way the page knows the inputs changed before the server does (the hash is only
updated when the drag ends), so it doesn't treat the preview as current from the first change, and the
prediction takes its place on screen.

### Inputs on the path (built)

Yaw keyframes and key presses and releases are drawn where they happen on the path the inputs last played
(the fresh preview, else the last run), in both views, and dragged along it as Bunnymod XT's editor drags
its frame bulks. The mark goes to the tick of the path nearest the mouse, between its neighbours, and the
path holds still while you drag. A keyframe's arrow turns its yaw by how far the mouse goes round the mark.
In 3D, the turn is measured on the horizontal plane at the arrow's height, under the mouse's ray.

### External editor over a local socket

Every plugin feature is a console command, and all data is exchanged as files
(timeline JSON in, baked file to the game, CSV out). An editor can drive the
game today through RCON or the console, and read CSVs directly. A socket
(e.g. a SourceMod socket extension bound to 127.0.0.1) can be added later for
push notifications ("run finished, CSV at …") without changing formats. The
timeline JSON is the editor's document format; yaw tracks, key ranges and
the speed graph (from the CSV `hspeed` column) map one-to-one onto its views.

### Auto-strafe (built)

The optimal yaw depends on the velocity, which only the engine knows, and on a
surf ramp the velocity bends every tick. A hand-made curve that trails it by
asin(30/speed), 2.75° at 625 u/s, gains nothing at all: that is what made the
first editor-made turn on surf_devs_ksf slow (0.3 u/s per tick for 17 ticks
where the recording gained 20). So `strafe.sp` computes the yaw in the plugin
from the actual velocity at each tick: deterministic, because the velocity is.
The baked file carries the ranges (`@strafe`, format version 2), and the CSV
records the yaw actually used (`yaw_bits`). The maths is in `physics.py`
(`strafe_yaw`, the same rule, used by the editor's estimate): with add-speed
capped at 30, gain is maximised with the wish direction perpendicular to the
velocity, giving `sqrt(speed² + 900)`; with
`accelspeed = airaccel·wishspeed·dt < 30`, at `v·wishdir = 30 − accelspeed`.
Ground ticks prestrafe with the same rule: friction first, then the most
acceleration from `sv_accelerate` (`ground_accelerate` in `physics.py` is the
check, a brute-force search over every yaw agrees). A tick that jumps aims as air.

### Recording from a tick (built)

A segment: the plugin plays the timeline on you from its start state, exactly as a
run plays it (inputs only; sped up with `host_timescale` until `tas_record_lead_in`
seconds before the tick, then at normal speed with a countdown), and from the tick on
your own inputs drive the engine and are recorded. The recording therefore starts
from the same state a run of the timeline would reach, with no teleport to a saved
position (which would skip the hidden state: stamina, slowdown, ducking, ground
friction). The editor splices it on (`editdoc.splice_recording`): everything before
the tick stays as it was edited, the recorded part becomes keyframes and key ranges
like an import, and the result is checked to bake to exactly the recorded inputs.
The splice refuses a recording whose first ticks differ from the current timeline
(it was edited meanwhile), except the yaw on auto-strafe ticks, which the game chose.
