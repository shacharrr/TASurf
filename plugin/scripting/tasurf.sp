// TASurf: offline TAS input playback for CS:S surf.
//
// Loads a baked per-tick input file, resets the player it plays on (the replay
// bot, or with tas_bot 0 you) to its start state, feeds one input per user
// command through OnPlayerRunCmd, and records the engine's resulting state
// every tick to a CSV. During a run, movement comes from the inputs alone:
// position and velocity are written only by the reset (Start_Apply), and only
// before tick 0.

#include <sourcemod>
#include <sdktools>
#include <sdkhooks>
#include <cstrike>

#pragma semicolon 1
#pragma newdecls required

#define TAS_VERSION "0.15.0"

public Plugin myinfo =
{
	name = "TASurf",
	author = "TASurf",
	description = "Offline TAS input playback, recording and path drawing for surf",
	version = TAS_VERSION,
	url = ""
};

#include "tasurf/types.sp"
#include "tasurf/util.sp"
#include "tasurf/baked.sp"
#include "tasurf/strafe.sp"
#include "tasurf/settings.sp"
#include "tasurf/start.sp"
#include "tasurf/record.sp"
#include "tasurf/draw.sp"

int g_RunState = Run_Idle;
int g_RunClient;
int g_RunTick;              // next timeline tick to play
int g_SettleLeft;
int g_RunSettleTicks;
int g_SkippedCmds;
int g_StartCmdNum;
int g_StartServerTick;
int g_TickBaseBefore;
bool g_HasTickBase;
bool g_HasEyeAngles;        // m_angEyeAngles, where replays show the view pitch
float g_RunMaxspeed;
int g_BatchTotal;
int g_BatchDone;
int g_BatchIdentical;
int g_BatchDiverged;

#include "tasurf/recorder.sp"
#include "tasurf/mode.sp"
#include "tasurf/bot.sp"
#include "tasurf/remote.sp"
#include "tasurf/preview.sp"
#include "tasurf/render.sp"
#include "tasurf/nokick.sp"
#include "tasurf/autobhop.sp"
#include "tasurf/falldamage.sp"

public void OnPluginStart()
{
	Baked_Init();
	Record_Init();
	Draw_Init();
	BuildPath(Path_SM, g_LogPath, sizeof(g_LogPath), "%s/tasurf.log", TAS_DATA_DIR);
	EnsureDataDirs();

	CreateConVar("tasurf_version", TAS_VERSION, "TASurf plugin version", FCVAR_NOTIFY | FCVAR_DONTRECORD);
	g_cvSettleTicks = CreateConVar("tas_settle_ticks", "1",
		"Reset ticks played at the start state (re-applied every tick, neutral inputs) before tick 0. They flush state left over from before the reset, such as last tick's buttons and ground friction.",
		_, true, 1.0, true, 66.0);
	g_cvStrictSettings = CreateConVar("tas_strict_settings", "1",
		"1: refuse to run when game settings or player maxspeed differ from the timeline. 0: warn and run anyway.",
		_, true, 0.0, true, 1.0);
	g_cvAutoDraw = CreateConVar("tas_autodraw", "1", "Draw the path when a run finishes.", _, true, 0.0, true, 1.0);
	g_cvDrawMaxSegments = CreateConVar("tas_draw_max_segments", "600",
		"Maximum beam segments per drawn path; longer runs use one segment per several ticks.", _, true, 1.0, true, 5000.0);
	g_cvDrawBudget = CreateConVar("tas_draw_budget", "16",
		"Beams sent per server frame while a path is drawn.", _, true, 1.0, true, 64.0);
	g_cvMarkerInterval = CreateConVar("tas_marker_interval", "0",
		"White marker every N ticks. 0 = one per second of game time.", _, true, 0.0);
	g_cvBeamWidth = CreateConVar("tas_beam_width", "2.0", "Path beam width.", _, true, 0.1, true, 64.0);
	g_cvBeamMaterial = CreateConVar("tas_beam_material", "materials/sprites/laserbeam.vmt",
		"Path beam material (precached at map start).");
	g_cvSpeedMin = CreateConVar("tas_color_speed_min", "-1", "Horizontal speed drawn blue. -1 = slowest point of the run.");
	g_cvSpeedMax = CreateConVar("tas_color_speed_max", "-1", "Horizontal speed drawn red. -1 = fastest point of the run.");
	g_cvDrawZOffset = CreateConVar("tas_draw_z_offset", "2.0", "Draw the path this far above the player's origin (feet).");
	g_cvRecordMaxSeconds = CreateConVar("tas_record_max_seconds", "600",
		"tas_record stops and saves automatically after this many seconds of game time.", _, true, 1.0, true, 3600.0);
	g_cvRecordLeadIn = CreateConVar("tas_record_lead_in", "2.0",
		"Recording from a tick: seconds before your turn that the replay slows to normal speed, with a countdown.", _, true, 0.0, true, 30.0);
	g_cvViewFollow = CreateConVar("tas_view_follow", "1",
		"During tas_run, turn your camera with the replayed yaw/pitch every tick (view angles only; cosmetic).",
		_, true, 0.0, true, 1.0);
	g_cvDrawGhost = CreateConVar("tas_draw_ghost", "1",
		"Also draw the previous run of the same timeline, thin and grey, for comparison.", _, true, 0.0, true, 1.0);
	g_cvPreviewTimescale = CreateConVar("tas_preview_timescale", "20",
		"host_timescale used while a preview plays (how many times faster than real time).", _, true, 1.0, true, 50.0);
	g_cvPreviewLockView = CreateConVar("tas_preview_lock_view", "1",
		"With tas_bot 0: keep your view where you were standing while a preview plays on you (rendering only).", _, true, 0.0, true, 1.0);
	g_cvBot = CreateConVar("tas_bot", "1",
		"1: runs and previews play on a replay bot, and you edit in noclip (tas_mode). 0: they play on you, as a normal player.",
		_, true, 0.0, true, 1.0);
	g_cvBotName = CreateConVar("tas_bot_name", "TAS", "Name of the replay bot.");
	g_cvDrawBox = CreateConVar("tas_draw_box", "1",
		"Draw the player's collision box (standing or ducked) at the tick selected in the editor.", _, true, 0.0, true, 1.0);
	g_cvSessionCfg = CreateConVar("tas_session_cfg", "1",
		"1: run cfg/tasurf_session.cfg at the start of every map, after the server's own configs (no AI bots, long rounds, ...). 0: don't.",
		_, true, 0.0, true, 1.0);
	NoKick_Init();
	AutoBhop_Init();
	FallDamage_Init();
	AutoExecConfig(true, "tasurf");
	g_cvBot.AddChangeHook(OnBotConVarChanged);

	RegConsoleCmd("tas_help", Cmd_Help, "List TASurf commands");
	RegConsoleCmd("tas_setstart", Cmd_SetStart, "tas_setstart [name] [still] - save your current state as a start state");
	RegConsoleCmd("tas_load", Cmd_Load, "tas_load <name> - load data/tasurf/baked/<name>.tas");
	RegConsoleCmd("tas_run", Cmd_Run, "tas_run [count] - play the loaded timeline (count times, comparing runs)");
	RegConsoleCmd("tas_preview", Cmd_Preview, "Play the loaded timeline sped up (tas_preview_timescale) and put you back afterwards");
	RegConsoleCmd("tas_stop", Cmd_Stop, "Stop the current run (saves a recording)");
	RegConsoleCmd("tas_record", Cmd_Record, "tas_record [name] | tas_record from <tick> - record your own play (from your spot, or from a tick of the loaded timeline); again to save");
	RegConsoleCmd("tas_draw", Cmd_Draw, "tas_draw [run csv] - draw the last run's path, or a saved run");
	RegConsoleCmd("tas_clear", Cmd_Clear, "Stop drawing the path");
	RegConsoleCmd("tas_status", Cmd_Status, "Show the loaded timeline, settings check and run state");
	RegConsoleCmd("tas_applysettings", Cmd_ApplySettings, "Set convars to the loaded timeline's expected values");
	RegConsoleCmd("tas_mode", Cmd_Mode, "tas_mode [edit|record] - noclip editing (runs play on the replay bot) or playing yourself");

	Remote_Init();
	HookEvent("player_death", Event_PlayerDeath);
	HookEvent("player_spawn", Event_PlayerSpawn);
	for (int i = 1; i <= MaxClients; i++)  // loaded while people are in game
	{
		if (IsClientInGame(i))
		{
			Mode_OnClientPutInServer(i);
			FallDamage_OnClientPutInServer(i);
		}
	}
}

