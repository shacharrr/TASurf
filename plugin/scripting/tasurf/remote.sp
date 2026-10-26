// Run requests from the browser editor (python -m tasurf editor).
//
// The editor writes data/tasurf/requests/request.txt (atomically, by rename):
//   id <request id>
//   load <baked name>
//   run            (or "preview": a sped-up run, see preview.sp; or "render <file>": a run recorded to
//                  video with SVR, see render.sp)
// Every 0.25 s, while no run is playing (or while an editor preview is playing,
// which the new request replaces), the plugin takes that file (renaming it to
// request.taken first, so a newer one written meanwhile isn't lost), loads the
// baked file and starts the run on the replay bot (or, with tas_bot 0, on the
// first human player). When the run ends it writes requests/<id>.result:
//   status <run status>   ("superseded" when a newer request replaced it)
//   csv <run csv file name>
//   detail <comparison or error>
//
// When you open a timeline on another map, the editor writes requests/changemap.txt the same way:
//   id <request id>
//   map <map name>
// and the game changes to it once no run is playing (Remote_CheckChangeMap). Run requests wait meanwhile, so one
// for the new timeline plays on its own map.


void Remote_Init()
{
	CreateTimer(0.25, Timer_Remote, _, TIMER_REPEAT);
}

void Remote_WriteResult(const char[] id, const char[] status, const char[] csv, const char[] detail)
{
	char path[PLATFORM_MAX_PATH];
	BuildPath(Path_SM, path, sizeof(path), "%s/requests/%s.result", TAS_DATA_DIR, id);
	File f = OpenFile(path, "w");
	if (f == null)
	{
		return;
	}
	f.WriteLine("status %s", status);
	f.WriteLine("csv %s", csv);
	f.WriteLine("detail %s", detail);
	f.WriteLine("baked %s", g_BakedName);  // a recording's own baked file (data/tasurf/baked/<name>.tas)
	delete f;
}

// Can an editor request be played now? runner: who plays it. why: what to tell the editor if not.
bool Remote_Ready(int human, int &runner, char[] why, int whylen)
{
	runner = 0;
	why[0] = '\0';
	if (human <= 0)
	{
		strcopy(why, whylen, "no player in the game yet");
		return false;
	}
	if (g_Recording)
	{
		strcopy(why, whylen, "you're recording (tas_record again saves it)");
		return false;
	}
	if (g_cvBot.BoolValue && g_Mode == Mode_Record)
	{
		strcopy(why, whylen, "you're in record mode: type tas_mode edit in the game (requests wait until then)");
		return false;
	}
	runner = Runner_Get(human, why, whylen);
	return runner > 0;
}

// requests/heartbeat.txt, about once a second, so the editor can tell "the
// game isn't running TASurf" apart from "not ready, and why" or "busy".
void Remote_WriteHeartbeat(int target, bool ready, const char[] why)
{
	char path[PLATFORM_MAX_PATH], map[PLATFORM_MAX_PATH], mode[16];
	BuildPath(Path_SM, path, sizeof(path), "%s/requests/heartbeat.txt", TAS_DATA_DIR);
	File f = OpenFile(path, "w");
	if (f == null)
	{
		return;
	}
	GetCurrentMap(map, sizeof(map));
	Mode_Name(mode, sizeof(mode));
	f.WriteLine("version %s", TAS_VERSION);
	f.WriteLine("map %s", map);
	f.WriteLine("mode %s", mode);
	f.WriteLine("bot %s", g_BotStatus);
	f.WriteLine("ready %s", ready ? "yes" : "no");
	if (!ready)
	{
		f.WriteLine("why %s", why);
	}
	if (target <= 0)
	{
		f.WriteLine("player none");
	}
	else if (IsPlayerAlive(target))
	{
		f.WriteLine("player alive");
	}
	else if (GetClientTeam(target) <= 1)
	{
		f.WriteLine("player spectator");
	}
	else
	{
		f.WriteLine("player dead");
	}
	if (g_RunState == Run_Idle)
	{
		f.WriteLine("state idle");
	}
	else if (g_Preview)
	{
		f.WriteLine("state preview");
	}
	else if (g_Recording)
	{
		f.WriteLine("state recording");
	}
	else
	{
		f.WriteLine("state running");
	}
	f.WriteLine("time %d", GetTime());
	delete f;
}

