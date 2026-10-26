// Per-tick recording, run CSV output and run-to-run comparison.
//
// The CSV holds only things that are a function of (baked file, settings,
// start state, settle ticks), so two runs of the same thing produce
// byte-identical CSVs. Wall-clock time, command numbers and other run
// metadata go to the .meta.txt file next to it.

#define CSV_COLUMNS "tick,yaw,pitch,forwardmove,sidemove,buttons,x,y,z,vx,vy,vz,hspeed,on_ground,ground_entity,ducked,base_vx,base_vy,base_vz,x_bits,y_bits,z_bits,vx_bits,vy_bits,vz_bits,yaw_bits,pitch_bits"

ArrayList g_Record;          // TickRecord, the run in progress
ArrayList g_LastRecord;      // the last finished run (for comparison and drawing)
bool g_LastComplete;
char g_LastHash[TAS_HASH_LEN];
char g_LastMap[PLATFORM_MAX_PATH];
char g_LastCsvName[PLATFORM_MAX_PATH];
int g_LastSettle;
float g_LastStartOrigin[3];
float g_LastStartSpeed;
char g_LastName[TAS_MAX_NAME];

ArrayList g_GhostRecord;     // the run before the last one (drawn grey for comparison)
char g_GhostName[TAS_MAX_NAME];
char g_GhostMap[PLATFORM_MAX_PATH];
float g_GhostStartOrigin[3];
float g_GhostStartSpeed;

ArrayList g_RunSettings;     // settings snapshot taken at tick 0

void Record_Init()
{
	g_Record = new ArrayList(sizeof(TickRecord));
	g_LastRecord = new ArrayList(sizeof(TickRecord));
	g_GhostRecord = new ArrayList(sizeof(TickRecord));
	g_RunSettings = new ArrayList(ByteCountToCells(256));
}

void Record_Capture(int client, int buttons, const float vel[3], const float angles[3])
{
	TickRecord r;
	r.yaw = angles[1];
	r.pitch = angles[0];
	r.forwardmove = vel[0];
	r.sidemove = vel[1];
	r.buttons = buttons;
	GetClientAbsOrigin(client, r.origin);
	GetEntPropVector(client, Prop_Data, "m_vecAbsVelocity", r.velocity);
	GetEntPropVector(client, Prop_Data, "m_vecBaseVelocity", r.baseVelocity);
	r.hspeed = SquareRoot(r.velocity[0] * r.velocity[0] + r.velocity[1] * r.velocity[1]);
	int flags = GetEntityFlags(client);
	r.onGround = (flags & FL_ONGROUND) ? 1 : 0;
	r.groundEntity = GetEntPropEnt(client, Prop_Data, "m_hGroundEntity");
	r.ducked = (flags & FL_DUCKING) ? 1 : 0;
	g_Record.PushArray(r, sizeof(r));
}

float Record_MaxSpeed(ArrayList records)
{
	float best = 0.0;
	TickRecord r;
	for (int i = 0; i < records.Length; i++)
	{
		records.GetArray(i, r, sizeof(r));
		if (r.hspeed > best)
		{
			best = r.hspeed;
		}
	}
	return best;
}

// Next free data/tasurf/runs/<name>_NNN.csv.
int Record_NextRunPath(char[] path, int maxlen)
{
	for (int i = 1; i < 100000; i++)
	{
		BuildPath(Path_SM, path, maxlen, "%s/runs/%s_%03d.csv", TAS_DATA_DIR, g_BakedName, i);
		if (!FileExists(path))
		{
			return i;
		}
	}
	return -1;
}

bool Record_WriteCsv(const char[] path, const char[] status, int settleTicks, float maxspeed, bool settingsChanged)
{
	File f = OpenFile(path, "w");
	if (f == null)
	{
		return false;
	}
	f.WriteLine("# tasurf-run 1");
	f.WriteLine("# plugin_version %s", TAS_VERSION);
	f.WriteLine("# name %s", g_BakedName);
	f.WriteLine("# baked_hash %s", g_BakedHash);
	f.WriteLine("# map %s", g_BakedMap);
	f.WriteLine("# ticks_planned %d", g_Inputs.Length);
	f.WriteLine("# ticks_recorded %d", g_Record.Length);
	f.WriteLine("# status %s", status);
	f.WriteLine("# settle_ticks %d", settleTicks);
	char line[256];
	for (int i = 0; i < g_RunSettings.Length; i++)
	{
		g_RunSettings.GetString(i, line, sizeof(line));
		f.WriteLine("%s", line);
	}
	if (settingsChanged)
	{
		f.WriteLine("# warning settings_changed_during_run (the values above are from tick 0)");
	}
	f.WriteLine("# player.maxspeed %.6f", maxspeed);
	Start_WriteHeader(f, g_Start);
	f.WriteLine(CSV_COLUMNS);

	TickRecord r;
	char a[320], b[320];
	for (int i = 0; i < g_Record.Length; i++)
	{
		g_Record.GetArray(i, r, sizeof(r));
		FormatEx(a, sizeof(a), "%d,%.6f,%.6f,%.6f,%.6f,%d,%.6f,%.6f,%.6f,%.6f,%.6f,%.6f,%.6f",
			i, r.yaw, r.pitch, r.forwardmove, r.sidemove, r.buttons,
			r.origin[0], r.origin[1], r.origin[2], r.velocity[0], r.velocity[1], r.velocity[2], r.hspeed);
		FormatEx(b, sizeof(b), "%d,%d,%d,%.6f,%.6f,%.6f,%08X,%08X,%08X,%08X,%08X,%08X,%08X,%08X",
			r.onGround, r.groundEntity, r.ducked, r.baseVelocity[0], r.baseVelocity[1], r.baseVelocity[2],
			view_as<int>(r.origin[0]), view_as<int>(r.origin[1]), view_as<int>(r.origin[2]),
			view_as<int>(r.velocity[0]), view_as<int>(r.velocity[1]), view_as<int>(r.velocity[2]),
			view_as<int>(r.yaw), view_as<int>(r.pitch));
		f.WriteLine("%s,%s", a, b);
	}
	delete f;
	return true;
}

