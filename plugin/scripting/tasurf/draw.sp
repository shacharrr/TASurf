// Path drawing with temp-entity beams.
//
// Temp-entity beams can't be removed and live at most 25.6 s, and only a few
// dozen temp entities fit in one update. So the path is kept as a list of
// beams and re-sent in a cycle: tas_draw_budget beams per server frame, each
// beam with a lifetime just long enough to last until it is sent again. After
// tas_clear the last beams fade within one lifetime (normally ~1 s).

#define DRAW_POINT 5  // a path point: x, y, z (feet), horizontal speed, ducked (1.0) or not

// The player's collision box, standing [0] and ducked [1]: CS:S's 32 x 32 x 72 / 54 until
// a run shows the real ones (read from the player it plays on, see Draw_NoteHull).
float g_HullMins[2][3] = { { -16.0, -16.0, 0.0 }, { -16.0, -16.0, 0.0 } };
float g_HullMaxs[2][3] = { { 16.0, 16.0, 72.0 }, { 16.0, 16.0, 54.0 } };

ArrayList g_DrawBeams;
bool g_Drawing;
int g_DrawUserId;
int g_DrawCursor;
float g_DrawLife;
float g_DrawRepeat;
float g_DrawNextCycle;
int g_BeamModel;

// The tick selected in the browser editor (requests/draw.txt), for its timeline: the path is
// drawn red up to it and white for one second of ticks after it. -1: no cursor.
char g_EditorTimeline[TAS_MAX_NAME];
int g_EditorCursor = -1;
bool g_EditorBox = true;  // the editor's Visuals toolbox can turn the box off ("box 0" in draw.txt)

// The editor's cursor if name (a run or preview of a timeline) is the timeline it has open.
int Draw_CursorFor(const char[] name)
{
	if (g_EditorCursor < 0 || g_EditorTimeline[0] == '\0')
	{
		return -1;
	}
	char preview[TAS_MAX_NAME + 8];
	FormatEx(preview, sizeof(preview), "%s.preview", g_EditorTimeline);
	return (StrEqual(name, g_EditorTimeline) || StrEqual(name, preview)) ? g_EditorCursor : -1;
}

float g_SpeedStops[5][3] =
{
	{ 0.0, 64.0, 255.0 },   // slow: blue
	{ 0.0, 255.0, 255.0 },
	{ 0.0, 255.0, 0.0 },
	{ 255.0, 255.0, 0.0 },
	{ 255.0, 0.0, 0.0 }     // fast: red
};

void Draw_Init()
{
	g_DrawBeams = new ArrayList(sizeof(PathBeam));
}

void Draw_Precache()
{
	char material[PLATFORM_MAX_PATH];
	g_cvBeamMaterial.GetString(material, sizeof(material));
	g_BeamModel = PrecacheModel(material, true);
}

void Draw_Clear()
{
	g_Drawing = false;
	g_DrawBeams.Clear();
	g_DrawCursor = 0;
}

void SpeedColor(float speed, float lo, float hi, int color[4])
{
	float t = (hi > lo) ? (speed - lo) / (hi - lo) : 0.0;
	if (t < 0.0)
	{
		t = 0.0;
	}
	if (t > 1.0)
	{
		t = 1.0;
	}
	float s = t * 4.0;
	int k = RoundToFloor(s);
	if (k > 3)
	{
		k = 3;
	}
	float frac = s - float(k);
	for (int c = 0; c < 3; c++)
	{
		color[c] = RoundToNearest(g_SpeedStops[k][c] + (g_SpeedStops[k + 1][c] - g_SpeedStops[k][c]) * frac);
	}
	color[3] = 255;
}

void Draw_AddPost(const float p[DRAW_POINT], float height, int r, int g, int b)
{
	PathBeam beam;
	beam.a[0] = p[0];
	beam.a[1] = p[1];
	beam.a[2] = p[2];
	beam.b[0] = p[0];
	beam.b[1] = p[1];
	beam.b[2] = p[2] + height;
	beam.color[0] = r;
	beam.color[1] = g;
	beam.color[2] = b;
	beam.color[3] = 255;
	beam.width = g_cvBeamWidth.FloatValue;
	g_DrawBeams.PushArray(beam, sizeof(beam));
}