// What the editor wants on screen: requests/draw.txt
//   id <changes whenever the choice changes>
//   map <map>
//   path <run csv in runs/>      (drawn in speed colours)
//   ghost <run csv in runs/>     (optional, drawn thin and grey)
//   timeline <name>              (the timeline open in the editor)
//   cursor <tick>                (its selected tick: path red up to it, white a second after; - for none)
//   files <stamp>                (changes whenever the CSVs do; file times here only have whole seconds)
// The file stays in place, so after a plugin reload or map change the same
// paths are drawn again by themselves.
char g_LastDrawId[64];
char g_LastDrawFiles[64];

// A run CSV drawn for the editor, kept loaded: the cursor moves far more often than the files change.
enum struct EditorPath
{
	char name[PLATFORM_MAX_PATH];
	int mtime;
	int firstTick;
	bool ok;
	ArrayList points;
}
EditorPath g_EdMain;
EditorPath g_EdGhost;

void Remote_ResetDraw()
{
	g_LastDrawId[0] = '\0';
	g_EdMain.name[0] = '\0';
	g_EdGhost.name[0] = '\0';
}

// Load (if it changed) the run CSV name into ep. changed: it was read again (or failed again).
bool Remote_EditorPath(int client, const char[] name, EditorPath ep, bool &changed)
{
	changed = false;
	if (ep.points == null)
	{
		ep.points = new ArrayList(DRAW_POINT);
	}
	if (name[0] == '\0' || StrEqual(name, "-") || !IsValidName(name))
	{
		ep.name[0] = '\0';
		ep.ok = false;
		return false;
	}
	char path[PLATFORM_MAX_PATH], error[256];
	BuildPath(Path_SM, path, sizeof(path), "%s/runs/%s", TAS_DATA_DIR, name);
	int mtime = GetFileTime(path, FileTime_LastChange);
	if (StrEqual(name, ep.name) && mtime == ep.mtime)
	{
		return ep.ok;
	}
	changed = true;
	strcopy(ep.name, sizeof(ep.name), name);
	ep.mtime = mtime;
	ep.points.Clear();
	ep.ok = Draw_LoadCsv(path, ep.points, ep.firstTick, error, sizeof(error));
	if (!ep.ok)
	{
		TAS_Print(client, "editor: can't draw %s: %s", name, error);
	}
	return ep.ok;
}

