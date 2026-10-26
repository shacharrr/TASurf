// Edit mode and record mode for the human player (tas_mode).
//
// Edit mode (the default, with the replay bot, tas_bot 1): you fly in noclip without
// weapons, can't take damage and don't block anyone. Runs and previews play on the replay
// bot (bot.sp), so you can watch from wherever you are. Record mode: you're a normal player
// again (walking, with a knife) for tas_record, e.g. a prestrafe or a rough live run to start
// a timeline from. Editor requests wait while you're in record mode.
//
// Nothing here touches the player a run plays on: the replay bot, or you while you record.

#define COLLISION_DEBRIS_TRIGGER 2  // collides with the world only, still touches triggers
#define DAMAGE_NO  0
#define DAMAGE_YES 2

int g_Mode = Mode_Edit;
int g_HumanCollision[MAXPLAYERS + 1];
float g_SpawnGrace[MAXPLAYERS + 1];     // until then, the weapons a spawn hands out are taken (and removed)
float g_EditPos[MAXPLAYERS + 1][3];
float g_EditAng[MAXPLAYERS + 1][3];
bool g_HaveEditPos[MAXPLAYERS + 1];
int g_FrozenUserId;                     // held still while a sped-up preview plays (preview.sp)
MoveType g_FrozenMoveType;

bool Mode_EditActive()
{
	return g_cvBot.BoolValue && g_Mode == Mode_Edit;
}

void Mode_Name(char[] out, int maxlen)
{
	if (!g_cvBot.BoolValue)
	{
		strcopy(out, maxlen, "off");
	}
	else
	{
		strcopy(out, maxlen, g_Mode == Mode_Edit ? "edit" : "record");
	}
}

void Mode_OnClientPutInServer(int client)
{
	g_HumanCollision[client] = 5;  // COLLISION_GROUP_PLAYER until we see the real one
	g_HaveEditPos[client] = false;
	g_SpawnGrace[client] = 0.0;
	if (!IsFakeClient(client))
	{
		SDKHook(client, SDKHook_WeaponCanUse, Hook_WeaponCanUse);
	}
}

void Mode_OnMapStart()
{
	for (int i = 1; i <= MaxClients; i++)
	{
		g_HaveEditPos[i] = false;
	}
	g_FrozenUserId = 0;
}

// In edit mode you don't pick weapons up while flying around (they stay where they are).
public Action Hook_WeaponCanUse(int client, int weapon)
{
	if (Mode_EditActive() && GetGameTime() > g_SpawnGrace[client])
	{
		return Plugin_Handled;
	}
	return Plugin_Continue;
}

bool Mode_IsFrozen(int client)
{
	return g_FrozenUserId != 0 && GetClientUserId(client) == g_FrozenUserId;
}

// Make the human match the mode. changed: the mode (or tas_bot) just changed, so also undo
// edit mode for record mode; otherwise record mode leaves you exactly as the game has you.
void Mode_Apply(int client, bool changed)
{
	if (client <= 0 || !IsClientInGame(client) || IsFakeClient(client) || !IsPlayerAlive(client))
	{
		return;
	}
	if (Mode_IsFrozen(client) || (g_RunState != Run_Idle && g_RunClient == client))
	{
		return;  // a preview holds you still, or the run is playing on you
	}
	if (Mode_EditActive())
	{
		if (GetEntityMoveType(client) != MOVETYPE_NOCLIP)
		{
			SetEntityMoveType(client, MOVETYPE_NOCLIP);
		}
		Weapons_Strip(client, false);
		SetPropIfDifferent(client, Prop_Data, "m_takedamage", DAMAGE_NO);
		int group = GetEntProp(client, Prop_Send, "m_CollisionGroup");
		if (group != COLLISION_DEBRIS_TRIGGER)
		{
			g_HumanCollision[client] = group;
			SetEntProp(client, Prop_Send, "m_CollisionGroup", COLLISION_DEBRIS_TRIGGER);
		}
	}
	else if (changed)
	{
		if (GetEntityMoveType(client) == MOVETYPE_NOCLIP)
		{
			float still[3];
			SetEntityMoveType(client, MOVETYPE_WALK);
			TeleportEntity(client, NULL_VECTOR, NULL_VECTOR, still);
		}
		SetPropIfDifferent(client, Prop_Data, "m_takedamage", DAMAGE_YES);
		if (GetEntProp(client, Prop_Send, "m_CollisionGroup") == COLLISION_DEBRIS_TRIGGER)
		{
			SetEntProp(client, Prop_Send, "m_CollisionGroup", g_HumanCollision[client]);
		}
		if (GetEntPropEnt(client, Prop_Send, "m_hActiveWeapon") == -1)
		{
			Weapons_KnifeOnly(client);
		}
	}
}