public void OnMapStart()
{
	Draw_Clear();
	Draw_Precache();
	g_LastRecord.Clear();
	g_GhostRecord.Clear();
	g_LastComplete = false;
	Remote_ResetDraw();  // draw the editor's choice again on the new map
	Mode_OnMapStart();
	Bot_OnMapStart();
}

public void OnClientPutInServer(int client)
{
	Mode_OnClientPutInServer(client);
	FallDamage_OnClientPutInServer(client);
}

public void OnConfigsExecuted()
{
	Draw_Precache();  // cfg/sourcemod/tasurf.cfg may have changed tas_beam_material
	NoKick_Apply();   // and server.cfg mp_autokick
	// the session's settings, after server.cfg and the map's own configs so they hold on every map: nobody has to
	// exec it (the replay bot joins seconds later, so its bot_kick only clears AI bots)
	if (g_cvSessionCfg.BoolValue && FileExists("cfg/tasurf_session.cfg", true))
	{
		ServerCommand("exec tasurf_session");
	}
}

public void OnMapEnd()
{
	if (g_RunState != Run_Idle)
	{
		Run_Abort("aborted: map ended");
	}
	Render_OnMapEnd();
	Bot_OnMapEnd();
}

public void OnClientDisconnect(int client)
{
	if (g_RunState != Run_Idle && client == g_RunClient)
	{
		Run_Abort("aborted: player disconnected");
	}
}

public void Event_PlayerDeath(Event event, const char[] name, bool dontBroadcast)
{
	if (g_RunState != Run_Idle && GetClientOfUserId(event.GetInt("userid")) == g_RunClient)
	{
		Run_Abort("aborted: player died");
	}
}

public void Event_PlayerSpawn(Event event, const char[] name, bool dontBroadcast)
{
	int client = GetClientOfUserId(event.GetInt("userid"));
	if (g_RunState != Run_Idle && client == g_RunClient)
	{
		Run_Abort("aborted: player respawned (round restart?)");
	}
	if (client > 0 && !IsFakeClient(client))
	{
		Mode_OnSpawn(client);
	}
}

// --------------------------------------------------------------------------
// playback

