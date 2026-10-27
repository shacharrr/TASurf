# Timeline format (version 1)

A timeline is a hand-edited JSON file describing a run: where it starts, which
server settings it expects, the view angles over time, and which keys are held
when. `python -m tasurf bake` turns it into the flat per-tick file the plugin
plays (see [file-formats.md](file-formats.md)).

Everything is in **ticks**. Tick 0 is the first command played after the reset.

Any object key that starts with `_` is a comment and is ignored, and a list
entry made only of `_` keys (e.g. `{"_comment": "..."}`) is skipped. Unknown
keys are errors, so typos such as `"form"` instead of `"from"` are caught.

```json
{
  "format": "tasurf-timeline",
  "version": 1,
  "name": "example_surf",
  "map": "surf_beginner",
  "length": 330,
  "settings": { "tickrate": 66.6667, "sv_airaccelerate": 150, "sv_gravity": 800 },
  "start": { "origin": [-64.0, 3616.0, -31.0], "angles": [0.0, 180.0, 0.0] },
  "input": { "move_speed": 400 },
  "yaw":   { "default_interp": "linear", "keys": [[0, 180.0, "hold"], [66, 180.0], [130, 140.0]] },
  "pitch": { "keys": [[0, 0.0]] },
  "keys": [
    { "key": "W", "from": 0, "to": 66 },
    { "key": "jump", "from": 66, "to": 67 },
    ["D", 67, 130]
  ]
}
```

## Top level

| field      | required | meaning |
|------------|----------|---------|
| `format`   | yes | always `"tasurf-timeline"` |
| `version`  | yes | always `1` |
| `name`     | no  | output name (`<name>.tas`, run CSVs `<name>_NNN.csv`). Letters, digits, `_ - .`. Defaults to the file name. |
| `map`      | yes | map the timeline is for. The plugin refuses to run it on another map. |
| `length`   | yes | number of ticks to play |
| `settings` | no  | server settings the run expects (below) |
| `start`    | yes | start state object, or the name of a start file (below) |
| `input`    | no  | `{"move_speed": 400}`: forwardmove/sidemove magnitude of a held movement key. 400 is the CS:S default of `cl_forwardspeed`/`cl_sidespeed`. |
| `yaw`      | yes | yaw track (below) |
| `pitch`    | no  | pitch track; if absent, pitch stays at the start state's pitch |
| `keys`     | no  | key ranges (below) |
| `strafe`   | no  | auto-strafe ranges (below) |
| `autobhop` | no  | `true`: holding jump jumps again the moment you land (below). Default `false`. |
| `camera_smoothing` | no | what replays look along: `"strafe"` (default), `"all"` or `"off"` (below). |
| `turns`    | no  | the editor's curve turns, by name only (below); not baked |

## settings

Names are convar names, plus the special names `tickrate` and `tick_interval`.
Values are numbers or strings without spaces.

```json
"settings": { "tickrate": 66.6667, "sv_airaccelerate": 150, "sv_gravity": 800, "sv_maxvelocity": 3500 }
```

Nothing here is applied automatically. Before every run the plugin reads the
real values from the game and refuses to run if any differ (`tas_strict_settings 1`,
the default). Numbers are compared with a tolerance of 0.01% (tickrate: ±0.01).
`tas_applysettings` sets the convars to these values. `tickrate` can only be
changed by restarting the game with `-tickrate N`. CS:S's default is 66.6667,
which is a tick interval of 0.015 s.

Every run CSV records the actual values of a fixed list of movement convars,
whether or not the timeline mentions them.

## start

Either an object, or a string naming a start file:

```json
"start": "surf_beginner"
```

A string is looked up as `<name>` or `<name>.json`, first relative to the
timeline's folder, then in the game's `addons/sourcemod/data/tasurf/starts/`
(where `tas_setstart` writes; the baker finds that folder through `cstrike_dir`
in `tasurf.local.json`).

