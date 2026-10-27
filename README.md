# TASurf

TASurf is a TAS (tool-assisted speedrun) editor for **Counter-Strike: Source Surf and Bhop gamemodes**, Designed as a video editor for source movement TASing.

_Automatically runs CS:S on `-insecure` to minimize VAC danger._

## Install (Windows)

**1. Prerequisits**:
* **Counter-Strike: Source** from Steam.
* **Metamod:Source** and **SourceMod**, see https://www.sourcemod.net/downloads.php?branch=stable for installation
  (tested with Metamod:Source 1.12.0-git1227 and SourceMod 1.12.0-git7255).

* **SVR** (Source Video Render) by Crashfort,
  https://github.com/crashfort/SourceDemoRender/releases/latest.

**2. TASurf:**
Download the installer from the
[latest release](https://github.com/shacharrr/TASurf/releases/latest) and run it. It installs for your Windows user
only (no administrator rights needed) and adds TASurf to the Start menu. The installer isn't code-signed, so Windows
SmartScreen may say it protected your PC: click **More info**, then **Run anyway**.

**Updating:** run the newer installer.

**Uninstalling:** uninstall in `Installed Apps`

### Running
TASurf can run either as an app or in the browser at `127.0.0.1:8768`

## How it works

The timeline format is in [docs/timeline-format.md](docs/timeline-format.md), the
other files in [docs/file-formats.md](docs/file-formats.md), and the design in [docs/design.md](docs/design.md).

## Documentation

Currently there is no documentation. Most functionality is explained in the app in the help popup clickable at the right top or via `F1`.

### Commands

| command | |
|---|---|
| `tas_setstart [name] [still]` | save your current state as a start state |
| `tas_load <name>` | load `data/tasurf/baked/<name>.tas` and show the settings check |
| `tas_run [count]` | reset the replay bot to the start and play; with a count, play several times and compare each run with the previous one |
| `tas_stop` | stop the current run (a partial CSV is still written, marked as stopped), or save a recording |
| `tas_preview` | play the loaded timeline sped up; writes `runs/<name>.preview.csv` |
| `tas_mode [edit\|record]` | edit: you noclip without weapons (the default); record: you play normally, for `tas_record`. No argument switches between them |
| `tas_record [name]` | in edit mode it switches to record mode first. In record mode: record your own play from your current spot (same reset as `tas_run`); run it again to save. Writes a run CSV and a replayable `<name>.tas` (name defaults to `<map>_rec`, never overwrites), and loads it |
| `tas_record from <tick>` | a segment: the loaded timeline plays you to that tick (sped up, then `tas_record_lead_in` seconds at normal speed with a countdown), then you play and are recorded; `tas_record` saves. The editor's **Record from here** does this and joins the result on |
| `tas_draw [run]` | draw the last run, or a saved run, e.g. `tas_draw example_surf_001` |
| `tas_clear` | stop drawing (beams already sent fade within about a second) |
| `tas_status` | loaded timeline, settings check, run state |
| `tas_applysettings` | set convars to the loaded timeline's expected values (not tickrate) |
| `tas_help` | list commands |

### ConVars (`cfg/sourcemod/tasurf.cfg`, created on first load)

| convar | default | |
|---|---|---|
| `tas_settle_ticks` | 1 | reset ticks before tick 0 (see [design.md](docs/design.md#reset-sequence)) |
| `tas_strict_settings` | 1 | refuse to run on a settings or maxspeed mismatch |
| `tas_autodraw` | 1 | draw the path after each run |
| `tas_draw_max_segments` | 600 | beam segments per path (long runs use one segment per several ticks) |
| `tas_draw_budget` | 16 | beams sent per server frame |
| `tas_marker_interval` | 0 | white marker every N ticks; 0 = once per second |
| `tas_beam_width`, `tas_beam_material`, `tas_draw_z_offset` | 2, `materials/sprites/laserbeam.vmt`, 2 | look of the path |
| `tas_color_speed_min/max` | -1 | speed range for blue→red; -1 = the run's own min/max |
| `tas_bot` | 1 | 1: runs and previews play on the replay bot and you edit in noclip. 0: no bot; they play on you, a normal player (TASurf 0.5 behaviour) |
| `tas_bot_name` | TAS | the replay bot's name |
| `tas_view_follow` | 1 | with `tas_bot 0`: turn your camera with the replayed yaw/pitch during `tas_run` (view angles only) |
| `tas_record_max_seconds` | 600 | recordings stop and save after this long |
| `tas_record_lead_in` | 2 | recording from a tick: seconds of normal speed (with a countdown) before your turn |
| `tas_draw_box` | 1 | draw the player's collision box at the tick selected in the editor |
| `tas_session_cfg` | 1 | run `cfg/tasurf_session.cfg` at the start of every map, after the server's own configs |
| `tas_no_kick` | 1 | hold `mp_autokick` at 0 (no idle or team-kill kicks); the old value comes back with 0 or on unload |
| `tas_fall_damage` | 0 | 0: no fall damage, as on surf servers: landing hard (at the bottom of a ramp, say) takes no health and doesn't start CS:S's damage slowdown. For you and the replay bot; every run records the value. 1: CS:S's own |
| `tas_autobhop` | 1 | while you play (between runs, and your own turn in `tas_record`): holding jump jumps again as you land. Recordings keep the jumps as played, so they replay the same either way. A timeline's runs use its own **Auto bhop** setting instead |
| `tas_draw_ghost` | 1 | also draw the previous run of the same timeline, thin and grey |
| `tas_preview_lock_view` | 1 | with `tas_bot 0`: keep your view where you were standing while a preview plays on you |
| `tas_preview_timescale` | 20 | how many times faster than real time a preview plays (`host_timescale`; turns on `sv_cheats` for the preview if it's off, and back off afterwards) |

### Edit mode, record mode and the replay bot (Ingame)

* **Edit mode** (the default / `tas_mode edit`): Noclip, was made for those who want to edit while looking at the game itself.
* **Record mode** (`tas_mode record`): Normal play, `tas_record` for starting and stopping a recording, editor will show the recording after you save it.
* **The replay bot** is a CS:S bot that TASurf adds to your team after you've
  joined one. It runs one command per tick through the same movement code as a
  player, and the plugin replaces every field of its commands, so its own AI
  never moves it. A run resets it exactly like a player (same settle tick, same
  reset). Between runs it's frozen where its last run ended, out of everyone's
  way. It holds no weapon, so it runs at 260 on the ground, CS:S's speed without a
  weapon and what surf servers such as KSF play at (a knife or pistol would cap it
  at 250). In edit mode you have no weapon either, so `tas_setstart` leaves
  maxspeed out of the start.
* **Maps without a `.nav` file:** CS:S won't simply add a bot there. It starts
  generating a navigation mesh, which takes minutes and reloads the map. So
  TASurf writes a minimal `cstrike/maps/<map>.nav` (one area, no paths) and
  reloads the map once. Delete that file if you ever want CS:S to generate a
  real one for AI bots.
* `tas_bot 0` turns all of this off: no bot, no noclip, and runs play on you.

### Recording your own play

Use this for real surf data: physics checks on ramps and long air time, and
checking that replays reproduce real play.

1. Bind a key so you don't need the console mid-run: `bind r tas_record`.
2. In edit mode, the first press switches you to record mode. Walk to where you
   want to start (e.g. stand still in the start zone), press `r`, then surf. Press
   `r` again to stop and save. Dying or being respawned also saves what you had.
3. The recording is loaded at once: `tas_run 2` replays your exact inputs on the
   replay bot. Each replay should report `bit-identical` to the recording.

**Recording from a tick (segments).** In the editor, put the cursor where you want
to take over and press **● Record from here** (R). In the game the timeline plays
you there exactly (fast, then two seconds at normal speed with a countdown), then
it's your turn; press `tas_record` (your bind) to save. The editor joins your
recording on from that tick: everything before it stays as you edited it, and the
new part comes in as keyframes and key ranges, checked to be exact. It's undoable,
like any edit.
4. Outside the game, run the physics check on the recording's CSV, and compare
   the recording with a replay. `--rows-only` ignores the expected header
   difference, `status recorded` vs `complete`:

```bash
python -m tasurf verify --latest surf_beginner_rec --count 3 --rows-only
```

Recording also saves its start as `starts/<name>.json`, so you can write a
timeline from the same spot. Recordings stop automatically after
`tas_record_max_seconds` (default 600).

## Editor

The editor is for per-tick editing, with the path, speed and best-yaw hints next to it. The desktop app shows it in
its own window. In a source checkout you can also run it in your browser, from the TASurf folder (it opens
http://127.0.0.1:8765):

```bash
python -m tasurf editor
```

* **Setup page:** it opens first (and from **Setup** in the page bar, the TASurf logo in the header, or **M**; Esc
  goes back to Edit).
  * **Game & folders** (in the desktop app) finds the game and SVR, keeps the project folder and starts the game;
    see [Desktop app](#desktop-app).
  * **Open timeline** lists your timelines grouped by map, with the map the game has loaded first. To work on
    another map, start the game on it or load it in the game. When the game is on a different map from the open
    timeline, the header says so; click that to get there.
  * **Create timeline** makes one from a start position you captured with `tas_setstart`, imports a
    recording, or copies a timeline. A new timeline starts at the start position, facing the same way, with no
    keys held. The start is copied in, so capturing that start again doesn't change it.
* **Help:** each page's own, from the **?** in its top right corner or **F1**: Setup's covers getting started and
  the game's commands, Edit's the editor's keys and how to read it, Export's how a render works.

  A timeline named in the address (`#name`, or the desktop app's argument) opens straight away.
* **Timeline ▾** (by Fit) says what each thing drawn on the timeline is: the keyframe colours (white
  linear, purple hold, green smooth, blue cubic), the auto-strafe bands, pale key bars (a key at its own
  speed), the speed graph's lines and the strip under it, the dashed line at the first tick your edits
  change. It also turns the extras on and off: the best yaw and the rows of buttons that don't move you
  (+left, +right and unnamed bits that recordings hold) are off at first. Hover a row's name for what the
  row is and how to edit it.
* **Yaw row:** the yaw curve. Drag keyframes to another tick and yaw (**Alt**: the keyframe stays on its tick and only its yaw changes; see **moving a keyframe** below); double-click adds one on the curve.
  **Smooth** (in the inspector, by Curve) makes the yaw smooth and differentiable through the keyframes of the
  selected ticks, or all of them without a selection: each becomes cubic, so holds no longer step, linear keys
  no longer make corners and smooth ones no longer stop the turn at every key. Where the smoothed stretch meets
  the rest, the key there keeps the slope the curve has, so that join has no corner either.
  The **best yaw** (dashed green, Timeline ▾ → Best yaw) is, for each air tick, the yaw that gives the most
  speed for the keys you hold, from the last run's velocity. With surf airaccelerate that's the
  perpendicular strafe, √(speed²+900). The inspector shows how many u/s it would gain on the selected tick,
  and **B** applies it, whether the line is shown or not.
* **Key rows** (W A S D jump duck, plus any other keys the timeline uses): drag a bar's edges or body, drag on
  empty space to create a range, double-click or press W/A/S/D/Space/C to toggle the selected tick. Where
  one movement key hands over to another on the same tick (A comes up where D goes down), dragging either
  edge, on its row or on the path, moves the switch: both edges go together, so no tick is left with
  neither key or both. **Alt** moves just the one.
* **Auto bhop** (top bar, by the length): holding jump jumps again the moment you land, as on surf servers,
  so one long jump bar hops on every landing. The game lets go of jump in the air; the jump row shows the
  bar you hold faint and the ticks it actually jumped bright. New timelines have it on; it's stored in the
  timeline (`"autobhop": true`), so its runs don't depend on the server. It needs plugin 0.12.
* **The preview: where you would go.** A moment after every edit (and when a timeline opens), the
  replay bot plays your current inputs, saved or not, with
  `host_timescale` raised (`tas_preview_timescale`, default 20×), so a 60-second route takes a few seconds.
  It sends back the exact path, which is the engine's own result, ramps and teleports included. The editor
  then draws it in speed colours on the path and in white on the speed graph, with your last real run in
  grey. It's always on, so the path you see is the game's exact answer a moment after you change
  anything, and the game follows along. A newer edit replaces a preview that's still waiting or playing, so the path catches up with
  your latest edit as fast as the game can play it. The edited inputs are loaded in game, so `tas_run`
  there plays them. The in-game path is redrawn the same way (preview in speed colours, last real run in
  grey), and chat says how the end speed changed. You stay where you are (held still while the
  preview plays), so you can watch the in-game path update. The game also draws the
  editor's current path as soon as you open a timeline: the newer of its latest preview and latest run,
  with the last real run in grey. **Where you are:** the in-game path is red up to the tick selected in the
  editor and white for the second after it (one tickrate of ticks), with the player's box at that tick
  (standing or ducked, the real size from the game) and a yellow post above it; it follows the cursor as
  you scrub. The top-down path shows the same box. That choice is kept in `requests/draw.txt`, so the lines come back by
  themselves after a plugin reload or map change. A preview from before
  your latest edits is drawn dotted until the next one arrives. The header shows the game's state
  (version, map, the replay bot), and auto-preview pauses with the reason when it isn't ready (for
  example while you're in record mode). A preview writes `runs/<name>.preview.csv` (overwritten each
  time) and doesn't count as a run. The game has to be running with a map loaded and the replay bot in
  the game. `tasurf_session.cfg` sets `engine_no_focus_sleep 0` so the game keeps full speed while
  you're alt-tabbed in the editor.
* **Time and selection** (as in Resolve): the **ruler** on top (the tick numbers, where the playhead's handle
  is) is time: click to put the cursor there, drag to scrub. Select ticks in the body: drag on empty space
  of the yaw row, the look row or the speed graph, Shift+drag anywhere, Shift+click (from the cursor), or
  `[` and `]` at the cursor (Esc clears). A plain click in the body clears the selection. Only the ruler
  moves the cursor: selecting and editing in the body (keyframes, key bars, blocks) leave it where it is,
  and with a keyframe or a block selected the inspector's Yaw group shows that instead of the cursor's tick.
* **Ticks: insert, delete, length.** **Insert** (I) adds copies of the
  cursor's tick before it (how many: the box under Ticks), with the same yaw, pitch and held keys.
  **Delete** (Shift+Delete) removes the selected ticks, or that many from the cursor. Making the
  **length** longer repeats the last tick (keys held at the end stay held); shorter cuts the end off.
  Everything after the edit moves by whole ticks and bakes to the same inputs as before. Where an edit
  cuts through a `cubic` segment, it gets one keyframe there with the slope the curve has, and the keys at
  its ends get theirs, so the curve stays the same smooth cubic (a part of a cubic is a cubic). A `smooth`
  segment can't be cut that way, so the part that's kept is written out as one keyframe per tick.
* **Turn… (T): auto-strafe or a generated turn.** Pick the ticks (the selection, or one second from the
  cursor) and left or right. **Auto-strafe** (the default) doesn't draw a curve: the game aims the held
  key from your real velocity every tick, exactly sideways for the most speed in the air and as a
  prestrafe on the ground (friction first, then the most ground acceleration), so it follows whatever a
  ramp does to the velocity and joins a recorded section without the loss a hand-made curve gets (one
  that trails the velocity by just 2.75° at 625 u/s gains nothing; where this comes from:
  [Where the auto-strafe math comes from](#where-the-auto-strafe-math-comes-from)). **Turn harder by** a few degrees
  turns sharper at some speed. With auto-strafe you don't pick W A S D: the direction sets them (left
  holds A, right holds D, W and S are released); only jump and duck are yours to set. **Straight**
  keeps going one way at the most speed: the game also picks A or D each tick (whichever steers back
  towards the **heading**, which starts as the direction you're going at the first tick), so the path
  zigzags around it by one tick's turn (about 6° at 260 u/s, 1.7° at 1000 u/s). **Replays smooth the
  camera there** (plugin 0.15): the game looks along the velocity, which zigzags with it, and the run plays
  that as it is, all its speed. Once the game has played these exact inputs (the preview after each edit,
  or a run), the editor knows every yaw the run will play, so when it sends a run, a preview or a render it
  bakes a camera: that path's yaw on the auto-strafe ranges averaged with the ticks either side of each
  (about 3.5 ticks wide, so no lag and no corners), easing in and out over 16 ticks either side, never
  across a teleport. What replays show (the bot's eyes, a render, your view when the run plays on you)
  looks along it; the movement still uses the yaw played. At 1800 u/s the shake goes from ±0.5° to a few
  hundredths. The light blue line on the yaw row is that camera, and the 3D view looks along it. Before
  the game has played the inputs (or with a plugin older than 0.15) replays show the yaw played.
  **Settings, Camera** (saved with the timeline, so its renders always look the same) picks what replays,
  renders and the 3D view look along: auto-strafe smoothed (the default), the **whole run** smoothed (every
  tick averaged with the ticks around it, which also evens out a recording's hand shake), or **off**, a
  robotic look: the yaw played, zigzag and all. Under Yaw, ↻ sets a straight range's heading to where you're
  going now, and Freeze writes out the A and D presses too. It needs plugin 0.11. **Strafing: Perfect or
  Swing.** Perfect switches A and D every tick or two, for the most speed in a straight line. **Swing ±X°**
  keeps a side until the direction is X° past the heading: fewer, longer sweeps (each tick still the most
  speed, the path weaving about ±X°); the dialog says about how many ticks a side lasts at your speed. Swing
  needs plugin 0.13. (For a while Swing had a "speed kept" slider that turned smoothly for less speed; the
  smoothed camera does that job at the full speed, so it's gone. A range made with it says so in Turn, and
  applying it there plays it at the most speed.) **After auto-strafe the yaw goes on from where it ended** (Settings, on): the tick after a
  range starts at the yaw the game played on its last tick (from the game's preview, or the prediction until
  then), so the view doesn't snap back to the curve; a flat end after it moves along. The range shows green on the yaw row with the yaw the game played, and
  under Yaw you can **Freeze** it into keyframes holding exactly those yaws (to edit by hand) or
  **Remove** it. Or a curve: a speed in °/tick (it shows °/s and the total too; or type the total),
  constant speed or ease in and out (either one meets the curve before and after it with the turn it has
  there, so the yaw gets no corners: constant speed eases into its speed over at most 8 ticks and out of it
  again, so the total comes out a little different, and the message says what it was), and for each of W A
  S D jump duck whether to hold it, release it or
  leave it as it is over those ticks (turning left defaults to holding A and releasing D). The turn
  starts from the yaw there now unless you give a start yaw. **Use best-yaw rate** takes how fast the
  best yaw (Timeline ▾ → Best yaw) turns over those ticks in the last run. Afterwards, later keyframes
  either stay where they are, or turn along by the change the turn made. The preview shows what it does
  a moment later.
* **Turn blocks.** Every turn Turn makes is a block on the yaw row: auto-strafe in green, a curve turn
  (constant speed or ease in and out) in blue, each with the keys held there on its strip. Click a block's
  strip (the top of the yaw row) to select the whole block. **T** (or **Edit…** under Yaw, or a double-click
  on the strip) opens it in the Turn dialog with its own settings, to change and apply over the same ticks.
  **Delete** (or ✕) takes its effect away: auto-strafe goes back to the yaw curve; a curve turn's
  keyframes go and the yaw runs in one smooth curve from the keyframe before the block to the one after,
  meeting the rest with the slopes it has there (A and D then follow the yaw). The curve turns are kept in
  the timeline's `turns`, which only names them and isn't baked.
* **Look… (L): the pitch replays show.** The run's own pitch goes into the movement (very slightly: the
  engine rescales the flattened view direction in float32), so a timeline can also have a **view pitch**
  that only replays show. The plugin puts it on the replay bot's eyes each tick, apart from the movement, so
  the run doesn't change, and spectators watching the bot in first person (and renders) see it. Since 0.10.2
  the bot's eyes also turn with the yaw it plays: before, CS:S's bot code held its view where its AI left it,
  so the run moved right but a spectator looked the wrong way. Look… makes
  one from the path, like a player looking where they go: it aims at where the player will be a moment
  later (**Look ahead**), follows part of the path's climbs and drops (**Follow**), a little below level
  (**Look down**), smoothed (**Smoothing**), never across a teleport, plus a hand that's never quite still
  (**Hand**: a slow wobble and a drift while the yaw turns fast; **New** gives another one). The dialog
  draws the new curve over the path's height before you apply it. It shows on its own **look** row under
  the yaw, in the inspector's Pitch group (**Run** and **Replay**, each with a keyframe diamond), and in the
  3D view's **Eye** camera. It's baked as `@view_pitch` lines the hash leaves out, so the preview stays
  up to date and runs still compare. With an older plugin in the game, Run says to update it.
* **On the path: keyframes and keys** (as in Bunnymod XT's TAS editor). In the top view and in 3D, each yaw
  keyframe is a diamond with an arrow pointing its yaw (in 3D a post with the arrow on top), and each
  place a key goes down is a dot, and where it comes up a ring, in its timeline colour (a one-tick press,
  such as a jump: a dot in a ring). Drag one along the path to move it to another tick, or a keyframe's
  arrow to turn its yaw (Shift: ten times finer); a click selects it. Where they crowd, every few are
  drawn. **Show ▾ → Keyframes and keys** turns them off.
* **Moving a yaw keyframe** (dragging it on the path or on the yaw row, or typing its angle) moves just
  that key. The curve from the keyframe before it to the one after becomes a cubic through it, meeting the
  rest with the slopes the curve had at those two keys (they get their own slopes, see the timeline format),
  so it has no corners or steps; beyond them nothing changes (the stretch shows in yellow while you drag).
  The one exception is a flat end: when the curve holds one yaw from the next keyframe to the end of the run
  (as ticks Insert adds at the end do), that end follows the key, turning by the same change, so the run ends
  at the yaw you turned to. **A and D follow the yaw** (Settings, on by default): on the ticks whose yaw the
  move changed, a tick turning left holds A and one turning right holds D; remove a key afterwards to turn
  without one. While you drag, the instant prediction redraws the path from the
  first tick that changed, and the label by the mouse (on the yaw row: the status line) says the predicted
  speed a second after the keyframe and how it compares with before.
* **The instant prediction** (dashed, in speed colours on the path in the top view and in 3D, orange on the
  speed graph): the editor
  plays your edited inputs itself, at once, from the first tick you changed, starting from the exact
  state of the newest preview or run there. It uses a copy of the game's player movement (friction,
  ground and air acceleration, jumps with CS:S stamina, ducking as CS:S does it, sliding along ramps and
  walls, steps, the ground) against the map's own brushes, with its teleports and pushes, a few
  milliseconds a frame, nearest ticks first, so a drag shows where it goes while you drag. Replaying
  recorded runs from 245 starting ticks, it stayed within a unit of the game for typically 100 to 300 ticks
  (1.5 to 4.5 s), and never for less than 30. It can't see displacements, props, moving brushes or map
  logic, and the game's float32 rounding makes it drift the further it goes; the game's preview replaces
  it. Without the map's file it falls back to the **estimate** (the air model until the first surface).
* **Speed lost hitting things** (as Bunnymod XT's TAS editor colours frames that hit a wall): red rings on
  the path (red posts in 3D, red bars hanging from the top of the speed graph), bigger for more, where the
  run loses speed hitting a wall, a ramp's edge or a ceiling, or landing badly on a ramp. The floor doesn't
  count, and neither does riding a ramp (a flat one costs a tiny fraction of a u/s a tick; under 5 u/s a
  tick isn't shown). Past the first edited tick it's the prediction's; before it, the exact path's, each
  tick played again from the state the game left (a tick that doesn't come out where and as fast as the game
  had it counts as nothing). The inspector's **Hits** group says what this tick lost, how many ticks lost
  how much in all, and ‹ › jump to the hit before or after the cursor. **Show ▾ → Speed lost hitting
  things** turns it off.
* **Speed graph:** the last run (light), the run before it (grey) and the prediction (dashed orange). The
  strip under the graph shows ground (green), ramp contact (blue) and teleports/events (red).
* **Layout** (after DaVinci Resolve's Edit page):
  * The **top bar** names the open timeline and its map, with its length (Insert and Delete change it) and
    **the game's state**: a dot and a few words (Game ready, Previewing, Game on another map, Game not running)
    with what you can do about it (Start…, Switch map). You open and create timelines on the start screen
    (Setup, M). **Opening a timeline on another map switches the game to that map** (plugin 0.15): the game
    changes once no run is playing, and a preview waits for it.
  * The **viewer** is in the middle, with the **inspector** on the right. It shows the cursor's tick in
    groups: Tick, Selection, Yaw, Pitch, Keys, Last run, Preview, Prediction, Hits and Runs. Click a group's header
    to fold it; the header still shows its main value (the yaw, the speed, the preview's state). The diamond
    beside Yaw and Pitch is filled on a keyframe. Click it to remove that keyframe, or to add one at the
    curve's value. The keys are keycaps in their timeline colours: click one to hold or release it on this
    tick.
  * Below them are the **toolbar** (Undo, Insert, Delete, Turn, Look, Record from here, Preview, Run, Fit) and the
    **timeline** across the full width, with its ruler and red playhead on top.
  * The **page bar** at the bottom has the pages: Setup, Edit and Export.
  * Drag the borders between panels to resize them. **Inspector** in the top bar hides and shows the
    inspector, and the enlarge button in the viewer hides it too. Sizes are remembered.
* **Viewer (V switches Top / 3D):** the run from above (coloured by speed, with the run before in grey, the
  prediction in orange, your keyframes and keys on it and the player's box at the cursor; click it to jump to
  that tick) or in 3D. **Show ▾** picks what's drawn: in the editor the last run, the run before, the
  prediction, the box, the keyframes and keys and the speed lost hitting things; in the game the last run in grey and the player's box.
* **Transport** (under the viewer): ⏮ ◀ ▶ ▶| ⏭ play the run at the game's own speed (or 0.1× to 2×), with
  the time, tick and speed beside them. ⟳ loops. With ticks selected, it plays the selection.
  **Shift+Space** plays and pauses.
* **3D:** the viewer's **3D** button draws the timeline's map from its `.bsp` file
  (from `cstrike/maps` or `cstrike/download/maps`). **Textured** (on unless you turn it off) shows it as
  the game does: every face's material and texture, found where the game finds them (in the map itself,
  then the game's VPK archives and folders, then `cstrike/download`), under the map's baked lighting.
  Displacements blend their two textures, glass and water are see-through, and tool surfaces the game
  doesn't draw (triggers, clips, nodraw, sky) are left out. A map compiled without lighting is shown
  fullbright, as the game shows it. Until a material has loaded, or if it can't be found, its faces get its
  average colour; the bar says how many are missing. The map's sky is drawn round everything, and its props
  (the models placed on it: trees, crates, and on some maps whole ramps) stand where the game puts them, lit
  by a light from above at about the map's brightness (the game's own lighting of models isn't in the
  map). The Export page's preview draws the same. Not drawn yet: a 3D skybox's scenery (the miniature
  landscape some maps show round the playable area) and reflections. With **Textured** off, floors are grey, surf ramps blue and walls dark, on a faint 256-unit
  grid. **Triggers** draws the map's triggers as see-through volumes coloured by type (teleports purple,
  pushes green, hurts red, `trigger_multiple` zones amber, ...), built from their brushes as the game's
  collision sees them. **▾** beside it lists the types in the map with their counts: tick the ones to draw
  (All, None). Soundscapes are off until you tick them, and the choice is remembered. Point at a trigger to
  see its type and name, such as `trigger_multiple · bonus3_ronin_endzone`. The run is drawn in speed colours, dimmer after the cursor's tick. You also get the
  player's box at the cursor and a green post at the start. Surfaces are only drawn from the front, so from
  behind a wall you see through it, and the run also shows faintly through walls. Clicking the path jumps
  to that tick. Cameras:
  * **Follow** orbits the player at the cursor: drag (or hold the right button) to turn, wheel to zoom,
    double-click to get behind them.
  * **Eye** is first person at the cursor, with the yaw the game played and the pitch replays show.
  * **Free** flies as in noclip. Hold the right button to look: the cursor locks and the raw mouse turns the
    view at the game's default sensitivity (3 × 0.022°). Move with W A S D, Q and E for down and up, Shift
    for faster; the wheel sets the speed. Where the cursor can't be locked, the right button drags the view.

  While playing, the 3D view and the playhead are drawn between ticks on every screen frame, blending the
  position and view as the game does between its 66 ticks a second (never across a teleport).

* **Runs at normal speed** are no longer a button: the preview gives the same exact path. `tas_run` in
  the game still plays the loaded timeline at normal speed on the replay bot, and Export renders it.
* **Settings** (⚙ in the top bar, Ctrl+,), remembered in this browser: **Auto-save** (on: every 5 minutes, when there are
  changes; Ctrl+S saves at once), **A and D follow the yaw** (on), and the **keyboard**: click an
  action's key and press the new one (Esc keeps it, Backspace leaves it without one; a key another action had
  moves over). Letters and digits go by their place on the keyboard, so shortcuts work in any layout.
* **Importing a recording** (Setup → Create timeline → From a recording) turns a `tas_record` recording into a timeline. The import is lossless (it bakes back
  to the recorded inputs bit for bit), so you start from an exact copy of your own run and the recording
  serves as the baseline until you've run it. From the command line:

```bash
python -m tasurf import-recording surf_beginner_rec
```

The bar under the timeline shows the whole run: drag its bright window to move, the window's edges to
zoom, click to jump, double-click to see everything. In an auto-strafe range a strip on the yaw row shows
which key it strafes with where (◀ A, D ▶, or "no key: curve"). On a straight range it shows the side the game
played on each tick (after an edit, the side the prediction says it will play): mostly A and D in turn, and one
side for a while where something knocked the velocity off the heading and it steers back.

Keys: ← → step ticks (Shift ×10, Ctrl ×100), K keyframe, I insert, Shift+Delete delete ticks, T turn, L look,
Delete removes the selected keyframe or range, Ctrl+Z/Y undo/redo, Ctrl+S save, F fit, wheel zoom,
right-drag pan. **Keys** in the editor lists them all.

## Where the auto-strafe math comes from

Nothing in auto-strafe is tuned by trial: it is worked out from the game's own movement code. The same rule
is in the plugin (`plugin/scripting/tasurf/strafe.sp`), the back end (`strafe_yaw` and `straight_sidemove`
in `tasurf/physics.py` and `rust/src/physics.rs`) and the editor's prediction
(`tasurf/editor_static/predict.js`).

* **The engine code.** CS:S moves the player with Valve's `CGameMovement`, published in the Source SDK 2013
  (https://github.com/ValveSoftware/source-sdk-2013, `src/game/shared/gamemovement.cpp`). In the air,
  `AirAccelerate` caps the wish speed at `GetAirSpeedCap()`, which is 30 (`gamemovement.h`), works out
  `addspeed = 30 − velocity·wishdir`, and adds `min(sv_airaccelerate · wishspeed · tick interval · surface
  friction, addspeed)` along the wish direction. On the ground `Friction` runs first, then `Accelerate` does
  the same with the full wish speed in place of the cap (cut to a third while ducked,
  `HandleDuckingSpeedCrop`). CS:S's own changes to this code aren't published, so the model is checked
  against real runs (below).
* **The best angle.** Call c the speed the velocity already has along the wish direction and a the speed
  added. The new speed² is speed² + 2ac + a², and with a = min(A, 30 − c) that's largest at
  c* = max(30 − A, 0) (A = sv_airaccelerate · wishspeed · tick interval). So the wish direction goes at
  acos(c*/speed) to the velocity. With surf's airaccelerate A is over 30, so c* = 0: exactly sideways, and
  the new speed is √(speed² + 900). On the ground it's the same with c* = max(wishspeed − A, 0),
  A = sv_accelerate · wishspeed · tick interval, after friction: the prestrafe angle. Source's movement
  code comes from Half-Life's, and this is the result the Half-Life Physics Reference derives in its
  chapter on strafing (J. Chong, https://www.jwchong.com/hl/strafing.html, section 6.3 "Maximum
  acceleration").
* **Straight.** At that angle the 30 u/s added is at right angles to the velocity, so each tick turns the
  velocity by atan(30/speed). Strafing left or right gains exactly the same speed, so choosing the side
  costs nothing. The game picks whichever side turns back towards the heading, so the path stays within
  one tick's turn of it. This part is TASurf's own rule, not taken from a reference.
* **Checked against the game.** `python -m tasurf check` re-predicts every free-flight air tick of a real
  run with this model and reports the √(speed² + 900) ticks separately (see **Determinism check**). Two things
  aren't modelled: a surface's own friction (taken as 1) and CS:S's landing slowdown (stamina). They make
  the aim a little off, never wrong.

## Settings are never assumed

The timeline declares what it expects (`settings`). The plugin reads the real
values from the game before every run and refuses to run on a mismatch.
`tas_applysettings` fixes convars; tickrate needs a restart with `-tickrate N`.
Every run CSV records the tick interval and about 25 movement convars as the
game had them, plus the player's maxspeed (weapon dependent).

## Determinism check

* **In game:** `tas_run 3`. Runs 2 and 3 should report
  `determinism: bit-identical (330 ticks), same as example_surf_00N.csv`. On a
  difference you get the first divergent tick and field.
* **Offline:** this compares the two newest runs byte for byte, then cell by
  cell (exact, via the `*_bits` columns). Exit code 0 means identical. Use
  `--count 3` to compare more, or pass CSV paths directly.

```bash
python -m tasurf verify --latest example_surf
```

* **Physics sanity check:** this re-predicts every free-flight air tick with
  the Source SDK 2013 air-movement model (half gravity, airaccelerate with the
  30 u/s cap, move, half gravity) and reports agreement, including the
  `sqrt(speed² + 900)` perpendicular-strafe case.

```bash
python -m tasurf check "C:/Program Files (x86)/Steam/steamapps/common/Counter-Strike Source/cstrike/addons/sourcemod/data/tasurf/runs/example_surf_001.csv"
```

## Troubleshooting

* **`meta` is an unknown command:** Metamod didn't load. Check that
  `cstrike\addons\metamod_x64.vdf` exists (64-bit game) and that the console
  log shows no error loading `addons/metamod/bin/win64/server`. A failure to load
  the 32-bit `addons/metamod/bin/server` from `metamod.vdf` is expected on the
  64-bit game and harmless.
* **SDK Tools `<FAILED>` on 64-bit:** please open an issue with the error log
  (`cstrike\addons\sourcemod\logs\errors_*.log`). The plugin needs
  SDKTools for `OnPlayerRunCmd`, teleporting and beams.
* **A jump drops you to about 286 u/s:** that's CS:S's bunnyhop cap. With
  `sv_enablebunnyhopping 0` (its default) every jump cuts your speed to 1.1 × your
  maxspeed (286 without a weapon, as TASurf plays; 275 with a knife). `tasurf_session.cfg` sets it to
  1, as surf servers do; a run's header records the value it had, and the editor's
  prediction applies the cap when that is 0.
* **The view jitters during playback, worst when ducking:** that's client-side
  prediction fighting the server-side inputs. Your client predicts your own
  (empty) keys while the server plays the timeline, so it's corrected every
  frame. The recorded run isn't affected (the CSV is server state).
  `cl_predict 0` may help, but it can only be set while not connected, i.e.
  before `map`. It also makes your own live movement lag behind, so it doesn't
  suit recording. I haven't verified it on a listen server.
* **The path steps up and down by about 8.5 units in the air:** that's CS:S
  lifting your feet when you duck in the air, and lowering them when you stand.
  The origin (feet) really moves; it's in the engine's state, not a glitch.
* **The run aborts with "player respawned":** the round ended. Use `exec tasurf_session`.
* **"your maxspeed is X":** you're holding a different weapon than at capture
  (none is 260; knife and pistols are 250).
* **"the replay bot's maxspeed is X but the start state was captured at Y":**
  the start was captured at a different speed. Capture it again, or delete
  `"maxspeed"` from the start.
* **The replay bot doesn't join:** `tas_status` says why. It joins your team a few
  seconds after you've joined one. `exec tasurf_session` kicks it, and it comes back.
  If the map had no `.nav` file, the first load reloads the map once.
* **The bot plays a run differently from you:** a recording replayed on the bot
  should be `bit-identical` to the recording. If it isn't, please open an issue with the console
  output; `tas_bot 0` plays runs on you as before.

## Development

### Repo layout

```
README.md
LICENSE                     GPL-3.0
docs/timeline-format.md     timeline JSON reference
docs/file-formats.md        baked file, start file, run CSV and metadata formats
docs/design.md              determinism model, reset sequence, known limits, later milestones
timelines/example_surf.json example timeline for surf_beginner
tasurf/                     Python package (stdlib only, Python 3.10+)
  timeline.py               load + validate timelines
  interp.py                 keyframe interpolation (hold / linear / smooth / cubic)
  bake.py                   timeline -> per-tick baked file (+ a reader with the plugin's rules)
  runcsv.py                 read run CSVs, bit-exact comparison
  physics.py                reference air-movement model for sanity checks
  editdoc.py                editor edits, JSON layout, recording import
  editor.py                 browser editor server (127.0.0.1)
  editor_static/            the editor page (index.html), its 3D view (view3d.js) and prediction (predict.js)
  __main__.py               CLI: bake, verify, check, build-plugin, install, editor, import-recording
tests/                      unittest suite (python -m unittest)
rust/                       the same back end in Rust: one native binary, no Python needed
  src/*.rs                  one module per Python one (py.rs: Python's number formatting, JSON and maths)
  parity.py                 runs Python and Rust side by side in a sandbox and compares every output
  testdata/                 CPython math results the Rust tests check bit for bit
  app/                      the editor as a desktop app (Tauri): its own window, the back end inside
  app/installer.nsh         what the installer puts beside the program
plugin/scripting/tasurf.sp  SourceMod plugin (+ tasurf/*.sp)
plugin/plugins/tasurf.smx   compiled plugin (build output, git-ignored)
plugin/cfg/tasurf_session.cfg  optional server config for a clean local session
tasurf.local.json           your game path (git-ignored; see tasurf.local.example.json)
third_party/                SourceMod/Metamod packages, used for spcomp (git-ignored)
```

### Building from source

You need Windows, Rust (https://rustup.rs) and, for the reference back end and its tests, Python 3.10 or newer (no
packages). The plugin is compiled with SourceMod's `spcomp`: extract the SourceMod zip into `third_party\sourcemod\`
in this repo as well as into the game.

1. `copy tasurf.local.example.json tasurf.local.json` and fix `cstrike_dir` (the desktop app's Setup page does
   this for you).
2. Build the plugin, and install it into the game (the app's **Launch ▶** installs it too):

```bash
python -m tasurf build-plugin
```

```bash
python -m tasurf install
```

**Launch options.** With the desktop app you don't need any: it starts the game itself (see **Desktop app**),
through SVR, which adds `-insecure` (no VAC for that session) and `-console`. Without the app, set CS:S launch
options in Steam to `-insecure -console` while you use TASurf, and remove them before playing on public servers.

### Desktop app

Build the app:

```bash
cargo build --release -p tasurf-app --manifest-path rust/Cargo.toml
```

That gives `rust\target\release\tasurf-editor.exe`. Double-click it or pin it to the taskbar. To open a
timeline straight away, pass its name, as with `tasurf editor`:

```bash
rust/target/release/tasurf-editor.exe surf_beginner_edit
```

* Keep the program inside the TASurf folder: it finds `timelines/` and `tasurf.local.json` from its own location
  (or set `TASURF_ROOT`).
* Closing the window asks about unsaved changes, as closing the browser tab does.
* The app and a browser editor (`tasurf editor`, port 8765) can run at the same time; they read and write the same
  files.
* Windows needs the WebView2 runtime, which Windows 11 has built in. On macOS and Linux the same code builds
  (Linux needs `webkit2gtk-4.1`; see https://v2.tauri.app/start/prerequisites/).
* The icon is drawn by `rust/app/icons/make_icons.py` (needs Pillow).

### Installer

The release installer is the Tauri CLI's NSIS bundle. Build the plugin first (it ships the compiled
`plugin/plugins/tasurf.smx`), then:

```bash
cargo install tauri-cli --version "^2" --locked
```

```bash
cd rust/app && cargo tauri build
```

That gives `rust\target\release\bundle\nsis\TASurf_<version>_x64-setup.exe`. The installer puts the program in
`%LOCALAPPDATA%\TASurf`, and `rust/app/installer.nsh` adds what it needs beside it: the plugin (sources, compiled
plugin, session config), the editor page and the example timeline. They aren't Tauri resources, which would also
be copied next to the program in `rust/target` on every build and make that folder look like a TASurf folder.

### Rust back end

`rust/` holds the whole back end (baker, checks, editor server, recording import) as a single native
binary. It takes the same commands and options as `python -m tasurf` and writes the same bytes: baked files,
saved timelines, run checks, editor answers and error messages. The Python package stays as the reference.

Build it once (needs Rust from https://rustup.rs; Windows, macOS and Linux):

```bash
cargo build --release --manifest-path rust/Cargo.toml
```

Then use `rust\target\release\tasurf.exe` (or `rust/target/release/tasurf` on macOS/Linux) wherever this
README says `python -m tasurf`, for example:

```bash
rust/target/release/tasurf.exe editor
```

It finds the TASurf folder from the current folder or its own location (set `TASURF_ROOT` to override) and
reads `tasurf.local.json` and `TASURF_CSTRIKE` / `TASURF_SPCOMP` as the Python one does. The editor page is
read from `tasurf/editor_static/index.html`, with a copy built into the binary if that file isn't there.

To check the two back ends still agree (it copies the repo and the game's data folder into a temporary
sandbox, so neither the game nor your timelines are touched):

```bash
python rust/parity.py
```

The Rust unit tests:

```bash
cargo test --release --manifest-path rust/Cargo.toml
```

### Tests

```bash
python -m unittest discover -s tests -t .
```

For the Rust back end, see [Rust back end](#rust-back-end): `cargo test` and `python rust/parity.py`.

## Licence

TASurf is free software under the GNU General Public License, version 3 or later: see [LICENSE](LICENSE). The
SourceMod plugin is GPL as SourceMod requires of its plugins. Metamod:Source, SourceMod and SVR are separate
projects with their own licences; TASurf doesn't include them.
