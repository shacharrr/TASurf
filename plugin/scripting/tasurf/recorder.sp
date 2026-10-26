// Recording live play (tas_record).
//
// The player's current state becomes the start, the run goes through the same
// reset as tas_run (settle tick + tick-0 reset), and from then on the
// player's own inputs drive the engine and are recorded every tick. On stop
// the recording is written as a run CSV and as a baked file holding the exact
// float bits of every input, and that baked file is loaded. tas_run then
// replays it and compares the replay with the live recording.

#define FNV_INIT  -2128831035  // 0x811C9DC5, FNV-1a 32 offset basis
#define FNV_PRIME 16777619     // 0x01000193

bool g_Recording;
int g_RecordMaxTicks;
int g_RecordFrom;        // recording from a tick: the timeline plays ticks before it, you from it on
int g_RecordSpeedUntil;  // ... sped up until this tick, then normal speed for the lead-in

// Settings a recording declares as expected, so its replay checks them.
char g_RecordSettingCvars[][] =
{
	"sv_gravity", "sv_airaccelerate", "sv_accelerate", "sv_friction", "sv_stopspeed",
	"sv_maxspeed", "sv_maxvelocity", "sv_stepsize", "sv_bounce"
};

// The value a replay reads back from a 6-decimal baked line.
float RoundTrip6(float v)
{
	char s[48];
	FormatEx(s, sizeof(s), "%.6f", v);
	return StringToFloat(s);
}

int Fnv1aLine(int h, const char[] line)
{
	for (int i = 0; line[i] != '\0'; i++)
	{
		h = (h ^ (line[i] & 0xFF)) * FNV_PRIME;
	}
	return (h ^ '\n') * FNV_PRIME;
}

bool Recording_Begin(int client, const char[] baseName)
{
	StartState st;
	char note[192];
	if (!Start_Capture(client, st, false, note, sizeof(note)))
	{
		TAS_Print(client, "%s", note);
		return false;
	}
	// Fields the baked file only stores as decimals: reset the live recording
	// to exactly what its replay will parse, so both start identically.
	for (int k = 0; k < 3; k++)
	{
		st.baseVelocity[k] = RoundTrip6(st.baseVelocity[k]);
	}
	st.gravity = RoundTrip6(st.gravity);
	st.laggedMovement = RoundTrip6(st.laggedMovement);
	st.velocityModifier = RoundTrip6(st.velocityModifier);
	st.stamina = RoundTrip6(st.stamina);
	st.viewOffsetZ = RoundTrip6(st.viewOffsetZ);
	st.maxspeed = RoundTrip6(st.maxspeed);

	char name[TAS_MAX_NAME], path[PLATFORM_MAX_PATH];
	Recording_FreeName(baseName, name, sizeof(name), path, sizeof(path));

	// The recording replaces the loaded timeline.
	g_Start = st;
	g_HaveBaked = false;
	g_HasViewPitch = false;
	g_AutoBhop = false;
	g_Inputs.Clear();
	g_SettingNames.Clear();
	g_SettingValues.Clear();
	char value[64];
	FormatEx(value, sizeof(value), "%.4f", 1.0 / GetTickInterval());
	g_SettingNames.PushString("tickrate");
	g_SettingValues.PushString(value);
	for (int i = 0; i < sizeof(g_RecordSettingCvars); i++)
	{
		ConVar cv = FindConVar(g_RecordSettingCvars[i]);
		if (cv != null)
		{
			cv.GetString(value, sizeof(value));
			g_SettingNames.PushString(g_RecordSettingCvars[i]);
			g_SettingValues.PushString(value);
		}
	}
	strcopy(g_BakedName, sizeof(g_BakedName), name);
	strcopy(g_BakedPath, sizeof(g_BakedPath), path);
	GetCurrentMap(g_BakedMap, sizeof(g_BakedMap));
	g_BakedHash[0] = '\0';

	// Also save the start, for hand-written timelines from the same spot.
	char startPath[PLATFORM_MAX_PATH];
	BuildPath(Path_SM, startPath, sizeof(startPath), "%s/starts/%s.json", TAS_DATA_DIR, name);
	Start_WriteJson(startPath, st, g_BakedMap);

	g_RecordMaxTicks = RoundToCeil(g_cvRecordMaxSeconds.FloatValue / GetTickInterval());
	g_RecordFrom = 0;
	g_RecordSpeedUntil = 0;
	g_Recording = true;
	g_BatchTotal = 1;
	g_BatchDone = 0;
	g_BatchIdentical = 0;
	g_BatchDiverged = 0;
	Run_Begin(client, client);
	return true;
}

