// The replay bot: a CS:S bot that plays runs and previews, so you stay where you are.
//
// CS:S bots run one user command per tick through the same movement code as human
// players. OnPlayerRunCmd replaces every field of the bot's commands (its own AI never
// moves it), and a run resets the bot exactly like a player (Start_Apply). Between runs the
// bot is frozen where it is (MOVETYPE_NONE), doesn't block anyone, can't take damage, and
// holds only a knife (maxspeed 250, the same as a player with a knife or pistol). For the
// run it gets its normal collision and damage back, like the player it stands in for.
//
// CS:S won't simply add a bot on a map without a navigation file: it starts generating one,
// which can take minutes and reloads the map. So on such a map TASurf writes a minimal
// maps/<map>.nav (one area, no paths; the bot never navigates) and reloads the map once.

#define NAV_MAGIC -17958194  // 0xFEEDFACE

int g_BotUserId;
float g_BotAddedAt = -100.0;
bool g_NavMissing;
int g_BotCollision = 5;     // the bot's own collision group (COLLISION_GROUP_PLAYER)
char g_BotStatus[32] = "none";

int Bot_Get()
{
	if (g_BotUserId == 0)
	{
		return 0;
	}
	int client = GetClientOfUserId(g_BotUserId);
	return (client > 0 && IsClientInGame(client) && IsFakeClient(client)) ? client : 0;
}

bool Bot_IsCandidate(int client)
{
	return IsClientInGame(client) && IsFakeClient(client) && !IsClientSourceTV(client) && !IsClientReplay(client);
}

void Bot_Adopt(int client)
{
	g_BotUserId = GetClientUserId(client);
	char name[MAX_NAME_LENGTH];
	g_cvBotName.GetString(name, sizeof(name));
	if (name[0] != '\0')
	{
		SetClientName(client, name);
	}
	int group = GetEntProp(client, Prop_Send, "m_CollisionGroup");
	if (group != COLLISION_DEBRIS_TRIGGER)
	{
		g_BotCollision = group;
	}
	LogMessage("TASurf replay bot: client %d", client);
}

// The bot's AI is switched off as far as CS:S allows; whatever is left of it can't move the
// bot anyway, because OnPlayerRunCmd overwrites its commands.
void Bot_ForceCvars()
{
	SetCvarIfDifferent("bot_quota_mode", "normal");
	SetCvarIfDifferent("bot_join_after_player", "0");
	SetCvarIfDifferent("bot_chatter", "off");
	SetCvarIfDifferent("bot_stop", "1");
	SetCvarIfDifferent("bot_zombie", "1");
	SetCvarIfDifferent("bot_controllable", "0");  // spectators can't take the bot over
}

// Between runs: frozen where it is, out of everyone's way, knife only.
void Bot_Idle(int bot)
{
	if (GetEntityMoveType(bot) != MOVETYPE_NONE)
	{
		float still[3];
		SetEntityMoveType(bot, MOVETYPE_NONE);
		TeleportEntity(bot, NULL_VECTOR, NULL_VECTOR, still);
	}
	SetPropIfDifferent(bot, Prop_Data, "m_takedamage", DAMAGE_NO);
	SetPropIfDifferent(bot, Prop_Send, "m_CollisionGroup", COLLISION_DEBRIS_TRIGGER);
	Weapons_KnifeOnly(bot);
}

// Right before a run: what a player has (Start_Apply then resets movement and position).
void Bot_PrepareRun(int bot)
{
	Weapons_KnifeOnly(bot);
	SetPropIfDifferent(bot, Prop_Data, "m_takedamage", DAMAGE_YES);
	SetPropIfDifferent(bot, Prop_Send, "m_CollisionGroup", g_BotCollision);
}

