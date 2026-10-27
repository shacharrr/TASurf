// Small helpers: output, parsing, names.

void TAS_Print(int client, const char[] fmt, any ...)
{
	char buffer[1024];
	VFormat(buffer, sizeof(buffer), fmt, 3);
	if (client > 0 && IsClientInGame(client))
	{
		PrintToConsole(client, "[TAS] %s", buffer);
	}
	else
	{
		PrintToServer("[TAS] %s", buffer);
	}
}

void TAS_Chat(int client, const char[] fmt, any ...)
{
	if (client <= 0 || !IsClientInGame(client))
	{
		return;
	}
	char buffer[256];
	VFormat(buffer, sizeof(buffer), fmt, 3);
	PrintToChat(client, "[TAS] %s", buffer);
}

void TAS_Log(const char[] fmt, any ...)
{
	char buffer[1024];
	VFormat(buffer, sizeof(buffer), fmt, 2);
	LogToFileEx(g_LogPath, "%s", buffer);
}

// The player a command acts on: the caller, or (from the server console) the
// first human in game.
int ResolveTarget(int client)
{
	if (client > 0)
	{
		return client;
	}
	for (int i = 1; i <= MaxClients; i++)
	{
		if (IsClientInGame(i) && !IsFakeClient(i))
		{
			return i;
		}
	}
	return 0;
}

bool ParseFloatStrict(const char[] s, float &out)
{
	int len = strlen(s);
	if (len == 0)
	{
		return false;
	}
	float value;
	if (StringToFloatEx(s, value) != len)
	{
		return false;
	}
	out = value;
	return true;
}

// Up to 8 hex digits -> 32-bit pattern. StringToInt(s, 16) clamps values
// above 0x7FFFFFFF on Windows, and float bit patterns use the sign bit.
bool ParseHex32(const char[] s, int &out)
{
	int len = strlen(s);
	if (len == 0 || len > 8)
	{
		return false;
	}
	int value = 0;
	for (int i = 0; i < len; i++)
	{
		int c = CharToLower(s[i]);
		int d;
		if (c >= '0' && c <= '9')
		{
			d = c - '0';
		}
		else if (c >= 'a' && c <= 'f')
		{
			d = c - 'a' + 10;
		}
		else
		{
			return false;
		}
		value = (value << 4) | d;
	}
	out = value;
	return true;
}

bool IsValidName(const char[] name)
{
	int len = strlen(name);
	if (len == 0 || len >= TAS_MAX_NAME)
	{
		return false;
	}
	for (int i = 0; i < len; i++)
	{
		int c = name[i];
		if (!IsCharAlpha(c) && !IsCharNumeric(c) && c != '_' && c != '-' && c != '.')
		{
			return false;
		}
	}
	return true;
}

void BaseName(const char[] path, char[] out, int maxlen)
{
	int start = 0;
	for (int i = 0; path[i] != '\0'; i++)
	{
		if (path[i] == '/' || path[i] == '\\')
		{
			start = i + 1;
		}
	}
	strcopy(out, maxlen, path[start]);
}

bool SameBits(float a, float b)
{
	return view_as<int>(a) == view_as<int>(b);
}

void SetPropIfDifferent(int entity, PropType type, const char[] prop, int value)
{
	if (HasEntProp(entity, type, prop) && GetEntProp(entity, type, prop) != value)
	{
		SetEntProp(entity, type, prop, value);
	}
}

void SetCvarIfDifferent(const char[] name, const char[] value)
{
	ConVar cv = FindConVar(name);
	if (cv == null)
	{
		return;
	}
	char current[64];
	cv.GetString(current, sizeof(current));
	if (!StrEqual(current, value, false))
	{
		cv.SetString(value);
	}
}

// Remove (and delete) every weapon. Without one a player runs at 260 (CS:S's own speed without a
// weapon, as on KSF); a weapon in hand caps it at the weapon's speed (knife and pistols 250), even
// though m_flMaxspeed still reads 260.
void Weapons_Strip(int client)
{
	for (int slot = 0; slot <= 5; slot++)
	{
		for (int guard = 0; guard < 8; guard++)  // grenades share a slot
		{
			int w = GetPlayerWeaponSlot(client, slot);
			if (w == -1 || !RemovePlayerItem(client, w))
			{
				break;
			}
			AcceptEntityInput(w, "Kill");
		}
	}
}

void EnsureDataDirs()
{
	char path[PLATFORM_MAX_PATH];
	BuildPath(Path_SM, path, sizeof(path), "%s", TAS_DATA_DIR);
	if (!DirExists(path))
	{
		CreateDirectory(path, 511);
	}
	BuildPath(Path_SM, path, sizeof(path), "%s/baked", TAS_DATA_DIR);
	if (!DirExists(path))
	{
		CreateDirectory(path, 511);
	}
	BuildPath(Path_SM, path, sizeof(path), "%s/starts", TAS_DATA_DIR);
	if (!DirExists(path))
	{
		CreateDirectory(path, 511);
	}
	BuildPath(Path_SM, path, sizeof(path), "%s/runs", TAS_DATA_DIR);
	if (!DirExists(path))
	{
		CreateDirectory(path, 511);
	}
	BuildPath(Path_SM, path, sizeof(path), "%s/requests", TAS_DATA_DIR);
	if (!DirExists(path))
	{
		CreateDirectory(path, 511);
	}
}
