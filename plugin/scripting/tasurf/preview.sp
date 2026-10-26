// Fast engine previews (tas_preview, or "preview" requests from the editor).
//
// A preview is an ordinary run (same reset, same inputs, same engine), played with
// host_timescale raised so it finishes in a fraction of real time. Ticks keep their normal
// length; only more of them run per real second. It writes runs/<name>.preview.csv
// (overwritten each time) instead of a numbered run, and doesn't count as a "last run" for
// comparisons.
//
// On the replay bot (the default), you're held still while it plays: with time sped up,
// noclip would fly you 20 times as far. With tas_bot 0 it plays on you, your view stays
// where you were (tas_preview_lock_view), and afterwards you're put back.
//
// A newer editor request replaces a preview that's still playing (Preview_Supersede): the
// stale one is dropped and the new one starts at once, still sped up.

bool g_PreviewSpedUp;
bool g_PreviewSetCheats;
char g_PreviewOldTimescale[32];
int g_PreviewRunner;
int g_PreviewViewer;
float g_PreviewReturnOrigin[3];
float g_PreviewReturnAngles[3];
int g_PreviewCamera = INVALID_ENT_REFERENCE;

// Keep the player's view where they were standing while their body plays the
// preview elsewhere. Rendering only: the view entity doesn't take part in
// movement. If the camera can't be made, the preview still runs, just visibly.
void Preview_LockView(int client)
{
	if (!g_cvPreviewLockView.BoolValue || g_PreviewCamera != INVALID_ENT_REFERENCE)
	{
		return;
	}
	int cam = CreateEntityByName("point_viewcontrol");
	if (cam == -1)
	{
		return;
	}
	DispatchKeyValue(cam, "spawnflags", "0");
	DispatchSpawn(cam);
	float eye[3];
	GetClientEyePosition(client, eye);
	TeleportEntity(cam, eye, g_PreviewReturnAngles, NULL_VECTOR);
	SetClientViewEntity(client, cam);
	g_PreviewCamera = EntIndexToEntRef(cam);
}

void Preview_UnlockView(int client)
{
	if (g_PreviewCamera == INVALID_ENT_REFERENCE)
	{
		return;
	}
	if (client > 0 && IsClientInGame(client))
	{
		SetClientViewEntity(client, client);
	}
	int cam = EntRefToEntIndex(g_PreviewCamera);
	if (cam > 0 && IsValidEntity(cam))
	{
		AcceptEntityInput(cam, "Kill");
	}
	g_PreviewCamera = INVALID_ENT_REFERENCE;
}

// Safe to call again for a preview that replaces another: what's saved stays saved.
void Preview_Enter(int runner, int viewer)
{
	bool first = !g_Preview;
	g_Preview = true;
	g_PreviewRunner = runner;
	g_PreviewViewer = viewer;
	if (runner == viewer)
	{
		if (first)
		{
			GetClientAbsOrigin(runner, g_PreviewReturnOrigin);
			GetClientEyeAngles(runner, g_PreviewReturnAngles);
			g_PreviewReturnAngles[2] = 0.0;
		}
		Preview_LockView(runner);
	}
	else
	{
		Mode_Freeze(viewer);
	}
	Timescale_Raise(viewer);
}

// Time sped up (host_timescale = tas_preview_timescale) for previews, and for the replayed
// part of a recording from a tick. Ticks keep their length; only more run per real second.
void Timescale_Raise(int viewer)
{
	if (g_PreviewSpedUp)
	{
		return;
	}
	ConVar cheats = FindConVar("sv_cheats");
	ConVar timescale = FindConVar("host_timescale");
	g_PreviewSetCheats = false;
	g_PreviewOldTimescale[0] = '\0';
	if (timescale == null)
	{
		TAS_Print(viewer, "host_timescale isn't available; this plays at normal speed");
		return;
	}
	timescale.GetString(g_PreviewOldTimescale, sizeof(g_PreviewOldTimescale));
	if (cheats != null && !cheats.BoolValue)
	{
		cheats.SetBool(true);  // host_timescale is a cheat convar
		g_PreviewSetCheats = true;
	}
	timescale.SetFloat(g_cvPreviewTimescale.FloatValue);
	g_PreviewSpedUp = true;
}

void Timescale_Restore()
{
	ConVar timescale = FindConVar("host_timescale");
	if (g_PreviewSpedUp && timescale != null && g_PreviewOldTimescale[0] != '\0')
	{
		timescale.SetString(g_PreviewOldTimescale);
	}
	if (g_PreviewSetCheats)
	{
		ConVar cheats = FindConVar("sv_cheats");
		if (cheats != null)
		{
			cheats.SetBool(false);
		}
		g_PreviewSetCheats = false;
	}
	g_PreviewSpedUp = false;
}

