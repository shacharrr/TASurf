// Keeps the server from kicking you while you work: CS:S kicks idle players (and team killers) when
// mp_autokick is on, which is the default, and sitting in the editor through a few rounds counts as idle.
// While tas_no_kick is 1 (the default) mp_autokick is held at 0, whatever a cfg sets; the value it had
// comes back with tas_no_kick 0 or when the plugin unloads.

ConVar g_cvNoKick;
ConVar g_cvAutoKick;
char g_AutoKickWas[16];   // mp_autokick before we held it at 0 ("" while we don't)
bool g_NoKickSetting;     // our own change, not a cfg's

void NoKick_Init()
{
	g_cvNoKick = CreateConVar("tas_no_kick", "1",
		"1: keep the server from kicking players (mp_autokick 0: no idle or team-kill kicks) while TASurf is loaded. 0: the server's own setting.",
		_, true, 0.0, true, 1.0);
	g_cvNoKick.AddChangeHook(OnNoKickChanged);
	g_cvAutoKick = FindConVar("mp_autokick");
	if (g_cvAutoKick != null)
	{
		g_cvAutoKick.AddChangeHook(OnAutoKickChanged);
	}
	NoKick_Apply();
}

void NoKick_Apply()
{
	if (g_cvAutoKick == null)
	{
		return;
	}
	if (g_cvNoKick.BoolValue)
	{
		if (g_AutoKickWas[0] == '\0')
		{
			g_cvAutoKick.GetString(g_AutoKickWas, sizeof(g_AutoKickWas));
		}
		if (g_cvAutoKick.BoolValue)
		{
			g_NoKickSetting = true;
			g_cvAutoKick.SetInt(0);
			g_NoKickSetting = false;
		}
	}
	else
	{
		NoKick_Restore();
	}
}

void NoKick_Restore()
{
	if (g_cvAutoKick != null && g_AutoKickWas[0] != '\0')
	{
		g_NoKickSetting = true;
		g_cvAutoKick.SetString(g_AutoKickWas);
		g_NoKickSetting = false;
		g_AutoKickWas[0] = '\0';
	}
}

public void OnNoKickChanged(ConVar cv, const char[] oldValue, const char[] newValue)
{
	NoKick_Apply();
}

public void OnAutoKickChanged(ConVar cv, const char[] oldValue, const char[] newValue)
{
	if (g_NoKickSetting || !g_cvNoKick.BoolValue)
	{
		return;
	}
	strcopy(g_AutoKickWas, sizeof(g_AutoKickWas), newValue);  // what a cfg wants: back to it when we let go
	NoKick_Apply();
}