void Record_WriteMeta(const char[] csvPath, const char[] status, int startCmd, int startServerTick,
	int skipped, int settleTicks, const char[] comparison)
{
	char path[PLATFORM_MAX_PATH];
	strcopy(path, sizeof(path), csvPath);
	ReplaceString(path, sizeof(path), ".csv", ".meta.txt", false);
	File f = OpenFile(path, "w");
	if (f == null)
	{
		return;
	}
	char date[64], csvName[PLATFORM_MAX_PATH];
	FormatTime(date, sizeof(date), "%Y-%m-%d %H:%M:%S");
	BaseName(csvPath, csvName, sizeof(csvName));
	f.WriteLine("# TASurf run metadata. Not part of the determinism check.");
	f.WriteLine("run_csv %s", csvName);
	f.WriteLine("date %s", date);
	f.WriteLine("status %s", status);
	f.WriteLine("baked_file %s", g_BakedPath);
	f.WriteLine("baked_hash %s", g_BakedHash);
	f.WriteLine("start_cmdnum %d", startCmd);
	f.WriteLine("start_server_tick %d", startServerTick);
	f.WriteLine("skipped_commands %d", skipped);
	f.WriteLine("settle_ticks %d", settleTicks);
	f.WriteLine("comparison %s", comparison);
	f.WriteLine("plugin_version %s", TAS_VERSION);
	f.WriteLine("sourcemod_version %s", SOURCEMOD_VERSION);
	delete f;
}

// Compare two records bit for bit. On the first difference, describe it.
bool Record_TickEqual(TickRecord a, TickRecord b, char[] what, int maxlen)
{
	for (int k = 0; k < 3; k++)
	{
		if (!SameBits(a.origin[k], b.origin[k]))
		{
			FormatEx(what, maxlen, "origin[%d] %.6f (%08X) vs %.6f (%08X)", k,
				a.origin[k], view_as<int>(a.origin[k]), b.origin[k], view_as<int>(b.origin[k]));
			return false;
		}
		if (!SameBits(a.velocity[k], b.velocity[k]))
		{
			FormatEx(what, maxlen, "velocity[%d] %.6f (%08X) vs %.6f (%08X)", k,
				a.velocity[k], view_as<int>(a.velocity[k]), b.velocity[k], view_as<int>(b.velocity[k]));
			return false;
		}
		if (!SameBits(a.baseVelocity[k], b.baseVelocity[k]))
		{
			FormatEx(what, maxlen, "base_velocity[%d] %.6f vs %.6f", k, a.baseVelocity[k], b.baseVelocity[k]);
			return false;
		}
	}
	if (a.onGround != b.onGround || a.groundEntity != b.groundEntity || a.ducked != b.ducked)
	{
		FormatEx(what, maxlen, "ground/duck state %d/%d/%d vs %d/%d/%d",
			a.onGround, a.groundEntity, a.ducked, b.onGround, b.groundEntity, b.ducked);
		return false;
	}
	if (!SameBits(a.yaw, b.yaw) || !SameBits(a.pitch, b.pitch) || !SameBits(a.forwardmove, b.forwardmove)
		|| !SameBits(a.sidemove, b.sidemove) || a.buttons != b.buttons)
	{
		strcopy(what, maxlen, "processed inputs differ (another plugin changing inputs?)");
		return false;
	}
	return true;
}

bool Record_Compare(ArrayList a, ArrayList b, char[] msg, int maxlen)
{
	if (a.Length != b.Length)
	{
		FormatEx(msg, maxlen, "different lengths (%d vs %d ticks)", a.Length, b.Length);
		return false;
	}
	TickRecord ra, rb;
	char what[192];
	for (int i = 0; i < a.Length; i++)
	{
		a.GetArray(i, ra, sizeof(ra));
		b.GetArray(i, rb, sizeof(rb));
		if (!Record_TickEqual(ra, rb, what, sizeof(what)))
		{
			FormatEx(msg, maxlen, "first divergence at tick %d: %s", i, what);
			return false;
		}
	}
	FormatEx(msg, maxlen, "bit-identical (%d ticks)", a.Length);
	return true;
}
