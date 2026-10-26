// Baked timeline files (docs/file-formats.md). Same parsing rules as
// read_baked() in tasurf/bake.py. tas_run re-reads the file every time, so a
// re-bake is picked up without tas_load.

#define SEEN_ORIGIN         (1 << 0)
#define SEEN_ANGLES         (1 << 1)
#define SEEN_VELOCITY       (1 << 2)
#define SEEN_ORIGIN_BITS    (1 << 3)
#define SEEN_ANGLES_BITS    (1 << 4)
#define SEEN_VELOCITY_BITS  (1 << 5)
#define VIEW_PITCH_PER_LINE 16  // values on one @view_pitch line (VIEW_PITCH_PER_LINE in bake.py)
#define VIEW_YAW_PER_LINE 16    // and on one @view_yaw line (camera.py)

bool g_HaveBaked;
bool g_HasViewPitch;         // the file has @view_pitch lines: TickInput.viewPitch is what replays show
bool g_AutoBhop;             // the file has @autobhop: a held jump jumps again on landing (autobhop.sp)
char g_BakedPath[PLATFORM_MAX_PATH];
char g_BakedName[TAS_MAX_NAME];
char g_BakedHash[TAS_HASH_LEN];
char g_BakedMap[PLATFORM_MAX_PATH];
StartState g_Start;
ArrayList g_Inputs;          // TickInput, one per tick
ArrayList g_SettingNames;    // expected settings declared by the timeline
ArrayList g_SettingValues;

void Baked_Init()
{
	g_Inputs = new ArrayList(sizeof(TickInput));
	g_SettingNames = new ArrayList(ByteCountToCells(64));
	g_SettingValues = new ArrayList(ByteCountToCells(64));
}

// "name" -> data/tasurf/baked/name.tas (or the exact file name if it exists).
bool Baked_ResolvePath(const char[] name, char[] path, int maxlen)
{
	BuildPath(Path_SM, path, maxlen, "%s/baked/%s", TAS_DATA_DIR, name);
	if (FileExists(path))
	{
		return true;
	}
	BuildPath(Path_SM, path, maxlen, "%s/baked/%s.tas", TAS_DATA_DIR, name);
	return FileExists(path);
}

bool ParseVec3(const char[][] parts, int first, float v[3])
{
	for (int i = 0; i < 3; i++)
	{
		if (!ParseFloatStrict(parts[first + i], v[i]))
		{
			return false;
		}
	}
	return true;
}

bool ParseBits3(const char[][] parts, int first, float v[3])
{
	for (int i = 0; i < 3; i++)
	{
		int bits;
		if (!ParseHex32(parts[first + i], bits))
		{
			return false;
		}
		v[i] = view_as<float>(bits);
	}
	return true;
}

void CopyVec3(const float src[3], float dst[3])
{
	dst[0] = src[0];
	dst[1] = src[1];
	dst[2] = src[2];
}