// First free name: base, base_2, base_3, ... (never overwrite a recording)
void Recording_FreeName(const char[] baseName, char[] name, int namelen, char[] path, int pathlen)
{
	for (int i = 1; i < 1000; i++)
	{
		if (i == 1)
		{
			strcopy(name, namelen, baseName);
		}
		else
		{
			FormatEx(name, namelen, "%s_%d", baseName, i);
		}
		BuildPath(Path_SM, path, pathlen, "%s/baked/%s.tas", TAS_DATA_DIR, name);
		if (!FileExists(path))
		{
			return;
		}
	}
}

// Recording from a tick of the loaded timeline (a segment): its start state and its
// first `from` ticks are played on you exactly as a run plays them, sped up until
// tas_record_lead_in seconds before the tick, then at normal speed so you can get
// ready. From that tick on your own inputs drive the engine and are recorded. The
// recording holds all of it, so it replays like any other, and the editor splices it
// onto the timeline from that tick.
bool Recording_BeginFrom(int client, int from)
{
	if (!g_HaveBaked || from < 1 || from > g_Inputs.Length)
	{
		TAS_Print(client, "recording from a tick needs a loaded timeline and a tick from 1 to its length (%d)", g_Inputs.Length);
		return false;
	}
	char shortName[40], base[TAS_MAX_NAME], name[TAS_MAX_NAME], path[PLATFORM_MAX_PATH];
	strcopy(shortName, sizeof(shortName), g_BakedName);
	FormatEx(base, sizeof(base), "%s_from%d", shortName, from);
	Recording_FreeName(base, name, sizeof(name), path, sizeof(path));
	// g_Start, the expected settings and g_Inputs stay: the ticks before `from` are played from them
	strcopy(g_BakedName, sizeof(g_BakedName), name);
	strcopy(g_BakedPath, sizeof(g_BakedPath), path);
	g_BakedHash[0] = '\0';
	g_HaveBaked = false;
	g_RecordFrom = from;
	g_RecordMaxTicks = from + RoundToCeil(g_cvRecordMaxSeconds.FloatValue / GetTickInterval());
	g_RecordSpeedUntil = from - RoundToCeil(g_cvRecordLeadIn.FloatValue / GetTickInterval());
	g_Recording = true;
	g_BatchTotal = 1;
	g_BatchDone = 0;
	g_BatchIdentical = 0;
	g_BatchDiverged = 0;
	if (g_RecordSpeedUntil > 0)
	{
		Timescale_Raise(client);
	}
	Run_Begin(client, client);
	return true;
}

// After each recorded tick of a recording from a tick: normal speed for the lead-in, a countdown, then go.
void Recording_LeadIn(int client)
{
	if (g_RecordFrom <= 0)
	{
		return;
	}
	if (g_RunTick == g_RecordSpeedUntil)
	{
		Timescale_Restore();
	}
	int left = g_RecordFrom - g_RunTick;
	int quarter = RoundToNearest(0.25 / GetTickInterval());
	if (left > 0 && g_RunTick >= g_RecordSpeedUntil && left % (quarter > 0 ? quarter : 1) == 0)
	{
		PrintCenterText(client, "your turn in %.1f s (tick %d)", float(left) * GetTickInterval(), g_RecordFrom);
	}
	else if (left == 0)
	{
		PrintCenterText(client, "GO: recording you");
		TAS_Chat(client, "your turn: recording live from tick %d (tas_record saves)", g_RecordFrom);
	}
}