| field | default | meaning |
|-------|---------|---------|
| `origin` | required | `[x, y, z]` feet position |
| `angles` | required | `[pitch, yaw, roll]`; roll must be 0 |
| `velocity` | `[0,0,0]` | |
| `origin_bits`, `angles_bits`, `velocity_bits` | none | exact float32 bit patterns as 8-digit hex strings, written by `tas_setstart`. When present they are used instead of the decimals, so the run starts from exactly the captured state. If you edit a decimal by hand, delete its `_bits` entry; the baker refuses mismatched pairs. |
| `on_ground` | `"auto"` | `true`, `false` or `"auto"`. `auto` lets the engine's own ground check decide during the settle tick. |
| `ground_entity` | `0` | entity index stood on when `on_ground` is `true` (0 = world) |
| `base_velocity`, `base_velocity_flag` | `[0,0,0]`, `false` | trigger_push state at capture time |
| `ducked` | `false` | must be false: ducked starts aren't supported yet |
| `gravity` | `1.0` | player gravity scale (`m_flGravity`; 0 means 1) |
| `lagged_movement` | `1.0` | `m_flLaggedMovementValue` |
| `stamina` | `0.0` | CS:S jump/landing slowdown (`m_flStamina`, ms left). 0 = rested. Always reset: without it, jumping just before `tas_run` slows the run's first second of walking. |
| `velocity_modifier` | `1.0` | CS:S damage slowdown (`m_flVelocityModifier`). 1 = no slowdown. Always reset. |
| `view_offset_z`, `health` | not touched | restored when present (`tas_setstart` captures them) |
| `maxspeed` | not checked | player `m_flMaxspeed` at capture (260; captured only with a weapon in hand). The plugin refuses to run if it differs. |
| `map` | none | if present, must equal the timeline's `map` |
| `format`, `version` | | `"tasurf-start"`, `1` (written by `tas_setstart`) |

## Tracks: yaw and pitch

```json
"yaw": {
  "default_interp": "linear",
  "keys": [
    [0, 180.0, "hold"],
    { "tick": 66, "value": 180.0 },
    [130, 140.0, "smooth"],
    [200, 190.0]
  ]
}
```

A key is `[tick, value]`, `[tick, value, "interp"]`, `[tick, value, "interp", slope]` or
`{"tick": t, "value": v, "interp": "...", "slope": s}`. Ticks must be strictly increasing
and inside `0 .. length-1`. Values are degrees. `slope` (optional, degrees per tick) is
the curve's slope at that key wherever a `cubic` segment starts or ends there, in place of
the estimate from the neighbouring keys. The editor writes slopes when you bend the curve,
so the bent part meets the rest without a corner.

**Yaw is unwrapped.** 170 → 190 turns +20° through 180; 170 → −170 turns −340°.
Write values continuously (…, 350, 370, 390, …) and interpolation never takes the
long way around. The baker warns when two neighbouring keys differ by more than
180° and prints the short-way value in case that's what you meant. Yaw is wrapped
to (−180, 180] only when baked. Yaw is absolute world yaw (0 = +x, 90 = +y), so
a map teleport that rotates a human player's view does not rotate the timeline.

A key's `interp` shapes the segment from that key to the next one:

| interp   | segment |
|----------|---------|
| `hold`   | stays at this key's value, jumps to the next key's value at its tick |
| `linear` | straight line |
| `smooth` | ease in/out (smoothstep): zero slope at both keys |
| `cubic`  | cubic Hermite through the keys, slopes from the neighbouring keys (non-uniform Catmull-Rom) unless a key gives its own, smooth across keys |

Before the first key the first value holds; after the last key the last value
holds.

## view_pitch

An optional third track, in the same shape: the pitch **replays** show. The
plugin (0.9 and later) sets it on the replay bot's eyes after each tick's
movement, which uses `pitch`, so it never changes the run. It is baked as
`@view_pitch` lines that the hash leaves out, so adding or editing it keeps the
same hash: previews stay up to date and runs still compare. Without it, replays
show `pitch`. The editor's Look tool (L) makes one from the run's path.

## keys

Each entry holds one key over a tick range:

```json
{ "key": "D", "from": 67, "to": 130 }
{ "key": "A", "from": 260, "to": 330, "value": 250 }
["jump", 66, 67]
```