// runner: who the run plays on (the replay bot, or you). viewer: who gets the messages.
bool Run_CheckReady(int runner, int viewer)
{
	if (runner <= 0 || !IsClientInGame(runner))
	{
		TAS_Print(viewer, "no player in game to run");
		return false;
	}
	if (!IsPlayerAlive(runner))
	{
		TAS_Print(viewer, runner == viewer ? "you need to be alive (join a team) to run" : "the replay bot isn't alive yet; try again in a moment");
		return false;
	}
	char map[PLATFORM_MAX_PATH];
	GetCurrentMap(map, sizeof(map));
	if (!StrEqual(map, g_BakedMap, false))
	{
		TAS_Print(viewer, "the timeline is for %s but the current map is %s", g_BakedMap, map);
		return false;
	}

	bool strict = g_cvStrictSettings.BoolValue;
	bool ok = true;
	int mismatches = Settings_Report(viewer, false);
	if (mismatches > 0)
	{
		if (strict)
		{
			TAS_Print(viewer, "refusing to run: %d setting(s) differ from the timeline. Use tas_applysettings (or tas_strict_settings 0).", mismatches);
			ok = false;
		}
		else
		{
			TAS_Print(viewer, "warning: running with %d setting(s) different from the timeline", mismatches);
		}
	}
	if (g_Start.hasMaxspeed)
	{
		if (runner == Bot_Get())
		{
			Weapons_KnifeOnly(runner);
		}
		float maxspeed = GetEntPropFloat(runner, Prop_Send, "m_flMaxspeed");
		if (FloatAbs(maxspeed - g_Start.maxspeed) > 0.01)
		{
			if (runner == viewer)
			{
				TAS_Print(viewer, "your maxspeed is %.2f but the start state was captured at %.2f: hold the same weapon (maxspeed 1 means freeze time).",
					maxspeed, g_Start.maxspeed);
			}
			else
			{
				TAS_Print(viewer, "the replay bot's maxspeed is %.2f (knife) but the start state was captured at %.2f: capture starts holding a knife or pistol, or remove \"maxspeed\" from the start (maxspeed 1 means freeze time).",
					maxspeed, g_Start.maxspeed);
			}
			if (strict)
			{
				ok = false;
			}
		}
	}
	return ok;
}

void Run_Begin(int client, int viewer)
{
	g_RunClient = client;
	g_RunViewer = viewer;
	if (client == Bot_Get())
	{
		Bot_PrepareRun(client);
	}
	g_Record.Clear();
	g_RunTick = 0;
	g_RunSettleTicks = g_cvSettleTicks.IntValue;
	g_SettleLeft = g_RunSettleTicks;
	g_SkippedCmds = 0;
	g_StartCmdNum = -1;
	g_StartServerTick = GetGameTickCount();
	g_HasTickBase = HasEntProp(client, Prop_Send, "m_nTickBase");
	g_HasEyeAngles = HasEntProp(client, Prop_Send, "m_angEyeAngles[0]");
	g_RunMaxspeed = GetEntPropFloat(client, Prop_Send, "m_flMaxspeed");
	g_RunSettings.Clear();
	Settings_Snapshot(g_RunSettings);
	g_RunState = Run_Settling;
	char on[64];
	if (client != viewer)
	{
		strcopy(on, sizeof(on), " on the replay bot");
	}
	if (g_Preview)
	{
		TAS_Print(viewer, "preview: %s (%d ticks) at %.0fx speed%s", g_BakedName, g_Inputs.Length, g_cvPreviewTimescale.FloatValue, on);
	}
	else if (g_Recording && g_RecordFrom > 0)
	{
		TAS_Print(viewer, "recording %s: the timeline plays you to tick %d (sped up until %.1f s before), then it's your turn; tas_record again (or tas_stop) saves",
			g_BakedName, g_RecordFrom, g_cvRecordLeadIn.FloatValue);
		TAS_Chat(viewer, "replaying to tick %d, then your turn - tas_record saves", g_RecordFrom);
	}
	else if (g_Recording)
	{
		TAS_Print(viewer, "recording %s from your current spot: play now; tas_record again (or tas_stop) saves it", g_BakedName);
		TAS_Chat(viewer, "recording %s - tas_record again to save", g_BakedName);
	}
	else if (g_BatchTotal > 1)
	{
		TAS_Print(viewer, "run %d/%d: %s (%d ticks)%s", g_BatchDone + 1, g_BatchTotal, g_BakedName, g_Inputs.Length, on);
	}
	else
	{
		TAS_Print(viewer, "run: %s (%d ticks)%s", g_BakedName, g_Inputs.Length, on);
	}
}