void Recording_Finish(const char[] status)
{
	int client = g_RunClient;
	g_Recording = false;
	Timescale_Restore();
	int from = g_RecordFrom;
	g_RecordFrom = 0;
	if (g_RunState != Run_Playing || g_Record.Length <= from)
	{
		g_RunState = Run_Idle;
		g_RunClient = 0;
		g_BatchTotal = 0;
		TAS_Print(client, from > 0 ? "recording cancelled: it stopped before your turn (tick %d)" : "recording cancelled: nothing was recorded (tas_load a timeline to continue)", from);
		if (g_RequestId[0] != '\0')
		{
			Remote_WriteResult(g_RequestId, "cancelled", "-", from > 0 ? "it stopped before your turn" : "nothing was recorded");
			g_RequestId[0] = '\0';
		}
		return;
	}

	// The recorded inputs become the loaded timeline (its replays show the pitch you played). They hold
	// the buttons as played, auto bhop's included, so they replay without it.
	g_Inputs.Clear();
	g_HasViewPitch = false;
	g_AutoBhop = false;
	TickRecord r;
	TickInput ti;
	for (int i = 0; i < g_Record.Length; i++)
	{
		g_Record.GetArray(i, r, sizeof(r));
		ti.yaw = r.yaw;
		ti.pitch = r.pitch;
		ti.forwardmove = r.forwardmove;
		ti.sidemove = r.sidemove;
		ti.buttons = r.buttons;
		g_Inputs.PushArray(ti, sizeof(ti));
	}
	bool saved = Recording_WriteBaked(g_BakedPath, g_BakedHash, sizeof(g_BakedHash));
	if (!saved)
	{
		strcopy(g_BakedHash, sizeof(g_BakedHash), "unsaved");
	}
	g_HaveBaked = saved;

	Run_Finish(status, true);

	if (saved)
	{
		TAS_Print(client, "saved and loaded %s: tas_run replays your inputs and compares the replay with this recording", g_BakedPath);
		if (g_cvBot.BoolValue)
		{
			TAS_Print(client, "  the replay bot plays it; tas_mode edit takes you back to noclip, tas_record records again");
		}
		TAS_Chat(client, "recording saved as %s; tas_run replays it", g_BakedName);
	}
	else
	{
		TAS_Print(client, "FAILED to write %s; the run CSV has the recording", g_BakedPath);
	}
}

void PushVecLine(ArrayList head, const char[] key, const float v[3])
{
	char line[256];
	FormatEx(line, sizeof(line), "@start %s %.6f %.6f %.6f", key, v[0], v[1], v[2]);
	head.PushString(line);
	FormatEx(line, sizeof(line), "@start %s_bits %08X %08X %08X", key, view_as<int>(v[0]), view_as<int>(v[1]), view_as<int>(v[2]));
	head.PushString(line);
}