// One "@start key values..." line. Exact *_bits values win over decimals.
bool Baked_ParseStart(StartState st, const char[][] parts, int n, int &seen)
{
	float v[3];
	if (StrEqual(parts[1], "origin") || StrEqual(parts[1], "angles") || StrEqual(parts[1], "velocity")
		|| StrEqual(parts[1], "base_velocity"))
	{
		if (n != 5 || !ParseVec3(parts, 2, v))
		{
			return false;
		}
		if (StrEqual(parts[1], "origin"))
		{
			seen |= SEEN_ORIGIN;
			if (!(seen & SEEN_ORIGIN_BITS))
			{
				CopyVec3(v, st.origin);
			}
		}
		else if (StrEqual(parts[1], "angles"))
		{
			seen |= SEEN_ANGLES;
			if (!(seen & SEEN_ANGLES_BITS))
			{
				CopyVec3(v, st.angles);
			}
		}
		else if (StrEqual(parts[1], "velocity"))
		{
			seen |= SEEN_VELOCITY;
			if (!(seen & SEEN_VELOCITY_BITS))
			{
				CopyVec3(v, st.velocity);
			}
		}
		else
		{
			CopyVec3(v, st.baseVelocity);
		}
		return true;
	}
	if (StrEqual(parts[1], "origin_bits") || StrEqual(parts[1], "angles_bits") || StrEqual(parts[1], "velocity_bits"))
	{
		if (n != 5 || !ParseBits3(parts, 2, v))
		{
			return false;
		}
		if (StrEqual(parts[1], "origin_bits"))
		{
			seen |= SEEN_ORIGIN_BITS;
			CopyVec3(v, st.origin);
		}
		else if (StrEqual(parts[1], "angles_bits"))
		{
			seen |= SEEN_ANGLES_BITS;
			CopyVec3(v, st.angles);
		}
		else
		{
			seen |= SEEN_VELOCITY_BITS;
			CopyVec3(v, st.velocity);
		}
		return true;
	}

	if (n != 3)
	{
		return false;
	}
	if (StrEqual(parts[1], "on_ground"))
	{
		if (StrEqual(parts[2], "yes"))
		{
			st.onGround = Ground_Yes;
		}
		else if (StrEqual(parts[2], "no"))
		{
			st.onGround = Ground_No;
		}
		else if (StrEqual(parts[2], "auto"))
		{
			st.onGround = Ground_Auto;
		}
		else
		{
			return false;
		}
		return true;
	}
	if (StrEqual(parts[1], "base_velocity_flag"))
	{
		st.baseVelocityFlag = StringToInt(parts[2]) != 0;
		return true;
	}
	if (StrEqual(parts[1], "ground_entity"))
	{
		st.groundEntity = StringToInt(parts[2]);
		return true;
	}
	if (StrEqual(parts[1], "ducked"))
	{
		return StringToInt(parts[2]) == 0;  // ducked starts are not supported yet
	}
	if (StrEqual(parts[1], "health"))
	{
		st.hasHealth = true;
		st.health = StringToInt(parts[2]);
		return true;
	}

	float f;
	if (!ParseFloatStrict(parts[2], f))
	{
		return false;
	}
	if (StrEqual(parts[1], "gravity"))
	{
		st.gravity = f;
	}
	else if (StrEqual(parts[1], "lagged_movement"))
	{
		st.laggedMovement = f;
	}
	else if (StrEqual(parts[1], "view_offset_z"))
	{
		st.hasViewOffsetZ = true;
		st.viewOffsetZ = f;
	}
	else if (StrEqual(parts[1], "velocity_modifier"))
	{
		st.hasVelocityModifier = true;
		st.velocityModifier = f;
	}
	else if (StrEqual(parts[1], "stamina"))
	{
		st.hasStamina = true;
		st.stamina = f;
	}
	else if (StrEqual(parts[1], "maxspeed"))
	{
		st.hasMaxspeed = true;
		st.maxspeed = f;
	}
	else
	{
		return false;
	}
	return true;
}