public Action OnPlayerRunCmd(int client, int &buttons, int &impulse, float vel[3], float angles[3],
	int &weapon, int &subtype, int &cmdnum, int &tickcount, int &seed, int mouse[2])
{
	if (g_RunState == Run_Idle || client != g_RunClient)
	{
		if (IsFakeClient(client) && client == Bot_Get())
		{
			// the replay bot between runs: whatever its AI wants, it does nothing
			buttons = 0;
			impulse = 0;
			weapon = 0;
			vel[0] = 0.0;
			vel[1] = 0.0;
			vel[2] = 0.0;
			return Plugin_Changed;
		}
		if (!IsFakeClient(client) && g_cvAutoBhop.BoolValue && AutoBhop_Apply(client, buttons))
		{
			return Plugin_Changed;  // you, playing: auto bhop
		}
		return Plugin_Continue;
	}
	g_TickBaseBefore = g_HasTickBase ? GetEntProp(client, Prop_Send, "m_nTickBase") : 0;

	if (g_RunState == Run_Settling)
	{
		Start_Apply(client, g_Start, true);
		buttons = 0;
		vel[0] = 0.0;
		vel[1] = 0.0;
		vel[2] = 0.0;
		angles[0] = g_Start.angles[0];
		angles[1] = g_Start.angles[1];
		angles[2] = 0.0;
	}
	else
	{
		if (g_RunTick == 0)
		{
			Start_Apply(client, g_Start, false);  // the last write of position/velocity
			g_LastButtons = 0;
			g_LastSidemove = 0.0;  // no tick before the first
			g_HavePrevStartYaw = false;
			if (g_StartCmdNum < 0)
			{
				g_StartCmdNum = cmdnum;
			}
		}
		if (g_Recording && g_RunTick >= g_RecordFrom)
		{
			// The player's own inputs. Only fields a replay can't reproduce
			// (upmove, roll, and impulse/weapon/mouse below) are zeroed.
			vel[2] = 0.0;
			angles[2] = 0.0;
			if (g_cvAutoBhop.BoolValue)
			{
				AutoBhop_Apply(client, buttons);  // recorded as played, so the replay doesn't need it
			}
		}
		else
		{
			TickInput ti;
			g_Inputs.GetArray(g_RunTick, ti, sizeof(ti));
			buttons = ti.buttons;
			vel[0] = ti.forwardmove;
			vel[1] = ti.sidemove;
			vel[2] = 0.0;
			angles[0] = ti.pitch;
			angles[1] = ti.yaw;
			angles[2] = 0.0;
			float autoYaw, side, smoothFwd, smoothSide, smoothYaw;
			float startVel[3];
			GetEntPropVector(client, Prop_Data, "m_vecAbsVelocity", startVel);
			g_PendingStartYaw = RadToDeg(Atan2(startVel[1], startVel[0]));  // where it's going as this tick starts
			bool smoothed = ti.strafe && AutoStrafe_Smooth(client, ti, smoothFwd, smoothSide, smoothYaw);
			if (smoothed)
			{
				// a smooth swing: the keys are the wish direction (some W, and A or D), the view along the velocity
				ti.forwardmove = smoothFwd;
				ti.sidemove = smoothSide;
				vel[0] = smoothFwd;
				vel[1] = smoothSide;
				buttons = (buttons & ~(IN_FORWARD | IN_BACK | IN_MOVELEFT | IN_MOVERIGHT))
					| (smoothFwd > 0.0 ? IN_FORWARD : 0) | (smoothSide < 0.0 ? IN_MOVELEFT : IN_MOVERIGHT);
				angles[1] = smoothYaw;
			}
			else if (ti.strafe && AutoStrafe_Side(client, ti, side))
			{
				// straight: A or D, whichever steers towards the heading (the keys' bits follow, as a client's would)
				ti.sidemove = side;
				vel[1] = side;
				buttons = (buttons & ~(IN_MOVELEFT | IN_MOVERIGHT)) | (side < 0.0 ? IN_MOVELEFT : IN_MOVERIGHT);
			}
			if (g_AutoBhop && AutoBhop_Apply(client, buttons))
			{
				ti.buttons = buttons;  // the timeline's auto bhop: jump let go in the air
			}
			if (!smoothed && ti.strafe && AutoStrafe_Yaw(client, g_RunTick, ti, autoYaw))
			{
				angles[1] = autoYaw;  // auto-strafe: aimed from the velocity the engine has now
			}
			// what replays show: auto-strafe's camera (@view_yaw: the yaw a run of these inputs played,
			// smoothed, see camera.py); not while recording, so you take over from the yaw played
			float camYaw = ti.hasViewYaw && !g_Recording ? ti.viewYaw : angles[1];
			// Turn the view with the replay: your camera when the run plays on you, and always the
			// replay bot's eyes (what spectators, a render's first person view and its player model
			// show; the bot's own code otherwise holds its view angles where its AI left them, even
			// with bot_stop). Movement uses the command's angles set above; this only snaps the eye
			// angles (TeleportEntity with angles only: no position, no velocity, no trigger touches).
			// Run comparisons show it changes nothing. Previews too, so they stay the same as runs.
			bool bot = client == Bot_Get();
			if (bot || (g_cvViewFollow.BoolValue && !g_Preview && client == g_RunViewer))
			{
				float view[3];
				view[0] = g_HasViewPitch ? ti.viewPitch : ti.pitch;
				view[0] = view[0] > 89.0 ? 89.0 : (view[0] < -89.0 ? -89.0 : view[0]);
				view[1] = camYaw;
				view[2] = 0.0;
				TeleportEntity(client, NULL_VECTOR, view, NULL_VECTOR);
			}
		}
	}
	impulse = 0;
	weapon = 0;
	subtype = 0;
	mouse[0] = 0;
	mouse[1] = 0;
	return Plugin_Changed;
}

public void OnPlayerRunCmdPost(int client, int buttons, int impulse, const float vel[3], const float angles[3],
	int weapon, int subtype, int cmdnum, int tickcount, int seed, const int mouse[2])
{
	if (g_RunState == Run_Idle || client != g_RunClient)
	{
		return;
	}
	// The tick base only advances when the engine actually ran the command.
	// Dropped commands (sv_maxusrcmdprocessticks, pause) moved nothing, so the
	// same timeline tick is played again on the next command.
	if (g_HasTickBase && GetEntProp(client, Prop_Send, "m_nTickBase") == g_TickBaseBefore)
	{
		g_SkippedCmds++;
		return;
	}
	if (g_RunState == Run_Settling)
	{
		if (--g_SettleLeft <= 0)
		{
			g_RunState = Run_Playing;
			g_RunTick = 0;
			Render_Tick(client, 0);  // a render: the movie starts as tick 0 plays
		}
		return;
	}
	g_LastButtons = buttons;  // as played (auto-strafe's jump check on the next tick)
	g_LastSidemove = vel[1];  // and the side (a straight range's swing keeps it)
	g_PrevStartYaw = g_PendingStartYaw;  // and where it was going as it started (a smooth swing's turn rate)
	g_HavePrevStartYaw = true;
	Record_Capture(client, buttons, vel, angles);
	Draw_NoteHull(client);
	ViewPitch_Show(client, g_RunTick);
	Render_Tick(client, g_RunTick);
	g_RunTick++;
	if (g_Recording)
	{
		Recording_LeadIn(client);
		if (g_RunTick >= g_RecordMaxTicks)
		{
			Recording_Finish("recorded (reached tas_record_max_seconds)");
		}
	}
	else if (g_RunTick >= g_Inputs.Length)
	{
		Run_Finish("complete", true);
	}
}