// Every line of the baked file except comments and @hash, in file order.
void Recording_HeaderLines(ArrayList head)
{
	char line[256], name[64], value[64];
	head.PushString("@format tasurf-baked 1");
	FormatEx(line, sizeof(line), "@name %s", g_BakedName);
	head.PushString(line);
	FormatEx(line, sizeof(line), "@map %s", g_BakedMap);
	head.PushString(line);
	FormatEx(line, sizeof(line), "@ticks %d", g_Inputs.Length);
	head.PushString(line);
	for (int i = 0; i < g_SettingNames.Length; i++)
	{
		g_SettingNames.GetString(i, name, sizeof(name));
		g_SettingValues.GetString(i, value, sizeof(value));
		FormatEx(line, sizeof(line), "@setting %s %s", name, value);
		head.PushString(line);
	}

	PushVecLine(head, "origin", g_Start.origin);
	PushVecLine(head, "angles", g_Start.angles);
	PushVecLine(head, "velocity", g_Start.velocity);
	FormatEx(line, sizeof(line), "@start base_velocity %.6f %.6f %.6f",
		g_Start.baseVelocity[0], g_Start.baseVelocity[1], g_Start.baseVelocity[2]);
	head.PushString(line);
	FormatEx(line, sizeof(line), "@start base_velocity_flag %d", g_Start.baseVelocityFlag ? 1 : 0);
	head.PushString(line);
	if (g_Start.onGround == Ground_Yes)
	{
		head.PushString("@start on_ground yes");
	}
	else if (g_Start.onGround == Ground_No)
	{
		head.PushString("@start on_ground no");
	}
	else
	{
		head.PushString("@start on_ground auto");
	}
	FormatEx(line, sizeof(line), "@start ground_entity %d", g_Start.groundEntity);
	head.PushString(line);
	head.PushString("@start ducked 0");
	FormatEx(line, sizeof(line), "@start gravity %.6f", g_Start.gravity);
	head.PushString(line);
	FormatEx(line, sizeof(line), "@start lagged_movement %.6f", g_Start.laggedMovement);
	head.PushString(line);
	FormatEx(line, sizeof(line), "@start velocity_modifier %.6f", g_Start.velocityModifier);
	head.PushString(line);
	FormatEx(line, sizeof(line), "@start stamina %.6f", g_Start.stamina);
	head.PushString(line);
	if (g_Start.hasViewOffsetZ)
	{
		FormatEx(line, sizeof(line), "@start view_offset_z %.6f", g_Start.viewOffsetZ);
		head.PushString(line);
	}
	if (g_Start.hasHealth)
	{
		FormatEx(line, sizeof(line), "@start health %d", g_Start.health);
		head.PushString(line);
	}
	if (g_Start.hasMaxspeed)
	{
		FormatEx(line, sizeof(line), "@start maxspeed %.6f", g_Start.maxspeed);
		head.PushString(line);
	}
	head.PushString("tick,yaw,pitch,forwardmove,sidemove,buttons,yaw_bits,pitch_bits,forwardmove_bits,sidemove_bits");
}

void Recording_TickLine(int i, char[] line, int maxlen)
{
	TickInput ti;
	g_Inputs.GetArray(i, ti, sizeof(ti));
	FormatEx(line, maxlen, "%d,%.6f,%.6f,%.6f,%.6f,%d,%08X,%08X,%08X,%08X",
		i, ti.yaw, ti.pitch, ti.forwardmove, ti.sidemove, ti.buttons,
		view_as<int>(ti.yaw), view_as<int>(ti.pitch), view_as<int>(ti.forwardmove), view_as<int>(ti.sidemove));
}

// Baked file from the recorded inputs. @hash is FNV-1a 32 over the same lines
// the baker's SHA-256 covers (see fnv1a32_hash in tasurf/bake.py).
bool Recording_WriteBaked(const char[] path, char[] hash, int hashlen)
{
	ArrayList head = new ArrayList(ByteCountToCells(256));
	Recording_HeaderLines(head);
	char line[256];
	int h = FNV_INIT;
	for (int i = 0; i < head.Length; i++)
	{
		head.GetString(i, line, sizeof(line));
		h = Fnv1aLine(h, line);
	}
	for (int i = 0; i < g_Inputs.Length; i++)
	{
		Recording_TickLine(i, line, sizeof(line));
		h = Fnv1aLine(h, line);
	}
	FormatEx(hash, hashlen, "fnv1a32:%08x", h);

	File f = OpenFile(path, "w");
	if (f == null)
	{
		delete head;
		return false;
	}
	f.WriteLine("# TASurf recording, written by tas_record: live inputs with their exact float bits.");
	f.WriteLine("# Replay it with tas_load / tas_run; do not edit by hand.");
	for (int i = 0; i < head.Length; i++)
	{
		head.GetString(i, line, sizeof(line));
		f.WriteLine("%s", line);
		if (i == 1)  // right after @name
		{
			f.WriteLine("@hash %s", hash);
		}
	}
	for (int i = 0; i < g_Inputs.Length; i++)
	{
		Recording_TickLine(i, line, sizeof(line));
		f.WriteLine("%s", line);
	}
	delete f;
	delete head;
	return true;
}