// Parse a baked file. On success it replaces the loaded timeline; on failure
// the previously loaded one is kept and error says why.
bool Baked_Load(const char[] path, char[] error, int errlen)
{
	error[0] = '\0';
	File f = OpenFile(path, "r");
	if (f == null)
	{
		FormatEx(error, errlen, "cannot open %s", path);
		return false;
	}

	StartState st;
	st.gravity = 1.0;
	st.laggedMovement = 1.0;
	st.velocityModifier = 1.0;
	st.stamina = 0.0;
	st.onGround = Ground_Auto;

	ArrayList inputs = new ArrayList(sizeof(TickInput));
	ArrayList names = new ArrayList(ByteCountToCells(64));
	ArrayList values = new ArrayList(ByteCountToCells(64));
	char name[TAS_MAX_NAME], hash[TAS_HASH_LEN], map[PLATFORM_MAX_PATH];
	int declaredTicks = -1;
	bool formatOk = false;
	int version = 0;
	int seen = 0;
	bool autobhop = false;
	ArrayList strafe = new ArrayList(8);  // [from, to, offset, straight, heading, swing, smooth, close] of each @strafe line (smooth: it has a gain)
	ArrayList view = new ArrayList();     // the @view_pitch values, one per tick
	ArrayList viewYaw = new ArrayList(2); // [tick, yaw] of each @view_yaw value

	char line[512];
	char parts[8][64];
	char viewParts[VIEW_PITCH_PER_LINE + 3][16];  // the tag, the first tick, the values, and one to notice too many
	int lineNo = 0;
	while (error[0] == '\0' && f.ReadLine(line, sizeof(line)))
	{
		lineNo++;
		TrimString(line);
		if (line[0] == '\0' || line[0] == '#')
		{
			continue;
		}

		// "@view_pitch <first tick> <pitch>...": the pitch replays show on those ticks.
		// Cosmetic, so not hashed: the run itself plays the tick lines' pitch.
		if (strncmp(line, "@view_pitch ", 12) == 0)
		{
			int n = ExplodeString(line, " ", viewParts, sizeof(viewParts), sizeof(viewParts[]));
			char first[16];
			IntToString(view.Length, first, sizeof(first));
			bool ok = n >= 3 && n <= VIEW_PITCH_PER_LINE + 2 && StrEqual(viewParts[1], first);
			for (int i = 2; i < n && ok; i++)
			{
				float v;
				ok = ParseFloatStrict(viewParts[i], v);
				view.Push(v);
			}
			if (!ok)
			{
				FormatEx(error, errlen, "line %d: bad view pitch line (expected @view_pitch %s, at most %d values)", lineNo, first, VIEW_PITCH_PER_LINE);
			}
			continue;
		}

		// "@view_yaw <first tick> <yaw>...": the camera replays show on those ticks (auto-strafe's yaw smoothed, from a
		// run of these inputs). Cosmetic, so not hashed: the run plays the yaw it plays.
		if (strncmp(line, "@view_yaw ", 10) == 0)
		{
			int n = ExplodeString(line, " ", viewParts, sizeof(viewParts), sizeof(viewParts[]));
			int first;
			bool ok = n >= 3 && n <= VIEW_YAW_PER_LINE + 2 && StringToIntEx(viewParts[1], first) == strlen(viewParts[1]) && first >= 0;
			for (int i = 2; i < n && ok; i++)
			{
				any pair[2];
				float v;
				ok = ParseFloatStrict(viewParts[i], v);
				pair[0] = first + i - 2;
				pair[1] = v;
				viewYaw.PushArray(pair);
			}
			if (!ok)
			{
				FormatEx(error, errlen, "line %d: bad view yaw line (expected @view_yaw <first tick>, at most %d values)", lineNo, VIEW_YAW_PER_LINE);
			}
			continue;
		}

		if (line[0] == '@')
		{
			int n = ExplodeString(line, " ", parts, sizeof(parts), sizeof(parts[]));
			if (StrEqual(parts[0], "@format"))
			{
				version = n == 3 ? StringToInt(parts[2]) : 0;
				formatOk = (n == 3 && StrEqual(parts[1], "tasurf-baked")
					&& (StrEqual(parts[2], "1") || StrEqual(parts[2], "2") || StrEqual(parts[2], "3") || StrEqual(parts[2], "4")
						|| StrEqual(parts[2], "5") || StrEqual(parts[2], "6")));
				if (!formatOk)
				{
					FormatEx(error, errlen, "line %d: not a tasurf-baked version 1, 2, 3, 4, 5 or 6 file (newer than this plugin?)", lineNo);
				}
			}
			else if (StrEqual(parts[0], "@name") && n == 2)
			{
				strcopy(name, sizeof(name), parts[1]);
			}
			else if (StrEqual(parts[0], "@hash") && n == 2)
			{
				strcopy(hash, sizeof(hash), parts[1]);
			}
			else if (StrEqual(parts[0], "@map") && n == 2)
			{
				strcopy(map, sizeof(map), parts[1]);
			}
			else if (StrEqual(parts[0], "@ticks") && n == 2)
			{
				declaredTicks = StringToInt(parts[1]);
			}
			else if (StrEqual(parts[0], "@setting") && n == 3)
			{
				names.PushString(parts[1]);
				values.PushString(parts[2]);
			}
			else if (StrEqual(parts[0], "@autobhop") && n == 1 && version >= 4)
			{
				autobhop = true;  // version 4: a held jump jumps again on landing
			}
			else if (StrEqual(parts[0], "@strafe") && ((n == 4 && version >= 2) || (n == 5 && version >= 3) || (n == 6 && version >= 5)
				|| (n == 7 && version >= 6)))
			{
				// version 3: a heading after the offset, for a straight range; version 5: a swing after that; version 6:
				// how much of the most speed gain the swing keeps (75 to 100 percent), turning smoothly
				any range[8];
				float offset, heading, swing, gain;
				range[0] = StringToInt(parts[1]);
				range[1] = StringToInt(parts[2]);
				if (!ParseFloatStrict(parts[3], offset) || range[0] < 0 || range[1] <= range[0]
					|| (n >= 5 && !ParseFloatStrict(parts[4], heading)) || (n >= 6 && (!ParseFloatStrict(parts[5], swing) || swing <= 0.0))
					|| (n == 7 && (!ParseFloatStrict(parts[6], gain) || gain < 75.0 || gain > 100.0)))
				{
					FormatEx(error, errlen, "line %d: bad strafe line \"%s\"", lineNo, line);
				}
				range[2] = offset;
				range[3] = n >= 5;
				range[4] = heading;
				range[5] = n >= 6 ? swing : 0.0;
				range[6] = n == 7;
				range[7] = n == 7 ? 1.0 - 2.0 * SquareRoot(1.0 - gain / 100.0) : 1.0;  // how close to perfect (swing_close)
				strafe.PushArray(range, sizeof(range));
			}
			else if (StrEqual(parts[0], "@start") && n >= 3)
			{
				if (!Baked_ParseStart(st, parts, n, seen))
				{
					FormatEx(error, errlen, "line %d: bad start line \"%s\"", lineNo, line);
				}
			}
			else
			{
				FormatEx(error, errlen, "line %d: unknown header \"%s\"", lineNo, line);
			}
			continue;
		}

		if (strncmp(line, "tick,", 5) == 0)
		{
			continue;
		}

		// 6 columns, or 10 with the exact float32 bits of yaw, pitch,
		// forwardmove and sidemove (recordings), which then win.
		char cols[11][32];
		int n = ExplodeString(line, ",", cols, sizeof(cols), sizeof(cols[]));
		float yaw, pitch, fwd, side;
		bool ok = (n == 6 || n == 10) && StringToInt(cols[0]) == inputs.Length
			&& ParseFloatStrict(cols[1], yaw) && ParseFloatStrict(cols[2], pitch)
			&& ParseFloatStrict(cols[3], fwd) && ParseFloatStrict(cols[4], side);
		if (ok && n == 10)
		{
			int b0, b1, b2, b3;
			ok = ParseHex32(cols[6], b0) && ParseHex32(cols[7], b1) && ParseHex32(cols[8], b2) && ParseHex32(cols[9], b3);
			yaw = view_as<float>(b0);
			pitch = view_as<float>(b1);
			fwd = view_as<float>(b2);
			side = view_as<float>(b3);
		}
		if (!ok)
		{
			FormatEx(error, errlen, "line %d: bad tick line (expected tick %d)", lineNo, inputs.Length);
			continue;
		}
		TickInput ti;
		ti.yaw = yaw;
		ti.pitch = pitch;
		ti.forwardmove = fwd;
		ti.sidemove = side;
		ti.buttons = StringToInt(cols[5]);
		inputs.PushArray(ti, sizeof(ti));
	}
	delete f;

	if (error[0] == '\0')
	{
		if (!formatOk)
		{
			strcopy(error, errlen, "missing @format line");
		}
		else if (name[0] == '\0' || hash[0] == '\0' || map[0] == '\0')
		{
			strcopy(error, errlen, "missing @name, @hash or @map");
		}
		else if (inputs.Length == 0 || declaredTicks != inputs.Length)
		{
			FormatEx(error, errlen, "@ticks says %d but the file has %d tick lines", declaredTicks, inputs.Length);
		}
		else if (!(seen & SEEN_ORIGIN) || !(seen & SEEN_ANGLES))
		{
			strcopy(error, errlen, "missing @start origin or @start angles");
		}
	}
	// auto-strafe ranges become per-tick flags
	for (int i = 0; i < strafe.Length && error[0] == '\0'; i++)
	{
		any range[8];
		strafe.GetArray(i, range, sizeof(range));
		if (range[1] > inputs.Length)
		{
			FormatEx(error, errlen, "@strafe %d %d goes past the last tick", range[0], range[1]);
			break;
		}
		TickInput ti;
		for (int t = range[0]; t < range[1]; t++)
		{
			inputs.GetArray(t, ti, sizeof(ti));
			ti.strafe = true;
			ti.strafeOffset = range[2];
			ti.strafeStraight = range[3];
			ti.strafeHeading = range[4];
			ti.strafeSwing = range[5];
			ti.strafeSmooth = range[6];
			ti.strafeClose = range[7];
			inputs.SetArray(t, ti, sizeof(ti));
		}
	}
	delete strafe;
	// the view pitch, when there is one, covers every tick
	bool hasView = view.Length > 0;
	if (error[0] == '\0' && hasView && view.Length != inputs.Length)
	{
		FormatEx(error, errlen, "@view_pitch has %d values for %d ticks", view.Length, inputs.Length);
	}
	for (int t = 0; t < inputs.Length && hasView && error[0] == '\0'; t++)
	{
		TickInput ti;
		inputs.GetArray(t, ti, sizeof(ti));
		ti.viewPitch = view.Get(t);
		inputs.SetArray(t, ti, sizeof(ti));
	}
	delete view;
	for (int i = 0; i < viewYaw.Length && error[0] == '\0'; i++)
	{
		any pair[2];
		viewYaw.GetArray(i, pair);
		int t = pair[0];
		if (t >= inputs.Length)
		{
			FormatEx(error, errlen, "@view_yaw names tick %d, past the last (%d)", t, inputs.Length - 1);
			break;
		}
		TickInput ti;
		inputs.GetArray(t, ti, sizeof(ti));
		ti.hasViewYaw = true;
		ti.viewYaw = pair[1];
		inputs.SetArray(t, ti, sizeof(ti));
	}
	delete viewYaw;
	if (error[0] != '\0')
	{
		delete inputs;
		delete names;
		delete values;
		return false;
	}

	delete g_Inputs;
	delete g_SettingNames;
	delete g_SettingValues;
	g_Inputs = inputs;
	g_SettingNames = names;
	g_SettingValues = values;
	g_Start = st;
	strcopy(g_BakedPath, sizeof(g_BakedPath), path);
	strcopy(g_BakedName, sizeof(g_BakedName), name);
	strcopy(g_BakedHash, sizeof(g_BakedHash), hash);
	strcopy(g_BakedMap, sizeof(g_BakedMap), map);
	g_HasViewPitch = hasView;
	g_AutoBhop = autobhop;
	g_HaveBaked = true;
	return true;
}