// Replays show the timeline's view pitch: it goes on the player's eyes after the tick's
// movement, which used the command's own pitch, so the run is the same with or without it.
// Spectators watching in first person, and the player model, see m_angEyeAngles, which is
// sent after this; the next tick sets it from its own command first, then here again.
void ViewPitch_Show(int client, int tick)
{
	if (!g_HasViewPitch || !g_HasEyeAngles || g_Preview || (g_Recording && tick >= g_RecordFrom) || tick >= g_Inputs.Length)
	{
		return;
	}
	TickInput ti;
	g_Inputs.GetArray(tick, ti, sizeof(ti));
	float pitch = ti.viewPitch;
	if (pitch > 89.0)
	{
		pitch = 89.0;
	}
	else if (pitch < -89.0)
	{
		pitch = -89.0;
	}
	SetEntPropFloat(client, Prop_Send, "m_angEyeAngles[0]", pitch);
}

void Run_Abort(const char[] reason)
{
	char status[128];
	if (g_Recording)
	{
		// A recording keeps what was recorded up to here.
		int skip = StrContains(reason, "aborted: ") == 0 ? 9 : 0;
		FormatEx(status, sizeof(status), "recorded (ended: %s at tick %d)", reason[skip], g_RunTick);
		Recording_Finish(status);
		return;
	}
	if (g_RunState == Run_Settling)
	{
		FormatEx(status, sizeof(status), "%s during reset", reason);
	}
	else
	{
		FormatEx(status, sizeof(status), "%s at tick %d", reason, g_RunTick);
	}
	Run_Finish(status, false);
}

void Run_Finish(const char[] status, bool complete)
{
	if (g_Preview)
	{
		Preview_Finish(status, complete);
		return;
	}
	int runner = g_RunClient;
	int client = g_RunViewer;  // messages and drawing go to whoever watches
	bool inGame = client > 0 && IsClientInGame(client);
	int reportTo = inGame ? client : 0;
	g_RunState = Run_Idle;
	g_RunClient = 0;
	if (runner > 0 && runner == Bot_Get() && IsPlayerAlive(runner))
	{
		Bot_Idle(runner);  // frozen where the run ended
	}

	char comparison[256];
	bool compared = false, identical = false;
	if (complete && g_LastComplete && StrEqual(g_LastHash, g_BakedHash) && StrEqual(g_LastMap, g_BakedMap)
		&& g_LastSettle == g_RunSettleTicks)
	{
		compared = true;
		identical = Record_Compare(g_LastRecord, g_Record, comparison, sizeof(comparison));
	}
	else if (complete)
	{
		strcopy(comparison, sizeof(comparison), "no earlier complete run of this baked file on this map to compare with");
	}
	else
	{
		strcopy(comparison, sizeof(comparison), "not compared (run did not complete)");
	}

	ArrayList settingsNow = new ArrayList(ByteCountToCells(256));
	Settings_Snapshot(settingsNow);
	bool settingsChanged = !Settings_SnapshotsEqual(g_RunSettings, settingsNow);
	delete settingsNow;

	char csv[PLATFORM_MAX_PATH], csvName[PLATFORM_MAX_PATH], meta[320];
	int runNumber = Record_NextRunPath(csv, sizeof(csv));
	bool wrote = runNumber > 0 && Record_WriteCsv(csv, status, g_RunSettleTicks, g_RunMaxspeed, settingsChanged);
	BaseName(csv, csvName, sizeof(csvName));
	if (compared)
	{
		FormatEx(meta, sizeof(meta), "vs %s: %s", g_LastCsvName, comparison);
	}
	else
	{
		strcopy(meta, sizeof(meta), comparison);
	}
	if (wrote)
	{
		Record_WriteMeta(csv, status, g_StartCmdNum, g_StartServerTick, g_SkippedCmds, g_RunSettleTicks, meta);
	}
	if (g_RequestId[0] != '\0')
	{
		Remote_WriteResult(g_RequestId, status, wrote ? csvName : "-", meta);
		g_RequestId[0] = '\0';
	}

	float maxSpeed = Record_MaxSpeed(g_Record);
	TAS_Print(reportTo, "%s: %s, %d/%d ticks, max horizontal speed %.2f", g_BakedName, status, g_Record.Length, g_Inputs.Length, maxSpeed);
	if (wrote)
	{
		TAS_Print(reportTo, "  wrote %s", csv);
	}
	else
	{
		TAS_Print(reportTo, "  FAILED to write the run CSV %s", csv);
	}
	if (compared && identical)
	{
		TAS_Print(reportTo, "  determinism: %s, same as %s", comparison, g_LastCsvName);
		TAS_Chat(client, "%s: bit-identical to %s", csvName, g_LastCsvName);
	}
	else if (compared)
	{
		TAS_Print(reportTo, "  determinism: DIVERGED from %s, %s", g_LastCsvName, comparison);
		TAS_Chat(client, "%s DIVERGED from %s (see console)", csvName, g_LastCsvName);
	}
	else
	{
		TAS_Print(reportTo, "  determinism: %s%s", comparison, complete ? " (tas_run 2 plays it twice)" : "");
		TAS_Chat(client, "%s: %s", csvName, status);
	}
	if (settingsChanged)
	{
		TAS_Print(reportTo, "  warning: server settings changed during the run (map trigger or console?)");
	}
	if (g_SkippedCmds > 0)
	{
		TAS_Print(reportTo, "  note: the engine dropped %d command(s); the timeline waited for them, so the result is unaffected", g_SkippedCmds);
	}
	TAS_Log("%s %s: %s, %d ticks, %s", csvName, g_BakedHash, status, g_Record.Length, meta);

	// The previous last run becomes the ghost; this run becomes the last run
	// (compared against next time, and drawn).
	ArrayList oldGhost = g_GhostRecord;
	g_GhostRecord = g_LastRecord;
	strcopy(g_GhostName, sizeof(g_GhostName), g_LastName);
	strcopy(g_GhostMap, sizeof(g_GhostMap), g_LastMap);
	CopyVec3(g_LastStartOrigin, g_GhostStartOrigin);
	g_GhostStartSpeed = g_LastStartSpeed;
	g_LastRecord = g_Record;
	g_Record = oldGhost;
	g_Record.Clear();
	strcopy(g_LastName, sizeof(g_LastName), g_BakedName);
	g_LastComplete = complete;
	g_LastSettle = g_RunSettleTicks;
	strcopy(g_LastHash, sizeof(g_LastHash), g_BakedHash);
	strcopy(g_LastMap, sizeof(g_LastMap), g_BakedMap);
	strcopy(g_LastCsvName, sizeof(g_LastCsvName), csvName);
	CopyVec3(g_Start.origin, g_LastStartOrigin);
	g_LastStartSpeed = SquareRoot(g_Start.velocity[0] * g_Start.velocity[0] + g_Start.velocity[1] * g_Start.velocity[1]);

	if (inGame && g_cvAutoDraw.BoolValue && !Render_Active())  // not into a render's video
	{
		Draw_FromLastRun(client);
	}
	Render_RunEnded();

	if (compared)
	{
		if (identical)
		{
			g_BatchIdentical++;
		}
		else
		{
			g_BatchDiverged++;
		}
	}
	g_BatchDone++;
	if (complete && g_BatchDone < g_BatchTotal && runner > 0 && IsClientInGame(runner) && Run_CheckReady(runner, client))
	{
		Run_Begin(runner, client);
		return;
	}
	if (g_BatchTotal > 1)
	{
		TAS_Print(reportTo, "batch: %d of %d runs played; %d compared: %d bit-identical, %d diverged",
			g_BatchDone, g_BatchTotal, g_BatchIdentical + g_BatchDiverged, g_BatchIdentical, g_BatchDiverged);
		TAS_Chat(client, "batch: %d identical, %d diverged", g_BatchIdentical, g_BatchDiverged);
	}
	g_BatchTotal = 0;
}

