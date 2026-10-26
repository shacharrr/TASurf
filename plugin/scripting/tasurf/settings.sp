// Server settings. Values always come from the running game; the timeline
// only declares what it expects (@setting lines), and every run CSV records
// what was actually in effect.

// Movement-relevant convars recorded in every run CSV header (whether or not
// the timeline mentions them). Convars this game doesn't have are recorded as
// <missing>.
char g_RecordedCvars[][] =
{
	"sv_gravity", "sv_airaccelerate", "sv_accelerate", "sv_friction", "sv_stopspeed",
	"sv_maxspeed", "sv_maxvelocity", "sv_stepsize", "sv_bounce", "sv_backspeed",
	"sv_wateraccelerate", "sv_waterfriction", "sv_noclipaccelerate", "sv_noclipspeed",
	"sv_specaccelerate", "sv_rollangle", "sv_rollspeed", "sv_optimizedmovement",
	"sv_enablebunnyhopping", "sv_maxusrcmdprocessticks", "sv_runcmds",
	"host_timescale", "phys_timescale", "sv_cheats", "tas_fall_damage"
};

void Settings_CvarString(const char[] name, char[] value, int maxlen)
{
	ConVar cv = FindConVar(name);
	if (cv == null)
	{
		strcopy(value, maxlen, "<missing>");
		return;
	}
	cv.GetString(value, maxlen);
}

// Does the running game match an expected setting? actual gets the game's value.
bool Settings_Compare(const char[] name, const char[] expected, char[] actual, int alen)
{
	float e;
	bool numeric = ParseFloatStrict(expected, e);
	float interval = GetTickInterval();

	if (StrEqual(name, "tickrate"))
	{
		FormatEx(actual, alen, "%.4f", 1.0 / interval);
		return numeric && FloatAbs(1.0 / interval - e) <= 0.01;
	}
	if (StrEqual(name, "tick_interval"))
	{
		FormatEx(actual, alen, "%.6f", interval);
		return numeric && FloatAbs(interval - e) <= 0.000001;
	}

	ConVar cv = FindConVar(name);
	if (cv == null)
	{
		strcopy(actual, alen, "<missing>");
		return false;
	}
	cv.GetString(actual, alen);
	if (numeric)
	{
		float tol = 0.0001 * (FloatAbs(e) > 1.0 ? FloatAbs(e) : 1.0);
		return FloatAbs(cv.FloatValue - e) <= tol;
	}
	return StrEqual(actual, expected);
}

// Print the expected-vs-actual table. Returns the number of mismatches.
int Settings_Report(int client, bool printMatches)
{
	char name[64], expected[64], actual[64];
	int mismatches = 0;
	for (int i = 0; i < g_SettingNames.Length; i++)
	{
		g_SettingNames.GetString(i, name, sizeof(name));
		g_SettingValues.GetString(i, expected, sizeof(expected));
		bool ok = Settings_Compare(name, expected, actual, sizeof(actual));
		if (!ok)
		{
			mismatches++;
			TAS_Print(client, "  MISMATCH %s: timeline expects %s, game has %s", name, expected, actual);
		}
		else if (printMatches)
		{
			TAS_Print(client, "  ok       %s = %s", name, actual);
		}
	}
	return mismatches;
}

void Settings_Apply(int client)
{
	char name[64], expected[64], actual[64];
	for (int i = 0; i < g_SettingNames.Length; i++)
	{
		g_SettingNames.GetString(i, name, sizeof(name));
		g_SettingValues.GetString(i, expected, sizeof(expected));
		if (Settings_Compare(name, expected, actual, sizeof(actual)))
		{
			continue;
		}
		if (StrEqual(name, "tickrate") || StrEqual(name, "tick_interval"))
		{
			TAS_Print(client, "  can't change %s at runtime (is %s): restart the game with -tickrate", name, actual);
			continue;
		}
		ConVar cv = FindConVar(name);
		if (cv == null)
		{
			TAS_Print(client, "  %s does not exist in this game", name);
			continue;
		}
		cv.SetString(expected, true);
		cv.GetString(actual, sizeof(actual));
		TAS_Print(client, "  set %s = %s", name, actual);
		TAS_Log("tas_applysettings: %s = %s", name, actual);
	}
}

// Header lines describing the settings in effect, for the run CSV.
void Settings_Snapshot(ArrayList lines)
{
	char line[256], value[128], name[64];
	float interval = GetTickInterval();
	FormatEx(line, sizeof(line), "# tick_interval %.6f", interval);
	lines.PushString(line);
	FormatEx(line, sizeof(line), "# tick_interval_bits %08X", view_as<int>(interval));
	lines.PushString(line);
	FormatEx(line, sizeof(line), "# tickrate %.4f", 1.0 / interval);
	lines.PushString(line);

	for (int i = 0; i < sizeof(g_RecordedCvars); i++)
	{
		Settings_CvarString(g_RecordedCvars[i], value, sizeof(value));
		FormatEx(line, sizeof(line), "# cvar.%s %s", g_RecordedCvars[i], value);
		lines.PushString(line);
	}
	// expected settings the fixed list doesn't cover
	for (int i = 0; i < g_SettingNames.Length; i++)
	{
		g_SettingNames.GetString(i, name, sizeof(name));
		if (StrEqual(name, "tickrate") || StrEqual(name, "tick_interval") || Settings_IsRecorded(name))
		{
			continue;
		}
		Settings_CvarString(name, value, sizeof(value));
		FormatEx(line, sizeof(line), "# cvar.%s %s", name, value);
		lines.PushString(line);
	}
}

bool Settings_IsRecorded(const char[] name)
{
	for (int i = 0; i < sizeof(g_RecordedCvars); i++)
	{
		if (StrEqual(g_RecordedCvars[i], name))
		{
			return true;
		}
	}
	return false;
}

bool Settings_SnapshotsEqual(ArrayList a, ArrayList b)
{
	if (a.Length != b.Length)
	{
		return false;
	}
	char la[256], lb[256];
	for (int i = 0; i < a.Length; i++)
	{
		a.GetString(i, la, sizeof(la));
		b.GetString(i, lb, sizeof(lb));
		if (!StrEqual(la, lb))
		{
			return false;
		}
	}
	return true;
}