// points: float[DRAW_POINT] = x, y, z, horizontal speed, ducked. firstTick: the tick number of
// points[0] (-1 when points[0] is the start state). A ghost path (the previous
// run, for comparison) is drawn thin and grey, without markers. cursor: the
// editor's selected tick: red up to it, white for one second after it, the player's
// box on it (tas_draw_box) with a yellow post above, and speed colours from there on
// (-1: speed colours throughout).
void Draw_AddPath(int client, ArrayList points, int firstTick, bool ghost, int cursor = -1, bool quiet = false)
{
	int n = points.Length;
	if (n < 2)
	{
		return;
	}
	if (g_BeamModel <= 0)
	{
		Draw_Precache();
	}

	float p[DRAW_POINT], q[DRAW_POINT];
	float lo = g_cvSpeedMin.FloatValue, hi = g_cvSpeedMax.FloatValue;
	if (lo < 0.0 || hi < 0.0)
	{
		float minSpeed = 1.0e9, maxSpeed = 0.0;
		for (int i = 0; i < n; i++)
		{
			points.GetArray(i, p, sizeof(p));
			minSpeed = p[3] < minSpeed ? p[3] : minSpeed;
			maxSpeed = p[3] > maxSpeed ? p[3] : maxSpeed;
		}
		if (lo < 0.0)
		{
			lo = minSpeed;
		}
		if (hi < 0.0)
		{
			hi = maxSpeed;
		}
	}

	int maxSegments = g_cvDrawMaxSegments.IntValue;
	if (maxSegments < 1)
	{
		maxSegments = 1;
	}
	int step = (n - 1 + maxSegments - 1) / maxSegments;
	if (step < 1)
	{
		step = 1;
	}
	float zoff = g_cvDrawZOffset.FloatValue;
	float width = g_cvBeamWidth.FloatValue;
	int perSecond = RoundToNearest(1.0 / GetTickInterval());
	int cursorAt = -1, whiteTo = -1;  // point indices
	if (cursor >= 0 && !ghost)
	{
		cursorAt = cursor - firstTick;
		cursorAt = cursorAt < 0 ? 0 : (cursorAt > n - 1 ? n - 1 : cursorAt);
		whiteTo = cursorAt + perSecond > n - 1 ? n - 1 : cursorAt + perSecond;
	}
	PathBeam beam;
	for (int i = 0; i < n - 1; )
	{
		int j = i + step;
		if (cursorAt > i && cursorAt < j)
		{
			j = cursorAt;  // the colours change exactly at the cursor and a second after it
		}
		if (whiteTo > i && whiteTo < j)
		{
			j = whiteTo;
		}
		if (j > n - 1)
		{
			j = n - 1;
		}
		points.GetArray(i, p, sizeof(p));
		points.GetArray(j, q, sizeof(q));
		for (int c = 0; c < 3; c++)
		{
			beam.a[c] = p[c];
			beam.b[c] = q[c];
		}
		beam.a[2] += zoff;
		beam.b[2] += zoff;
		if (ghost)
		{
			beam.color[0] = 150;
			beam.color[1] = 150;
			beam.color[2] = 150;
			beam.color[3] = 255;
			beam.width = width * 0.5;
		}
		else if (cursorAt >= 0 && j <= cursorAt)
		{
			SetColor(beam.color, 255, 30, 30);  // up to the editor's tick
			beam.width = width;
		}
		else if (cursorAt >= 0 && j <= whiteTo)
		{
			SetColor(beam.color, 255, 255, 255);  // the second after it
			beam.width = width;
		}
		else
		{
			SpeedColor(q[3], lo, hi, beam.color);
			beam.width = width;
		}
		g_DrawBeams.PushArray(beam, sizeof(beam));
		i = j;
	}
	if (ghost)
	{
		return;
	}

	// Markers: white post every N ticks, green post at the start, red at the end.
	int interval = g_cvMarkerInterval.IntValue;
	if (interval <= 0)
	{
		interval = RoundToNearest(1.0 / GetTickInterval());  // one per second of game time
	}
	for (int i = 0; i < n; i++)
	{
		int tick = firstTick + i;
		points.GetArray(i, p, sizeof(p));
		if (i == 0)
		{
			Draw_AddPost(p, 48.0, 0, 255, 0);
		}
		else if (i == n - 1)
		{
			Draw_AddPost(p, 48.0, 255, 0, 0);
		}
		else if (tick > 0 && tick % interval == 0)
		{
			Draw_AddPost(p, 16.0, 255, 255, 255);
		}
	}
	if (cursorAt >= 0)
	{
		// where the editor's tick is: the player's box there, and a post above it to spot it from afar
		points.GetArray(cursorAt, p, sizeof(p));
		int duck = p[4] > 0.5 ? 1 : 0;
		if (g_cvDrawBox.BoolValue && g_EditorBox)
		{
			Draw_AddBox(p, duck, 255, 220, 0);
		}
		float top[DRAW_POINT];
		top = p;
		top[2] += g_HullMaxs[duck][2];
		Draw_AddPost(top, 40.0, 255, 220, 0);
	}
	if (quiet)
	{
		return;
	}
	TAS_Print(client, "drawing %d points (1 segment per %d ticks, speed colours %.0f..%.0f u/s, marker every %d ticks)",
		n, step, lo, hi, interval);
	if (cursorAt >= 0)
	{
		TAS_Print(client, "  red: up to the editor's tick %d (yellow post), white: the %d ticks after it", cursor, perSecond);
	}
}