// --------------------------------------------------------------------------
// commands

public Action Cmd_Help(int client, int args)
{
	TAS_Print(client, "TASurf %s commands:", TAS_VERSION);
	TAS_Print(client, "  tas_setstart [name] [still]  save your current state to data/tasurf/starts/<name>.json (name defaults to the map)");
	TAS_Print(client, "  tas_load <name>              load data/tasurf/baked/<name>.tas");
	TAS_Print(client, "  tas_run [count]              play the timeline on the replay bot from its start state (count times, comparing runs)");
	TAS_Print(client, "  tas_preview                  play the loaded timeline sped up (writes <name>.preview.csv)");
	TAS_Print(client, "  tas_stop                     stop the current run (or save the recording)");
	TAS_Print(client, "  tas_mode [edit|record]       edit: you noclip without weapons; record: you play normally (for tas_record)");
	TAS_Print(client, "  tas_record [name]            in record mode: record your own play from where you are; tas_record again saves it");
	TAS_Print(client, "  tas_draw [run]               draw the last run, or a saved run CSV from data/tasurf/runs");
	TAS_Print(client, "  tas_clear                    stop drawing");
	TAS_Print(client, "  tas_status                   loaded timeline, settings check, run state");
	TAS_Print(client, "  tas_applysettings            set convars to the timeline's expected values");
	return Plugin_Handled;
}

public Action Cmd_SetStart(int client, int args)
{
	if (g_RunState != Run_Idle)
	{
		TAS_Print(client, "a run is playing; tas_stop first");
		return Plugin_Handled;
	}
	int target = ResolveTarget(client);
	if (target <= 0 || !IsPlayerAlive(target))
	{
		TAS_Print(client, "you need to be alive in game to capture a start state");
		return Plugin_Handled;
	}

	char name[TAS_MAX_NAME], arg[TAS_MAX_NAME], map[PLATFORM_MAX_PATH];
	bool still = false;
	for (int i = 1; i <= args; i++)
	{
		GetCmdArg(i, arg, sizeof(arg));
		if (StrEqual(arg, "still", false))
		{
			still = true;
		}
		else if (name[0] == '\0')
		{
			strcopy(name, sizeof(name), arg);
		}
	}
	GetCurrentMap(map, sizeof(map));
	if (name[0] == '\0')
	{
		strcopy(name, sizeof(name), map);
	}
	if (!IsValidName(name))
	{
		TAS_Print(client, "start names may only use letters, digits, '_', '-' and '.'");
		return Plugin_Handled;
	}

	StartState st;
	char note[192];
	if (!Start_Capture(target, st, still, note, sizeof(note)))
	{
		TAS_Print(client, "%s", note);
		return Plugin_Handled;
	}
	char path[PLATFORM_MAX_PATH];
	BuildPath(Path_SM, path, sizeof(path), "%s/starts/%s.json", TAS_DATA_DIR, name);
	if (!Start_WriteJson(path, st, map))
	{
		TAS_Print(client, "could not write %s", path);
		return Plugin_Handled;
	}
	float speed = SquareRoot(st.velocity[0] * st.velocity[0] + st.velocity[1] * st.velocity[1]);
	TAS_Print(client, "saved start state %s", path);
	TAS_Print(client, "  origin %.3f %.3f %.3f  yaw %.3f pitch %.3f  speed %.3f  %s",
		st.origin[0], st.origin[1], st.origin[2], st.angles[1], st.angles[0], speed,
		st.onGround == Ground_Yes ? "on ground" : (st.onGround == Ground_No ? "in air" : "ground: auto"));
	if (note[0] != '\0')
	{
		TAS_Print(client, "  %s", note);
	}
	TAS_Print(client, "  use it in a timeline with  \"start\": \"%s\"  then bake", name);
	TAS_Log("tas_setstart %s on %s", name, map);
	return Plugin_Handled;
}