void Remote_CheckDraw(int target)
{
	if (target <= 0 || !IsClientInGame(target) || g_RunState != Run_Idle || Render_Active())
	{
		return;  // a run draws its own result when it ends; nothing is drawn into a render's video
	}
	char path[PLATFORM_MAX_PATH];
	BuildPath(Path_SM, path, sizeof(path), "%s/requests/draw.txt", TAS_DATA_DIR);
	if (!FileExists(path))
	{
		return;
	}
	File f = OpenFile(path, "r");
	if (f == null)
	{
		return;
	}
	char line[PLATFORM_MAX_PATH], id[64], map[PLATFORM_MAX_PATH], main[PLATFORM_MAX_PATH], ghost[PLATFORM_MAX_PATH];
	char timeline[TAS_MAX_NAME], files[64];
	int cursor = -1;
	bool box = true;
	while (f.ReadLine(line, sizeof(line)))
	{
		TrimString(line);
		if (strncmp(line, "id ", 3) == 0)
		{
			strcopy(id, sizeof(id), line[3]);
		}
		else if (strncmp(line, "map ", 4) == 0)
		{
			strcopy(map, sizeof(map), line[4]);
		}
		else if (strncmp(line, "path ", 5) == 0)
		{
			strcopy(main, sizeof(main), line[5]);
		}
		else if (strncmp(line, "ghost ", 6) == 0)
		{
			strcopy(ghost, sizeof(ghost), line[6]);
		}
		else if (strncmp(line, "timeline ", 9) == 0)
		{
			strcopy(timeline, sizeof(timeline), line[9]);
		}
		else if (strncmp(line, "cursor ", 7) == 0 && IsCharNumeric(line[7]))
		{
			cursor = StringToInt(line[7]);
		}
		else if (strncmp(line, "files ", 6) == 0)
		{
			strcopy(files, sizeof(files), line[6]);
		}
		else if (strncmp(line, "box ", 4) == 0)
		{
			box = StringToInt(line[4]) != 0;
		}
	}
	delete f;
	if (id[0] == '\0' || StrEqual(id, g_LastDrawId))
	{
		return;
	}
	strcopy(g_LastDrawId, sizeof(g_LastDrawId), id);
	char current[PLATFORM_MAX_PATH];
	GetCurrentMap(current, sizeof(current));
	if (!StrEqual(map, current, false))
	{
		return;  // drawn for another map
	}
	strcopy(g_EditorTimeline, sizeof(g_EditorTimeline), IsValidName(timeline) ? timeline : "");
	g_EditorCursor = cursor;
	g_EditorBox = box;
	if (!StrEqual(files, g_LastDrawFiles))
	{
		strcopy(g_LastDrawFiles, sizeof(g_LastDrawFiles), files);
		g_EdMain.name[0] = '\0';  // read the CSVs again
		g_EdGhost.name[0] = '\0';
	}
	bool ghostChanged, mainChanged;
	bool haveGhost = Remote_EditorPath(target, ghost, g_EdGhost, ghostChanged);
	bool haveMain = Remote_EditorPath(target, main, g_EdMain, mainChanged);
	if (!haveGhost && !haveMain)
	{
		return;
	}
	bool quiet = !ghostChanged && !mainChanged;  // only the cursor moved: no console lines
	Draw_Clear();
	if (haveGhost)
	{
		Draw_AddPath(target, g_EdGhost.points, g_EdGhost.firstTick, true, -1, quiet);
	}
	if (haveMain)
	{
		Draw_AddPath(target, g_EdMain.points, g_EdMain.firstTick, false, cursor, quiet);
	}
	Draw_Start(target);
	if (quiet)
	{
		return;
	}
	if (haveGhost)
	{
		TAS_Print(target, "editor: showing %s (grey: %s)", main, ghost);
	}
	else
	{
		TAS_Print(target, "editor: showing %s", main);
	}
}

