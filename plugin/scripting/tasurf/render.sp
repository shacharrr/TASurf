// Rendering a run to video with SVR (Source Video Render), for "render" requests from the desktop app.
//
// The app starts the game through SVR's launcher, which hooks startmovie and endmovie. A render is an ordinary
// run at normal speed on the replay bot, seen through its eyes: you're moved to the spectators and watch it in
// first person (the engine's own view: smooth, it shows the replay pitch, and SVR's velocity overlay follows the
// player you watch). Beams (the path drawings) and the weapon are hidden, the movie starts on tick 0 and ends
// tail seconds after the last tick, then you're put back on your team where you were (edit mode's respawn does
// that) and the view settings SVR changed for the movie are put back.
//
// startmovie reaches your game through ClientCommand (CS:S lets the server run it on its own host). Without SVR the
// engine's own startmovie would write image files instead, so the app only sends render requests to a game with
// SVR loaded.
//
// The request (remote.sp) has, besides id and load:
//   render <file>      the movie's file name in SVR's data/movies (name.mp4, name.mov or name.mkv)
//   tail <seconds>     video after the last tick (default 0.5)
//   viewmodel 0|1      show the weapon (default 0)

// The render's state and what the request asked for are declared in types.sp (remote.sp reads requests first).

bool Render_Active()
{
	return g_RenderPhase != Render_None;
}

void Render_ResetRequest()
{
	g_ReqRender = false;
	g_ReqRenderFile[0] = '\0';
	g_ReqRenderTail = 0.5;
	g_ReqRenderViewmodel = false;
}

// A movie file name SVR takes: letters, digits, _ - . and one of its containers.
bool Render_ValidFile(const char[] file)
{
	int dot = FindCharInString(file, '.', true);
	if (dot <= 0 || !IsValidName(file))
	{
		return false;
	}
	return StrEqual(file[dot], ".mp4", false) || StrEqual(file[dot], ".mov", false) || StrEqual(file[dot], ".mkv", false);
}

// Instead of Run_Begin for a render request: you to the spectators, then (Timer_RenderGo) the run.
void Render_Begin(int runner, int host)
{
	strcopy(g_RenderFile, sizeof(g_RenderFile), g_ReqRenderFile);
	g_RenderTail = g_ReqRenderTail < 0.0 ? 0.0 : (g_ReqRenderTail > 10.0 ? 10.0 : g_ReqRenderTail);
	g_RenderViewmodel = g_ReqRenderViewmodel;
	g_RenderHost = GetClientUserId(host);
	g_RenderRunner = GetClientUserId(runner);
	g_RenderTeam = GetClientTeam(host);
	g_RenderMovie = false;
	ConVar cheats = FindConVar("sv_cheats");
	g_RenderSetCheats = cheats != null && !cheats.BoolValue;
	if (g_RenderSetCheats)
	{
		cheats.SetInt(1);  // SVR's movie config sets it too; hiding beams and the weapon needs it now
	}
	Draw_Clear();
	ClientCommand(host, "r_drawbeams 0; r_drawviewmodel %d", g_RenderViewmodel ? 1 : 0);
	ChangeClientTeam(host, CS_TEAM_SPECTATOR);
	g_RenderPhase = Render_PreRoll;
	TAS_Print(host, "render: %s to %s (you watch the replay bot; you're put back afterwards)", g_BakedName, g_RenderFile);
	CreateTimer(0.6, Timer_RenderGo, _, TIMER_FLAG_NO_MAPCHANGE);
}

public Action Timer_RenderGo(Handle timer)
{
	if (g_RenderPhase != Render_PreRoll)
	{
		return Plugin_Stop;
	}
	int host = GetClientOfUserId(g_RenderHost), runner = GetClientOfUserId(g_RenderRunner);
	if (host <= 0 || runner <= 0 || !IsPlayerAlive(runner))
	{
		if (g_RequestId[0] != '\0')
		{
			Remote_WriteResult(g_RequestId, "error", "-", host <= 0 ? "you left the game" : "the replay bot isn't there");
			g_RequestId[0] = '\0';
		}
		Render_Restore();
		return Plugin_Stop;
	}
	Render_Watch(host, runner);
	g_RenderPhase = Render_Playing;
	Run_Begin(runner, host);
	return Plugin_Stop;
}