void Print_Loaded(int client)
{
	TAS_Print(client, "loaded %s: map %s, %d ticks, hash %s", g_BakedName, g_BakedMap, g_Inputs.Length, g_BakedHash);
	char map[PLATFORM_MAX_PATH];
	GetCurrentMap(map, sizeof(map));
	if (!StrEqual(map, g_BakedMap, false))
	{
		TAS_Print(client, "  note: the current map is %s", map);
	}
	if (g_SettingNames.Length == 0)
	{
		TAS_Print(client, "  the timeline declares no expected settings (the run CSV still records them)");
		return;
	}
	int mismatches = Settings_Report(client, true);
	if (mismatches > 0)
	{
		TAS_Print(client, "  %d setting(s) differ: tas_applysettings sets convars; tickrate needs a game restart with -tickrate", mismatches);
	}
	else
	{
		TAS_Print(client, "  all %d expected settings match the game", g_SettingNames.Length);
	}
}

public Action Cmd_Load(int client, int args)
{
	if (g_RunState != Run_Idle)
	{
		TAS_Print(client, "a run is playing; tas_stop first");
		return Plugin_Handled;
	}
	if (args < 1)
	{
		TAS_Print(client, "usage: tas_load <name>   (addons/sourcemod/%s/baked/<name>.tas)", TAS_DATA_DIR);
		return Plugin_Handled;
	}
	char name[PLATFORM_MAX_PATH], path[PLATFORM_MAX_PATH], error[256];
	GetCmdArg(1, name, sizeof(name));
	if (!Baked_ResolvePath(name, path, sizeof(path)))
	{
		TAS_Print(client, "no baked file %s (looked for %s)", name, path);
		return Plugin_Handled;
	}
	if (!Baked_Load(path, error, sizeof(error)))
	{
		TAS_Print(client, "loading %s failed: %s", path, error);
		return Plugin_Handled;
	}
	Print_Loaded(client);
	return Plugin_Handled;
}

public Action Cmd_Run(int client, int args)
{
	if (g_RunState != Run_Idle)
	{
		TAS_Print(client, "already running (tas_stop to abort)");
		return Plugin_Handled;
	}
	if (!g_HaveBaked)
	{
		TAS_Print(client, "no timeline loaded: tas_load <name>");
		return Plugin_Handled;
	}
	char error[256];
	if (!Baked_Load(g_BakedPath, error, sizeof(error)))  // pick up a re-bake
	{
		TAS_Print(client, "re-reading %s failed: %s", g_BakedPath, error);
		return Plugin_Handled;
	}
	int viewer = ResolveTarget(client);
	char why[192];
	int runner = Runner_Get(viewer, why, sizeof(why));
	if (runner <= 0)
	{
		TAS_Print(client, "can't run yet: %s", why);
		return Plugin_Handled;
	}
	if (!Run_CheckReady(runner, viewer))
	{
		return Plugin_Handled;
	}
	int count = 1;
	if (args >= 1)
	{
		char arg[16];
		GetCmdArg(1, arg, sizeof(arg));
		count = StringToInt(arg);
	}
	if (count < 1)
	{
		count = 1;
	}
	if (count > 100)
	{
		count = 100;
	}
	g_BatchTotal = count;
	g_BatchDone = 0;
	g_BatchIdentical = 0;
	g_BatchDiverged = 0;
	Run_Begin(runner, viewer);
	return Plugin_Handled;
}

public Action Cmd_Preview(int client, int args)
{
	if (g_RunState != Run_Idle)
	{
		TAS_Print(client, "already running (tas_stop to abort)");
		return Plugin_Handled;
	}
	if (!g_HaveBaked)
	{
		TAS_Print(client, "no timeline loaded: tas_load <name>");
		return Plugin_Handled;
	}
	char error[256];
	if (!Baked_Load(g_BakedPath, error, sizeof(error)))
	{
		TAS_Print(client, "re-reading %s failed: %s", g_BakedPath, error);
		return Plugin_Handled;
	}
	int viewer = ResolveTarget(client);
	char why[192];
	int runner = Runner_Get(viewer, why, sizeof(why));
	if (runner <= 0)
	{
		TAS_Print(client, "can't preview yet: %s", why);
		return Plugin_Handled;
	}
	if (!Run_CheckReady(runner, viewer))
	{
		return Plugin_Handled;
	}
	g_BatchTotal = 1;
	g_BatchDone = 0;
	Preview_Enter(runner, viewer);
	Run_Begin(runner, viewer);
	return Plugin_Handled;
}

public Action Cmd_Stop(int client, int args)
{
	if (g_RunState == Run_Idle)
	{
		TAS_Print(client, "nothing is running");
		return Plugin_Handled;
	}
	if (g_Recording)
	{
		Recording_Finish("recorded");
		return Plugin_Handled;
	}
	Run_Abort("stopped by tas_stop");
	return Plugin_Handled;
}