// Every 0.25 s (Timer_Remote): keep one replay bot in the game, on your team (so there are
// no round-win conditions between you), alive, and idle between runs.
void Bot_Maintain(int human)
{
	if (!g_cvBot.BoolValue)
	{
		strcopy(g_BotStatus, sizeof(g_BotStatus), "off");
		return;
	}
	if (g_NavMissing)
	{
		strcopy(g_BotStatus, sizeof(g_BotStatus), "reload-map");
		return;
	}
	int bot = Bot_Get();
	if (bot == 0)
	{
		// one that's already here: after a plugin reload, or one the server added
		for (int i = 1; i <= MaxClients && bot == 0; i++)
		{
			if (Bot_IsCandidate(i))
			{
				Bot_Adopt(i);
				bot = i;
			}
		}
	}
	if (bot == 0)
	{
		strcopy(g_BotStatus, sizeof(g_BotStatus), "none");
		// After you've picked a team, so it joins yours: a bot alone on the other team would
		// make CS:S restart the round ("Game Commencing") as soon as you join.
		if (human <= 0 || GetClientTeam(human) < CS_TEAM_T || GetEngineTime() - g_BotAddedAt < 5.0)
		{
			return;
		}
		Bot_ForceCvars();
		ServerCommand(GetClientTeam(human) == CS_TEAM_CT ? "bot_add_ct" : "bot_add_t");
		g_BotAddedAt = GetEngineTime();
		strcopy(g_BotStatus, sizeof(g_BotStatus), "adding");
		return;
	}
	if (g_RunState != Run_Idle && g_RunClient == bot)
	{
		strcopy(g_BotStatus, sizeof(g_BotStatus), "ready");
		return;
	}
	int team = GetClientTeam(bot);
	int want = (human > 0 && GetClientTeam(human) >= CS_TEAM_T) ? GetClientTeam(human) : (team >= CS_TEAM_T ? team : CS_TEAM_T);
	if (team != want)
	{
		CS_SwitchTeam(bot, want);
		CS_RespawnPlayer(bot);
		strcopy(g_BotStatus, sizeof(g_BotStatus), "dead");
		return;
	}
	if (!IsPlayerAlive(bot))
	{
		CS_RespawnPlayer(bot);
		strcopy(g_BotStatus, sizeof(g_BotStatus), "dead");
		return;
	}
	if (GetEngineTime() - g_BotAddedAt > 2.0)
	{
		Bot_ForceCvars();  // turning sv_cheats off (after a preview) resets cheat convars
	}
	Bot_Idle(bot);
	strcopy(g_BotStatus, sizeof(g_BotStatus), "ready");
}

// Who plays runs and previews: the replay bot, or (tas_bot 0) you.
int Runner_Get(int human, char[] why, int whylen)
{
	why[0] = '\0';
	if (!g_cvBot.BoolValue)
	{
		if (human <= 0)
		{
			strcopy(why, whylen, "no player in the game yet");
			return 0;
		}
		if (!IsPlayerAlive(human))
		{
			strcopy(why, whylen, GetClientTeam(human) <= 1 ? "you're spectating: join a team" : "you're dead: respawn");
			return 0;
		}
		return human;
	}
	if (g_NavMissing)
	{
		strcopy(why, whylen, "the map is being reloaded once so the replay bot can join (it had no .nav file)");
		return 0;
	}
	int bot = Bot_Get();
	if (bot == 0)
	{
		if (human <= 0 || GetClientTeam(human) < CS_TEAM_T)
		{
			strcopy(why, whylen, "join a team: the replay bot joins your team after you");
		}
		else
		{
			strcopy(why, whylen, "the replay bot is joining (tas_status in the game says more)");
		}
		return 0;
	}
	if (!IsPlayerAlive(bot))
	{
		strcopy(why, whylen, "the replay bot is respawning");
		return 0;
	}
	return bot;
}

