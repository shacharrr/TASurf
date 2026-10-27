# TASurf

TASurf is a TAS (tool-assisted speedrun) editor for **Counter-Strike: Source Surf and Bhop gamemodes**, Designed as a video editor for source movement TASing.

Photos of the software in [Gallery](https://github.com/shacharrr/TASurf/tree/main/gallery)

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
TASurf can run either as an app or in the browser at http://127.0.0.1:8765

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
  player, and the plugin replaces every field of its commands.
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
3. The recording is loaded at once: `tas_run <number-of-times>` replays your exact inputs on the
   replay bot `<number-of-time>` times. Each replay should report `bit-identical` to the recording.

4. Outside the game, run the physics check on the recording's CSV, and compare
   the recording with a replay. `--rows-only` ignores the expected header
   difference, `status recorded` vs `complete`.

Example:
```bash
python -m tasurf verify --latest surf_beginner_rec --count 3 --rows-only
```

**Recording from a tick (segments).** In the editor, put the cursor where you want
to take over and press **● Record from here** (R). In the game the timeline plays
you there exactly (fast, then two seconds at normal speed with a countdown), then
it's your turn; press `tas_record` (your bind) to save. The editor joins your
recording on from that tick: everything before it stays as you edited it, and the
new part comes in as keyframes and key ranges, checked to be exact. It's undoable,
like any edit. (_This part is temporary and would become `saveloc` type_)

Recording also saves its start as `starts/<name>.json`, so you can write a
timeline from the same spot. Recordings stop automatically after
`tas_record_max_seconds` (default 600).

## Settings are never assumed

The timeline declares what it expects in `settings`. The plugin reads the real
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
  interp.py                 keyframe interpolation (hold / linear / smooth / cubic / joint)
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