public Action Cmd_Record(int client, int args)
{
	if (g_Recording)
	{
		Recording_Finish("recorded");
		return Plugin_Handled;
	}
	if (g_RunState != Run_Idle)
	{
		TAS_Print(client, "a run is playing; tas_stop first");
		return Plugin_Handled;
	}
	int target = ResolveTarget(client);
	if (target <= 0 || !IsPlayerAlive(target))
	{
		TAS_Print(client, "you need to be alive in game to record");
		return Plugin_Handled;
	}
	char arg1[16];
	GetCmdArg(1, arg1, sizeof(arg1));
	if (args >= 2 && StrEqual(arg1, "from", false))
	{
		// a segment: the loaded timeline plays you to that tick, then it's your turn
		char tickArg[16], error[256];
		GetCmdArg(2, tickArg, sizeof(tickArg));
		if (!g_HaveBaked || !Baked_Load(g_BakedPath, error, sizeof(error)))
		{
			TAS_Print(client, "tas_record from <tick> needs a loaded timeline: tas_load <name>");
			return Plugin_Handled;
		}
		if (Mode_EditActive())
		{
			Mode_Set(Mode_Record, target);
		}
		if (Run_CheckReady(target, target))
		{
			Recording_BeginFrom(target, StringToInt(tickArg));
		}
		return Plugin_Handled;
	}
	if (Mode_EditActive())
	{
		// recording is you playing: switch to record mode first, so you can walk to the start
		Mode_Set(Mode_Record, target);
		TAS_Print(client, "walk to where the recording should start, then tas_record again to start it");
		return Plugin_Handled;
	}
	char base[TAS_MAX_NAME];
	if (args >= 1)
	{
		GetCmdArg(1, base, sizeof(base));
	}
	else
	{
		char map[PLATFORM_MAX_PATH];
		GetCurrentMap(map, sizeof(map));
		FormatEx(base, sizeof(base), "%s_rec", map);
	}
	if (!IsValidName(base) || strlen(base) > TAS_MAX_NAME - 5)
	{
		TAS_Print(client, "recording names may only use letters, digits, '_', '-' and '.' (max %d characters)", TAS_MAX_NAME - 5);
		return Plugin_Handled;
	}
	Recording_Begin(target, base);
	return Plugin_Handled;
}

public Action Cmd_Draw(int client, int args)
{
	int target = ResolveTarget(client);
	if (target <= 0)
	{
		TAS_Print(client, "no player in game to draw for");
		return Plugin_Handled;
	}
	if (args < 1)
	{
		Draw_FromLastRun(target);
		return Plugin_Handled;
	}
	char name[PLATFORM_MAX_PATH], path[PLATFORM_MAX_PATH], error[256];
	GetCmdArg(1, name, sizeof(name));
	BuildPath(Path_SM, path, sizeof(path), "%s/runs/%s", TAS_DATA_DIR, name);
	if (!FileExists(path))
	{
		BuildPath(Path_SM, path, sizeof(path), "%s/runs/%s.csv", TAS_DATA_DIR, name);
	}
	if (!Draw_FromCsv(target, path, error, sizeof(error)))
	{
		TAS_Print(client, "can't draw %s: %s", name, error);
	}
	return Plugin_Handled;
}

public Action Cmd_Clear(int client, int args)
{
	Draw_Clear();
	TAS_Print(client, "path cleared (beams already sent fade within %.1f s)", g_DrawLife);
	return Plugin_Handled;
}

public Action Cmd_Status(int client, int args)
{
	TAS_Print(client, "TASurf %s, tick interval %.6f (%.4f tick)", TAS_VERSION, GetTickInterval(), 1.0 / GetTickInterval());
	char mode[16], why[192];
	Mode_Name(mode, sizeof(mode));
	int runner = Runner_Get(ResolveTarget(client), why, sizeof(why));
	if (!g_cvBot.BoolValue)
	{
		TAS_Print(client, "mode: off (tas_bot 0): runs and previews play on you");
	}
	else if (runner > 0)
	{
		TAS_Print(client, "mode: %s; replay bot: %N (%s)", mode, runner, g_BotStatus);
	}
	else
	{
		TAS_Print(client, "mode: %s; replay bot: %s (%s)", mode, g_BotStatus, why);
	}
	if (g_HaveBaked)
	{
		Print_Loaded(client);
	}
	else
	{
		TAS_Print(client, "no timeline loaded");
	}
	if (g_RunState == Run_Settling)
	{
		TAS_Print(client, "state: resetting (%d settle tick(s) left)", g_SettleLeft);
	}
	else if (g_RunState == Run_Playing && g_Recording)
	{
		TAS_Print(client, "state: recording %s, tick %d (tas_record again to save)", g_BakedName, g_RunTick);
	}
	else if (g_RunState == Run_Playing)
	{
		TAS_Print(client, "state: playing tick %d/%d (%d dropped commands so far)", g_RunTick, g_Inputs.Length, g_SkippedCmds);
	}
	else
	{
		TAS_Print(client, "state: idle");
	}
	if (g_LastRecord.Length > 0)
	{
		TAS_Print(client, "last run: %s, %d ticks, %s", g_LastCsvName, g_LastRecord.Length, g_LastComplete ? "complete" : "incomplete");
	}
	if (g_Drawing)
	{
		TAS_Print(client, "drawing: %d beams, lifetime %.2f s", g_DrawBeams.Length, g_DrawLife);
	}
	return Plugin_Handled;
}

public Action Cmd_ApplySettings(int client, int args)
{
	if (!g_HaveBaked)
	{
		TAS_Print(client, "no timeline loaded: tas_load <name>");
		return Plugin_Handled;
	}
	Settings_Apply(client);
	int mismatches = Settings_Report(client, false);
	TAS_Print(client, mismatches == 0 ? "all expected settings now match" : "some settings still differ (see above)");
	return Plugin_Handled;
}