public Action Timer_Remote(Handle timer)
{
	static int beat;
	beat++;
	int target = ResolveTarget(0);
	Bot_Maintain(target);
	Mode_Tick(target);
	int runner;
	char why[192];
	bool ready = Remote_Ready(target, runner, why, sizeof(why));
	if (beat % 4 == 1)
	{
		Remote_WriteHeartbeat(target, ready, why);
	}
	Remote_CheckDraw(target);
	if (Remote_CheckChangeMap(target))
	{
		return Plugin_Continue;  // a map change asked for: run requests wait for the new map
	}
	// an editor preview still playing is replaced by whatever the editor asks for next
	bool supersede = g_RunState != Run_Idle && g_Preview && g_RequestId[0] != '\0' && !g_Recording;
	if ((g_RunState != Run_Idle && !supersede) || Render_Active())
	{
		return Plugin_Continue;  // a render takes nothing new until you're back
	}
	char path[PLATFORM_MAX_PATH], taken[PLATFORM_MAX_PATH];
	BuildPath(Path_SM, path, sizeof(path), "%s/requests/request.txt", TAS_DATA_DIR);
	if (!FileExists(path))
	{
		return Plugin_Continue;
	}
	char id[64], name[TAS_MAX_NAME];
	bool run, preview;
	int recordFrom;
	if (!Remote_ReadRequest(path, id, sizeof(id), name, sizeof(name), run, preview, recordFrom))
	{
		return Plugin_Continue;
	}
	if (recordFrom >= 0)
	{
		ready = Remote_ReadyToRecord(target, why, sizeof(why));  // a recording plays on you
	}
	if (!ready)
	{
		if (target > 0 && beat % 40 == 1)
		{
			TAS_Chat(target, "the editor is waiting: %s", why);
		}
		return Plugin_Continue;  // keep the request until it can be played
	}
	BuildPath(Path_SM, taken, sizeof(taken), "%s/requests/request.taken", TAS_DATA_DIR);
	if (FileExists(taken))
	{
		DeleteFile(taken);
	}
	if (!RenameFile(taken, path))
	{
		return Plugin_Continue;
	}
	// read again: the editor may have replaced it between the look and the rename
	bool readOk = Remote_ReadRequest(taken, id, sizeof(id), name, sizeof(name), run, preview, recordFrom);
	DeleteFile(taken);

	if (supersede)
	{
		Preview_Supersede();
	}
	if (!readOk || !IsValidName(id) || !IsValidName(name) || !run)
	{
		LogError("ignoring malformed editor request %s", path);
		Remote_EndPreview();
		return Plugin_Continue;
	}
	if (recordFrom >= 0 ? !Remote_ReadyToRecord(target, why, sizeof(why)) : !Remote_Ready(target, runner, why, sizeof(why)))
	{
		Remote_WriteResult(id, "error", "-", why);
		Remote_EndPreview();
		return Plugin_Continue;
	}
	char baked[PLATFORM_MAX_PATH], error[256];
	if (!Baked_ResolvePath(name, baked, sizeof(baked)))
	{
		Remote_WriteResult(id, "error", "-", "no such baked file");
		Remote_EndPreview();
		return Plugin_Continue;
	}
	if (!Baked_Load(baked, error, sizeof(error)))
	{
		Remote_WriteResult(id, "error", "-", error);
		Remote_EndPreview();
		return Plugin_Continue;
	}
	if (recordFrom >= 0)
	{
		// "record from here": the timeline plays you to that tick, then you play and are recorded
		Remote_EndPreview();
		if (g_cvBot.BoolValue && g_Mode == Mode_Edit)
		{
			Mode_Set(Mode_Record, target);
		}
		TAS_Print(target, "editor request: recording from tick %d of %s", recordFrom, g_BakedName);
		if (!Run_CheckReady(target, target))
		{
			Remote_WriteResult(id, "error", "-", "the game refused to start it (settings or maxspeed; see the game console)");
			return Plugin_Continue;
		}
		strcopy(g_RequestId, sizeof(g_RequestId), id);
		if (!Recording_BeginFrom(target, recordFrom))
		{
			Remote_WriteResult(id, "error", "-", "can't record from that tick (see the game console)");
			g_RequestId[0] = '\0';
		}
		return Plugin_Continue;
	}
	if (g_ReqRender && (!Render_ValidFile(g_ReqRenderFile) || runner == target))
	{
		Remote_WriteResult(id, "error", "-", runner == target ? "a render plays on the replay bot: tas_bot 1" : "bad movie file name");
		Remote_EndPreview();
		return Plugin_Continue;
	}
	TAS_Print(target, "editor request: %s %s", g_ReqRender ? "rendering" : preview ? "previewing" : "running", g_BakedName);
	if (!Run_CheckReady(runner, target))
	{
		Remote_WriteResult(id, "error", "-", "the game refused to run it (settings or maxspeed; see the game console)");
		Remote_EndPreview();
		return Plugin_Continue;
	}
	strcopy(g_RequestId, sizeof(g_RequestId), id);
	g_BatchTotal = 1;
	g_BatchDone = 0;
	g_BatchIdentical = 0;
	g_BatchDiverged = 0;
	if (g_ReqRender)
	{
		Remote_EndPreview();
		Render_Begin(runner, target);  // the run starts once you're watching
		return Plugin_Continue;
	}
	if (preview)
	{
		Preview_Enter(runner, target);
	}
	else
	{
		Remote_EndPreview();  // a run at normal speed after a preview it replaced
	}
	Run_Begin(runner, target);
	return Plugin_Continue;
}