// The player's collision box at a point (feet at p, as the engine's origin): its 12 edges.
void Draw_AddBox(const float p[DRAW_POINT], int duck, int r, int g, int b)
{
	float corner[8][3];
	for (int i = 0; i < 8; i++)
	{
		corner[i][0] = p[0] + ((i & 1) ? g_HullMaxs[duck][0] : g_HullMins[duck][0]);
		corner[i][1] = p[1] + ((i & 2) ? g_HullMaxs[duck][1] : g_HullMins[duck][1]);
		corner[i][2] = p[2] + ((i & 4) ? g_HullMaxs[duck][2] : g_HullMins[duck][2]);
	}
	PathBeam beam;
	SetColor(beam.color, r, g, b);
	beam.width = g_cvBeamWidth.FloatValue * 0.6;
	for (int i = 0; i < 8; i++)
	{
		for (int bit = 1; bit < 8; bit <<= 1)
		{
			if (i & bit)
			{
				continue;  // each edge once: from the corner without the bit to the one with it
			}
			for (int c = 0; c < 3; c++)
			{
				beam.a[c] = corner[i][c];
				beam.b[c] = corner[i | bit][c];
			}
			g_DrawBeams.PushArray(beam, sizeof(beam));
		}
	}
}

// Remember the real collision box of the player a run plays on, standing or ducked.
void Draw_NoteHull(int client)
{
	if (!HasEntProp(client, Prop_Send, "m_vecMaxs"))
	{
		return;
	}
	int duck = (GetEntityFlags(client) & FL_DUCKING) ? 1 : 0;
	GetEntPropVector(client, Prop_Send, "m_vecMins", g_HullMins[duck]);
	GetEntPropVector(client, Prop_Send, "m_vecMaxs", g_HullMaxs[duck]);
}

void SetColor(int color[4], int r, int g, int b)
{
	color[0] = r;
	color[1] = g;
	color[2] = b;
	color[3] = 255;
}

// Start (re)sending everything added since Draw_Clear.
void Draw_Start(int client)
{
	if (g_DrawBeams.Length == 0)
	{
		TAS_Print(client, "nothing to draw (fewer than 2 points)");
		return;
	}
	int budget = g_cvDrawBudget.IntValue;
	if (budget < 1)
	{
		budget = 1;
	}
	float period = float(g_DrawBeams.Length) / (float(budget) / GetTickInterval());
	g_DrawLife = period + 0.3;
	if (g_DrawLife < 1.0)
	{
		g_DrawLife = 1.0;
	}
	if (g_DrawLife > 25.0)
	{
		g_DrawLife = 25.0;
		TAS_Print(client, "warning: %d beams take %.1f s to send; the path will flicker. Lower tas_draw_max_segments or raise tas_draw_budget.",
			g_DrawBeams.Length, period);
	}
	g_DrawRepeat = g_DrawLife - 0.15;
	g_DrawUserId = GetClientUserId(client);
	g_DrawCursor = 0;
	g_DrawNextCycle = 0.0;
	g_Drawing = true;
}

void Draw_Build(int client, ArrayList points, int firstTick)
{
	Draw_Clear();
	Draw_AddPath(client, points, firstTick, false);
	Draw_Start(client);
}

// Points of a recorded run, starting with its start state.
ArrayList Draw_RunPoints(ArrayList records, const float startOrigin[3], float startSpeed)
{
	ArrayList points = new ArrayList(DRAW_POINT);
	float p[DRAW_POINT];
	p[0] = startOrigin[0];
	p[1] = startOrigin[1];
	p[2] = startOrigin[2];
	p[3] = startSpeed;
	p[4] = 0.0;  // starts are standing
	points.PushArray(p, sizeof(p));
	TickRecord r;
	for (int i = 0; i < records.Length; i++)
	{
		records.GetArray(i, r, sizeof(r));
		p[0] = r.origin[0];
		p[1] = r.origin[1];
		p[2] = r.origin[2];
		p[3] = r.hspeed;
		p[4] = float(r.ducked);
		points.PushArray(p, sizeof(p));
	}
	return points;
}