**`to` is exclusive**: `from 67 to 130` holds D on ticks 67…129 (63 ticks). A
one-tick press is `from t to t+1`. `bake` prints every resolved range as
`first..last (N ticks)` so off-by-one mistakes are visible.

| key | aliases | button | movement |
|-----|---------|--------|----------|
| `forward` | `w` | IN_FORWARD | forwardmove +value |
| `back` | `s` | IN_BACK | forwardmove −value |
| `moveleft` | `a` | IN_MOVELEFT | sidemove −value |
| `moveright` | `d` | IN_MOVERIGHT | sidemove +value |
| `jump` | | IN_JUMP | |
| `duck` | `crouch` | IN_DUCK | |
| `walk` | `speed` | IN_SPEED | (button only; no client-side slowdown is applied) |
| `use`, `attack`, `attack2`, `reload` | | IN_USE, IN_ATTACK, IN_ATTACK2, IN_RELOAD | |
| `turnleft`, `turnright` | | IN_LEFT, IN_RIGHT (the `+left`/`+right` turn keys; the turn itself is in the yaw) | |
| `bit0` … `bit31` | | any other button bit, e.g. `bit16` for the scoreboard | |

Key names are case-insensitive. `value` (movement keys only) overrides
`input.move_speed` for that range. Opposite keys held together cancel, as on a
real client (W+S gives forwardmove 0 with both bits set). The same key can't be
in two overlapping ranges.

## strafe

Auto-strafe ranges. On these ticks the plugin doesn't play the yaw track: it
aims the held keys from the velocity the engine has at that moment.

```json
"strafe": [
  { "from": 177, "to": 212 },
  { "from": 300, "to": 340, "offset": 4 },
  [400, 450]
]
```

`to` is exclusive, as for keys. Ranges can't overlap. Each tick the wish direction of the held
movement keys is set at the max-gain angle to the velocity, on the side the keys strafe to (A
left, D right, W+A 45° left of the view, and so on). That angle is acos(c*/speed):

* **In the air**, c* = max(30 − sv_airaccelerate·wishspeed·tick interval, 0), which is exactly
  sideways whenever that product reaches 30, as it does with surf settings.
* **On the ground** it prestrafes. Friction comes off the speed first, as in the engine
  (speed −= max(speed, sv_stopspeed)·sv_friction·tick interval), then
  c* = max(wishspeed − sv_accelerate·wishspeed·tick interval, 0) with the wish speed cut to a third
  when ducked. Below c* the best is straight along the velocity, so a slow start just goes
  forward. A tick that jumps counts as air, because the engine leaves the ground before it moves.

`offset` (degrees, −90…90, default 0) adds to that angle: positive turns sharper and gains less
(or loses speed), negative turns wider. W or S alone strafe to the side the yaw track turns on
that tick.

**Straight.** With a `heading` (degrees, world yaw: the direction of travel to keep), the plugin
also picks the side each tick: A when the heading is to the left of the velocity, D otherwise,
replacing whichever side key is held (hold A or D over the range; the editor holds D). In the air
each tick then turns the velocity by atan(30/speed) towards the heading and gains the most there
is, so the path zigzags around the heading by one tick's turn (about 6° at 260 u/s, 1.7° at
1000 u/s) and goes straight:

```json
"strafe": [ { "from": 300, "to": 340, "heading": 87.5 } ]
```

The editor's Turn (T) makes one with the direction you're going at its first tick, and Freeze
writes out the A and D presses the game picked as well as its yaws.

**Swing.** That's the most speed in a straight line, but the side switches every tick or two, and the
camera with it. With a `swing` (degrees, more than 0, at most 90) the plugin keeps the side it played
the tick before until the direction is that far past the heading, then switches: fewer, longer sweeps
that look smoother, every tick still gaining the most there is, with the path weaving about ±swing
(a little less straight, so a little slower along the heading). A side lasts about
2 × swing / atan(30/speed) ticks: ±5° is about 6 ticks at 1000 u/s, under 2 at 260 u/s.

```json
"strafe": [ { "from": 300, "to": 340, "heading": 87.5, "swing": 5 } ]
```