// The editor's "change to this map" (requests/changemap.txt): taken once no run, render or recording is under way;
// one older than 30 seconds is dropped, so a game started later doesn't act on it. The result says what happened.
// True while one is waiting (run requests wait too).
bool Remote_CheckChangeMap(int target)
{
	char path[PLATFORM_MAX_PATH];
	BuildPath(Path_SM, path, sizeof(path), "%s/requests/changemap.txt", TAS_DATA_DIR);
	if (!FileExists(path))
	{
		return false;
	}
	if (g_RunState != Run_Idle || Render_Active())
	{
		return true;  // after what's playing
	}
	int age = GetTime() - GetFileTime(path, FileTime_LastChange);
	char id[64], map[PLATFORM_MAX_PATH], line[256];
	id[0] = '\0';
	map[0] = '\0';
	File f = OpenFile(path, "r");
	if (f != null)
	{
		while (f.ReadLine(line, sizeof(line)))
		{
			TrimString(line);
			if (strncmp(line, "id ", 3) == 0)
			{
				strcopy(id, sizeof(id), line[3]);
			}
			else if (strncmp(line, "map ", 4) == 0)
			{
				strcopy(map, sizeof(map), line[4]);
			}
		}
		delete f;
	}
	DeleteFile(path);
	if (!IsValidName(id))
	{
		return false;
	}
	char current[PLATFORM_MAX_PATH];
	GetCurrentMap(current, sizeof(current));
	if (age > 30)
	{
		Remote_WriteResult(id, "error", "-", "the request was too old");
	}
	else if (StrEqual(current, map, false))
	{
		Remote_WriteResult(id, "complete", "-", "already on that map");
	}
	else if (!IsValidName(map) || !IsMapValid(map))
	{
		Remote_WriteResult(id, "error", "-", "the game doesn't have that map");
	}
	else
	{
		Remote_WriteResult(id, "complete", "-", "changing the map");
		TAS_Print(target, "the editor opened a timeline on %s: changing the map", map);
		ForceChangeLevel(map, "TASurf: the editor opened a timeline on this map");
	}
	return false;
}

// One request file: id, baked name, and what to do (run, preview, or record from a tick: recordFrom, else -1).
bool Remote_ReadRequest(const char[] path, char[] id, int idlen, char[] name, int namelen, bool &run, bool &preview, int &recordFrom)
{
	id[0] = '\0';
	name[0] = '\0';
	run = false;
	preview = false;
	recordFrom = -1;
	Render_ResetRequest();
	File f = OpenFile(path, "r");
	if (f == null)
	{
		return false;
	}
	char line[256];
	while (f.ReadLine(line, sizeof(line)))
	{
		TrimString(line);
		if (strncmp(line, "id ", 3) == 0)
		{
			strcopy(id, idlen, line[3]);
		}
		else if (strncmp(line, "load ", 5) == 0)
		{
			strcopy(name, namelen, line[5]);
		}
		else if (StrEqual(line, "run"))
		{
			run = true;
		}
		else if (StrEqual(line, "preview"))
		{
			run = true;
			preview = true;
		}
		else if (strncmp(line, "record ", 7) == 0 && IsCharNumeric(line[7]))
		{
			run = true;
			recordFrom = StringToInt(line[7]);
		}
		else if (strncmp(line, "render ", 7) == 0)
		{
			run = true;
			g_ReqRender = true;
			strcopy(g_ReqRenderFile, sizeof(g_ReqRenderFile), line[7]);
		}
		else if (strncmp(line, "tail ", 5) == 0)
		{
			g_ReqRenderTail = StringToFloat(line[5]);
		}
		else if (strncmp(line, "viewmodel ", 10) == 0)
		{
			g_ReqRenderViewmodel = StringToInt(line[10]) != 0;
		}
	}
	delete f;
	return true;
}

bool Remote_ReadyToRecord(int human, char[] why, int whylen)
{
	if (human <= 0)
	{
		strcopy(why, whylen, "no player in the game yet");
		return false;
	}
	if (!IsPlayerAlive(human))
	{
		strcopy(why, whylen, "you need to be alive to record: join a team");
		return false;
	}
	if (g_Recording)
	{
		strcopy(why, whylen, "you're recording already (tas_record saves it)");
		return false;
	}
	return true;
}

// After a superseded preview, when no new preview follows: normal speed, you unfrozen.
void Remote_EndPreview()
{
	if (g_Preview && g_RunState == Run_Idle)
	{
		Preview_Restore();
	}
}