void Bot_OnMapStart()
{
	g_BotUserId = 0;
	g_BotAddedAt = -100.0;
	g_NavMissing = false;
	if (!g_cvBot.BoolValue)
	{
		return;
	}
	char map[PLATFORM_MAX_PATH], nav[PLATFORM_MAX_PATH];
	GetCurrentMap(map, sizeof(map));
	FormatEx(nav, sizeof(nav), "maps/%s.nav", map);
	if (FileExists(nav, true))
	{
		return;
	}
	g_NavMissing = true;
	if (Bot_WriteBlankNav(map, nav))
	{
		LogMessage("%s had no bot navigation file: wrote a minimal %s for the replay bot and reloading the map", map, nav);
		PrintToServer("[TAS] %s has no bot navigation file. Wrote a minimal %s so the replay bot can join; reloading the map once.", map, nav);
		CreateTimer(2.0, Timer_ReloadMap, _, TIMER_FLAG_NO_MAPCHANGE);
	}
	else
	{
		PrintToServer("[TAS] couldn't write %s: the replay bot can't join on this map. tas_bot 0 plays runs on you instead.", nav);
	}
}

public Action Timer_ReloadMap(Handle timer)
{
	char map[PLATFORM_MAX_PATH];
	GetCurrentMap(map, sizeof(map));
	ForceChangeLevel(map, "TASurf: load the replay bot's navigation file");
	return Plugin_Stop;
}

void Bot_OnMapEnd()
{
	// No bots by quota on the next map: if it has no .nav, a quota bot would make CS:S
	// start generating one. TASurf adds its bot itself once the map is ready.
	if (g_cvBot.BoolValue)
	{
		SetCvarIfDifferent("bot_quota", "0");
	}
}

void NavFloat(File f, float v)
{
	f.WriteInt32(view_as<int>(v));
}

// A version 16 (sub-version 1, CS:S) navigation file with one 64x64 area and nothing else.
bool Bot_WriteBlankNav(const char[] map, const char[] nav)
{
	char bsp[PLATFORM_MAX_PATH];
	FormatEx(bsp, sizeof(bsp), "maps/%s.bsp", map);
	int bspSize = FileSize(bsp, true);
	File f = OpenFile(nav, "wb");
	if (f == null)
	{
		return false;
	}
	f.WriteInt32(NAV_MAGIC);
	f.WriteInt32(16);                         // version
	f.WriteInt32(1);                          // sub-version: Counter-Strike: Source
	f.WriteInt32(bspSize > 0 ? bspSize : 0);  // size of the .bsp it belongs to
	f.WriteInt8(1);                           // analyzed
	f.WriteInt16(0);                          // place names: none
	f.WriteInt8(0);                           // has unnamed areas
	f.WriteInt32(1);                          // areas
	f.WriteInt32(1);                          //   id
	f.WriteInt32(0);                          //   attribute flags
	NavFloat(f, -32.0); NavFloat(f, -32.0); NavFloat(f, 0.0);  // north-west corner
	NavFloat(f, 32.0); NavFloat(f, 32.0); NavFloat(f, 0.0);    // south-east corner
	NavFloat(f, 0.0); NavFloat(f, 0.0);       //   heights of the other two corners
	for (int d = 0; d < 4; d++)
	{
		f.WriteInt32(0);                      //   connections in each direction
	}
	f.WriteInt8(0);                           //   hiding spots
	f.WriteInt32(0);                          //   encounter paths
	f.WriteInt16(0);                          //   place
	f.WriteInt32(0);                          //   ladders up
	f.WriteInt32(0);                          //   ladders down
	NavFloat(f, 0.0); NavFloat(f, 0.0);       //   earliest occupy time per team
	for (int c = 0; c < 4; c++)
	{
		NavFloat(f, 1.0);                     //   light intensity per corner
	}
	f.WriteInt32(0);                          //   visible areas
	f.WriteInt32(0);                          //   inherit visibility from
	f.WriteInt8(0);                           //   CS:S approach areas
	f.WriteInt32(0);                          // ladders
	f.WriteInt32(0);                          // padding
	delete f;
	return true;
}