**Gain: a smooth swing** (the editor no longer makes these: replays smooth the camera on auto-strafe instead,
at the full speed; timelines that have one still play it). A swing is still a zigzag: every tick turns the most there is (atan(30/speed)),
one way or the other, so the yaw has a corner at each switch. With a `gain` as well (how much of the most
speed gain to keep, 75 to 100 percent) the turn rate changes by at most a step a tick instead: it speeds up to
the most, brakes in time to stop turning at ±swing, and goes on through zero to turn the other way, so the
direction, and the view, which looks along it, follow a smooth curve. Turning less than the most takes the
wish direction short of square to the velocity, some W besides A or D (the plugin sets forwardmove and
sidemove to it), which adds less speed. 75 is the smoothest (the step a sine of that size needs); 100 is two
whole turns a step, straight from one side's most to the other's: the most speed, the yaw zigzags as a plain
swing's. The step for a gain g is that of "close to perfect" c = 1 − 2·√(1 − g/100), which keeps about
1 − (1 − c)²/4 of the gain. It only smooths sweeps of more than about two ticks' turn (±5° at 1000 u/s is
fine; at 300 u/s it takes ±17° or so), and only in the air. (A first version wrote `smooth`, how close to
perfect from 0 to 100; it's read as the gain it keeps.)

```json
"strafe": [ { "from": 300, "to": 340, "heading": 87.5, "swing": 5, "gain": 90 } ]
```

Only the yaw input is chosen. Position and velocity still come from the engine
alone, and the same state gives the same yaw, so runs stay bit-identical.
Without a movement key, below 1 u/s (or stopped by friction), or with W or S alone
and no turn, the tick plays the yaw track, which is why the track still has to be
there. Not modelled, so the aim is a little off there (never wrong): a surface's own
friction (taken as 1) and the landing slowdown (stamina). Run CSVs record the yaw
that was played, and the editor's **Freeze** turns a range into keyframes
holding exactly those yaws.

## autobhop

CS:S only jumps on a tick where jump is pressed, the player is on the ground, and jump was **not**
held on the tick before, so holding jump through a landing doesn't jump again. With
`"autobhop": true` the plugin lets go of a held jump on every tick that starts in the air (not on a
ladder, in water or in noclip), as surf servers do: one long jump range then jumps on the first tick
after each landing, before any ground friction.

```json
"autobhop": true,
"keys": [ { "key": "jump", "from": 2, "to": 200 } ]
```

Without it, bunnyhop with separate presses: one tick each, with jump up in between. Timelines the
editor creates have it on; imported recordings don't need it (they hold the jumps as you played
them). Run CSVs record the buttons as played, jump let go in the air.

## camera_smoothing

What replays (the bot's eyes, renders) look along. On auto-strafe the game looks along the velocity,
which zigzags a little each tick for the most speed. Once the game has played the same inputs, the
editor bakes a camera with the run as `@view_yaw` lines (see file-formats.md): the yaw played, each tick
averaged with the ticks around it (never across a teleport). The run and its hash are the same whichever
you pick.

* `"strafe"` (the default): on the auto-strafe ranges, easing in and out over 16 ticks either side;
  elsewhere exactly the yaw played.
* `"all"`: on the whole run (also evens out a recording's hand shake; sharp turns get very slightly rounder).
* `"off"`: the yaw played everywhere, zigzag and all, a robotic look.

Written only when not the default (the editor's Settings, Camera).

```json
"camera_smoothing": "all"
```

## turns

The curve turns the editor's Turn made, so it can show each as one block on the yaw row, open it in
the Turn dialog again, or take it away. They only name a stretch: the yaw keyframes there are what
plays, and nothing here is baked, so the run and its hash are the same with or without them.

```json
"turns": [ { "from": 140, "to": 181, "rate": -1.2, "shape": "linear" } ]
```

`to` is exclusive; `rate` is degrees per tick (positive turns left); `shape` is `linear` (constant
speed) or `smooth` (ease in and out). Inserting and deleting ticks moves and cuts them as it does
key ranges, and a new turn over part of one cuts it.
