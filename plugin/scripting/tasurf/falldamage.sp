// No fall damage, as on surf servers. CS:S takes health for landing faster than about 580 u/s (say at the
// bottom of a ramp), and taking damage starts its damage slowdown. While tas_fall_damage is 0 (the default)
// fall damage is blocked for everyone, you and the replay bot, before the game applies any of it. Every run
// records the value in its CSV header (g_RecordedCvars), and a timeline can expect it in "settings".

ConVar g_cvFallDamage;

void FallDamage_Init()
{
	g_cvFallDamage = CreateConVar("tas_fall_damage", "0",
		"0: no fall damage, as on surf servers (a hard landing takes no health and doesn't slow you). 1: CS:S's own fall damage.",
		_, true, 0.0, true, 1.0);
}

void FallDamage_OnClientPutInServer(int client)
{
	SDKHook(client, SDKHook_OnTakeDamage, Hook_FallDamage);
}

public Action Hook_FallDamage(int victim, int &attacker, int &inflictor, float &damage, int &damagetype)
{
	if ((damagetype & DMG_FALL) && !g_cvFallDamage.BoolValue)
	{
		return Plugin_Handled;
	}
	return Plugin_Continue;
}