void Mode_ApplyAll(bool changed)
{
	for (int i = 1; i <= MaxClients; i++)
	{
		if (IsClientInGame(i) && !IsFakeClient(i))
		{
			Mode_Apply(i, changed);
		}
	}
}

// Every 0.25 s (Timer_Remote): keep edit mode in force, and remember where you are, so a
// round restart (which respawns everyone) puts you back where you were editing.
void Mode_Tick(int human)
{
	if (!Mode_EditActive() || human <= 0 || !IsPlayerAlive(human))
	{
		return;
	}
	Mode_Apply(human, false);
	if (!Mode_IsFrozen(human))
	{
		GetClientAbsOrigin(human, g_EditPos[human]);
		GetClientEyeAngles(human, g_EditAng[human]);
		g_EditAng[human][2] = 0.0;
		g_HaveEditPos[human] = true;
	}
}

void Mode_OnSpawn(int client)
{
	g_SpawnGrace[client] = GetGameTime() + 0.5;
	CreateTimer(0.1, Timer_ModeSpawn, GetClientUserId(client), TIMER_FLAG_NO_MAPCHANGE);
}

public Action Timer_ModeSpawn(Handle timer, int userid)
{
	int client = GetClientOfUserId(userid);
	if (client <= 0 || !IsClientInGame(client) || !IsPlayerAlive(client) || !Mode_EditActive())
	{
		return Plugin_Stop;
	}
	Mode_Apply(client, false);
	if (g_HaveEditPos[client])
	{
		float still[3];
		TeleportEntity(client, g_EditPos[client], g_EditAng[client], still);
	}
	return Plugin_Stop;
}

void Mode_Freeze(int client)
{
	if (client <= 0 || !IsClientInGame(client) || !IsPlayerAlive(client) || g_FrozenUserId != 0)
	{
		return;
	}
	g_FrozenUserId = GetClientUserId(client);
	g_FrozenMoveType = GetEntityMoveType(client);
	float still[3];
	SetEntityMoveType(client, MOVETYPE_NONE);
	TeleportEntity(client, NULL_VECTOR, NULL_VECTOR, still);
}

void Mode_Unfreeze()
{
	int client = GetClientOfUserId(g_FrozenUserId);
	g_FrozenUserId = 0;
	if (client > 0 && IsClientInGame(client) && IsPlayerAlive(client) && GetEntityMoveType(client) == MOVETYPE_NONE)
	{
		SetEntityMoveType(client, g_FrozenMoveType);
	}
}

void Mode_Set(int mode, int human)
{
	g_Mode = mode;
	Mode_ApplyAll(true);
	if (mode == Mode_Edit)
	{
		TAS_Print(human, "edit mode: noclip, no weapons. tas_run, tas_preview and the editor play on the replay bot; tas_mode record to play yourself");
		TAS_Chat(human, "edit mode (noclip): runs play on the replay bot");
	}
	else
	{
		TAS_Print(human, "record mode: you walk and hold a knife. tas_record starts recording where you are (again to save); tas_mode edit goes back to noclip");
		TAS_Print(human, "  editor previews wait while you're in record mode; tas_run still plays on the replay bot");
		TAS_Chat(human, "record mode: tas_record to start recording, tas_mode edit for noclip");
	}
}

public Action Cmd_Mode(int client, int args)
{
	int human = ResolveTarget(client);
	if (!g_cvBot.BoolValue)
	{
		TAS_Print(client, "edit mode needs the replay bot (tas_bot 1). With tas_bot 0 you play normally and runs play on you.");
		return Plugin_Handled;
	}
	int want = g_Mode == Mode_Edit ? Mode_Record : Mode_Edit;
	if (args >= 1)
	{
		char arg[16];
		GetCmdArg(1, arg, sizeof(arg));
		if (StrEqual(arg, "edit", false))
		{
			want = Mode_Edit;
		}
		else if (StrEqual(arg, "record", false))
		{
			want = Mode_Record;
		}
		else
		{
			TAS_Print(client, "usage: tas_mode [edit|record]   (no argument switches between them)");
			return Plugin_Handled;
		}
	}
	if (g_Recording && want == Mode_Edit)
	{
		TAS_Print(client, "you're recording: tas_record saves the recording first");
		return Plugin_Handled;
	}
	Mode_Set(want, human);
	return Plugin_Handled;
}

public void OnBotConVarChanged(ConVar convar, const char[] oldValue, const char[] newValue)
{
	Mode_ApplyAll(true);
}