void Preview_Restore()
{
	Timescale_Restore();
	int runner = g_PreviewRunner, viewer = g_PreviewViewer;
	if (runner == viewer && runner > 0 && IsClientInGame(runner) && IsPlayerAlive(runner))
	{
		float still[3];
		TeleportEntity(runner, g_PreviewReturnOrigin, g_PreviewReturnAngles, still);
		Preview_UnlockView(runner);
	}
	else
	{
		Preview_UnlockView(viewer);
		Mode_Unfreeze();
	}
	if (runner > 0 && runner == Bot_Get() && IsPlayerAlive(runner))
	{
		Bot_Idle(runner);
	}
	g_Preview = false;
	g_PreviewRunner = 0;
	g_PreviewViewer = 0;
}

// A newer request arrived while an editor preview is playing: drop this one quietly and
// keep the speed-up (and your frozen view) for what comes next. The caller starts it, or
// calls Preview_Restore if it doesn't.
void Preview_Supersede()
{
	Remote_WriteResult(g_RequestId, "superseded", "-", "a newer request replaced it");
	g_RequestId[0] = '\0';
	g_RunState = Run_Idle;
	g_RunClient = 0;
	g_BatchTotal = 0;
	g_Record.Clear();
}

void Preview_Finish(const char[] status, bool complete)
{
	int viewer = g_RunViewer;
	bool inGame = viewer > 0 && IsClientInGame(viewer);
	int reportTo = inGame ? viewer : 0;
	g_RunState = Run_Idle;
	g_RunClient = 0;
	g_BatchTotal = 0;
	Preview_Restore();

	char path[PLATFORM_MAX_PATH], csvName[PLATFORM_MAX_PATH];
	BuildPath(Path_SM, path, sizeof(path), "%s/runs/%s.preview.csv", TAS_DATA_DIR, g_BakedName);
	FormatEx(csvName, sizeof(csvName), "%s.preview.csv", g_BakedName);
	bool wrote = Record_WriteCsv(path, status, g_RunSettleTicks, g_RunMaxspeed, false);
	if (g_RequestId[0] != '\0')
	{
		Remote_WriteResult(g_RequestId, complete ? "preview" : status, wrote ? csvName : "-", "fast engine preview");
		g_RequestId[0] = '\0';
	}
	TAS_Print(reportTo, "preview of %s: %s, %d/%d ticks, max horizontal speed %.2f -> %s",
		g_BakedName, status, g_Record.Length, g_Inputs.Length, Record_MaxSpeed(g_Record), wrote ? csvName : "FAILED to write");
	if (g_Record.Length > 0)
	{
		TickRecord end;
		g_Record.GetArray(g_Record.Length - 1, end, sizeof(end));
		if (g_LastRecord.Length > 0 && StrEqual(g_LastName, g_BakedName) && StrEqual(g_LastMap, g_BakedMap))
		{
			TickRecord lastEnd;
			g_LastRecord.GetArray(g_LastRecord.Length - 1, lastEnd, sizeof(lastEnd));
			TAS_Chat(viewer, "preview updated: ends at %.1f u/s (%+.1f vs last run)", end.hspeed, end.hspeed - lastEnd.hspeed);
		}
		else
		{
			TAS_Chat(viewer, "preview updated: ends at %.1f u/s", end.hspeed);
		}
	}

	if (inGame && g_cvAutoDraw.BoolValue && g_Record.Length > 0)
	{
		// the preview in speed colours, the last real run of this timeline in grey
		Draw_Clear();
		if (g_LastRecord.Length > 0 && StrEqual(g_LastName, g_BakedName) && StrEqual(g_LastMap, g_BakedMap))
		{
			ArrayList last = Draw_RunPoints(g_LastRecord, g_LastStartOrigin, g_LastStartSpeed);
			Draw_AddPath(viewer, last, -1, true);
			delete last;
		}
		float startSpeed = SquareRoot(g_Start.velocity[0] * g_Start.velocity[0] + g_Start.velocity[1] * g_Start.velocity[1]);
		ArrayList points = Draw_RunPoints(g_Record, g_Start.origin, startSpeed);
		Draw_AddPath(viewer, points, -1, false, Draw_CursorFor(g_BakedName));
		delete points;
		Draw_Start(viewer);
	}
	g_Record.Clear();
}

public void OnPluginEnd()
{
	if (g_Preview || g_PreviewSpedUp)
	{
		Preview_Restore();  // never leave the game sped up, or you frozen
	}
	Mode_Unfreeze();
	NoKick_Restore();  // the server's own mp_autokick again
}