// First person on the runner (again every tick: a click in the game would move the view to another player).
void Render_Watch(int host, int runner)
{
	if (GetEntPropEnt(host, Prop_Send, "m_hObserverTarget") != runner)
	{
		SetEntPropEnt(host, Prop_Send, "m_hObserverTarget", runner);
	}
	if (GetEntProp(host, Prop_Send, "m_iObserverMode") != 4)
	{
		SetEntProp(host, Prop_Send, "m_iObserverMode", 4);  // OBS_MODE_IN_EYE
	}
}

// Every tick of the render's run (OnPlayerRunCmdPost): the view on the runner; at tick 0, the movie starts.
void Render_Tick(int runner, int tick)
{
	if (g_RenderPhase != Render_Playing)
	{
		return;
	}
	int host = GetClientOfUserId(g_RenderHost);
	if (host <= 0)
	{
		return;
	}
	Render_Watch(host, runner);
	if (tick == 0 && !g_RenderMovie)
	{
		// timeout: SVR stops by itself should endmovie never come (the game hung, the plugin was reloaded)
		int timeout = RoundToCeil(g_Inputs.Length * GetTickInterval() + g_RenderTail + 5.0);
		ClientCommand(host, "startmovie %s profile=tasurf timeout=%d", g_RenderFile, timeout);
		g_RenderMovie = true;
	}
}

// The render's run ended (Run_Finish): the tail, then endmovie.
void Render_RunEnded()
{
	if (g_RenderPhase != Render_Playing)
	{
		return;
	}
	g_RenderPhase = Render_Tail;
	CreateTimer(g_RenderTail > 0.05 ? g_RenderTail : 0.05, Timer_RenderStop, _, TIMER_FLAG_NO_MAPCHANGE);
}

public Action Timer_RenderStop(Handle timer)
{
	int host = GetClientOfUserId(g_RenderHost);
	if (host > 0 && g_RenderMovie)
	{
		ClientCommand(host, "endmovie");
	}
	CreateTimer(1.0, Timer_RenderRestore, _, TIMER_FLAG_NO_MAPCHANGE);
	return Plugin_Stop;
}

public Action Timer_RenderRestore(Handle timer)
{
	Render_Restore();
	return Plugin_Stop;
}

// You back on your team (edit mode's respawn puts you where you were), the view as it was before the movie:
// SVR's movie config turns off the HUD, prediction and interpolation and makes the game sleep when it's in the
// background, and doesn't turn them back.
void Render_Restore()
{
	int host = GetClientOfUserId(g_RenderHost);
	if (host > 0 && IsClientInGame(host))
	{
		ClientCommand(host, "r_drawbeams 1; r_drawviewmodel 1; cl_drawhud 1; cl_predict 1; cl_interp 0.1; cl_interp_ratio 2; engine_no_focus_sleep 0");
		if (GetClientTeam(host) != g_RenderTeam && g_RenderTeam >= CS_TEAM_T)
		{
			ChangeClientTeam(host, g_RenderTeam);
			CS_RespawnPlayer(host);
		}
	}
	if (g_RenderSetCheats)
	{
		// after your game has run the commands above: some need cheats on (turning them off resets those anyway)
		CreateTimer(1.0, Timer_RenderCheatsOff, _, TIMER_FLAG_NO_MAPCHANGE);
		g_RenderSetCheats = false;
	}
	g_RenderPhase = Render_None;
	g_RenderMovie = false;
	Remote_ResetDraw();  // the editor's drawing comes back
}

public Action Timer_RenderCheatsOff(Handle timer)
{
	if (!Render_Active() && g_RunState == Run_Idle && !g_Preview)  // a preview since sets and puts back cheats itself
	{
		ConVar cheats = FindConVar("sv_cheats");
		if (cheats != null)
		{
			cheats.SetInt(0);
		}
	}
	return Plugin_Stop;
}

// The map is ending: nothing of the render survives it.
void Render_OnMapEnd()
{
	if (Render_Active())
	{
		g_RenderPhase = Render_None;
		g_RenderMovie = false;
		if (g_RenderSetCheats)
		{
			ConVar cheats = FindConVar("sv_cheats");
			if (cheats != null)
			{
				cheats.SetInt(0);
			}
			g_RenderSetCheats = false;
		}
	}
}