// The last run, plus (tas_draw_ghost) the run before it of the same timeline in grey.
void Draw_FromLastRun(int client)
{
	if (g_LastRecord.Length == 0)
	{
		TAS_Print(client, "no run to draw yet");
		return;
	}
	Draw_Clear();
	if (g_cvDrawGhost.BoolValue && g_GhostRecord.Length > 0 && StrEqual(g_GhostName, g_LastName)
		&& StrEqual(g_GhostMap, g_LastMap))
	{
		ArrayList ghost = Draw_RunPoints(g_GhostRecord, g_GhostStartOrigin, g_GhostStartSpeed);
		Draw_AddPath(client, ghost, -1, true);
		delete ghost;
		TAS_Print(client, "  grey: the previous run of %s, for comparison (tas_draw_ghost 0 hides it)", g_LastName);
	}
	ArrayList points = Draw_RunPoints(g_LastRecord, g_LastStartOrigin, g_LastStartSpeed);
	Draw_AddPath(client, points, -1, false, Draw_CursorFor(g_LastName));
	delete points;
	Draw_Start(client);
}

// Positions and speeds from a run CSV (data/tasurf/runs).
bool Draw_LoadCsv(const char[] path, ArrayList points, int &firstTick, char[] error, int errlen)
{
	error[0] = '\0';
	File f = OpenFile(path, "r");
	if (f == null)
	{
		FormatEx(error, errlen, "cannot open %s", path);
		return false;
	}
	char line[1024];
	char cells[32][32];
	int itick = -1, ix = -1, iy = -1, iz = -1, ihs = -1, iduck = -1;
	bool haveColumns = false;
	firstTick = 0;
	while (f.ReadLine(line, sizeof(line)))
	{
		TrimString(line);
		if (line[0] == '\0' || line[0] == '#')
		{
			continue;
		}
		int n = ExplodeString(line, ",", cells, sizeof(cells), sizeof(cells[]));
		if (!haveColumns)
		{
			for (int i = 0; i < n; i++)
			{
				if (StrEqual(cells[i], "tick")) itick = i;
				else if (StrEqual(cells[i], "x")) ix = i;
				else if (StrEqual(cells[i], "y")) iy = i;
				else if (StrEqual(cells[i], "z")) iz = i;
				else if (StrEqual(cells[i], "hspeed")) ihs = i;
				else if (StrEqual(cells[i], "ducked")) iduck = i;
			}
			haveColumns = true;
			if (itick < 0 || ix < 0 || iy < 0 || iz < 0 || ihs < 0)
			{
				strcopy(error, errlen, "not a TASurf run CSV (missing tick/x/y/z/hspeed columns)");
				break;
			}
			continue;
		}
		float p[DRAW_POINT];
		p[0] = StringToFloat(cells[ix]);
		p[1] = StringToFloat(cells[iy]);
		p[2] = StringToFloat(cells[iz]);
		p[3] = StringToFloat(cells[ihs]);
		p[4] = iduck >= 0 ? float(StringToInt(cells[iduck])) : 0.0;
		if (points.Length == 0)
		{
			firstTick = StringToInt(cells[itick]);
		}
		points.PushArray(p, sizeof(p));
	}
	delete f;
	return error[0] == '\0';
}

// Draw a run CSV from data/tasurf/runs (e.g. an older run).
bool Draw_FromCsv(int client, const char[] path, char[] error, int errlen)
{
	ArrayList points = new ArrayList(DRAW_POINT);
	int firstTick;
	bool ok = Draw_LoadCsv(path, points, firstTick, error, errlen);
	if (ok)
	{
		Draw_Build(client, points, firstTick);
	}
	delete points;
	return ok;
}

public void OnGameFrame()
{
	if (!g_Drawing)
	{
		return;
	}
	int client = GetClientOfUserId(g_DrawUserId);
	if (client == 0 || !IsClientInGame(client))
	{
		g_Drawing = false;
		return;
	}
	int count = g_DrawBeams.Length;
	if (count == 0)
	{
		return;
	}
	if (g_DrawCursor == 0)
	{
		float now = GetGameTime();
		if (now < g_DrawNextCycle)
		{
			return;
		}
		g_DrawNextCycle = now + g_DrawRepeat;
	}
	int budget = g_cvDrawBudget.IntValue;
	PathBeam beam;
	for (int i = 0; i < budget && g_DrawCursor < count; i++)
	{
		g_DrawBeams.GetArray(g_DrawCursor, beam, sizeof(beam));
		TE_SetupBeamPoints(beam.a, beam.b, g_BeamModel, 0, 0, 0, g_DrawLife, beam.width, beam.width, 0, 0.0, beam.color, 0);
		TE_SendToClient(client);
		g_DrawCursor++;
	}
	if (g_DrawCursor >= count)
	{
		g_DrawCursor = 0;
	}
}
