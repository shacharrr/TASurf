// Auto bhop: holding jump jumps again the moment you land, as on surf servers. CS:S only jumps when jump
// wasn't held on the tick before (CGameMovement::CheckJumpButton), so a held jump is let go in the air and
// pressed again on landing: the tick that starts on the ground jumps, before any friction.
//
// While you play (between runs, and your own turn in tas_record) tas_autobhop decides; a timeline's run and
// preview use the timeline's own setting ("autobhop": true, baked as @autobhop), so a run doesn't depend on
// the server. Recordings store the buttons as played (let go in the air), so they replay the same with it
// on or off. Not in water (jump swims up), on a ladder (jump lets go of it) or in noclip.

ConVar g_cvAutoBhop;

void AutoBhop_Init()
{
	g_cvAutoBhop = CreateConVar("tas_autobhop", "1",
		"1: while you play (not in a timeline's run: that's the timeline's own setting), holding jump jumps again as you land. 0: let go and press again, as plain CS:S.",
		_, true, 0.0, true, 1.0);
}

// Jump held in the air: let it go, so the tick that lands with it held jumps. True when it changed buttons.
bool AutoBhop_Apply(int client, int &buttons)
{
	if (!(buttons & IN_JUMP) || (GetEntityFlags(client) & FL_ONGROUND))
	{
		return false;
	}
	MoveType mt = GetEntityMoveType(client);
	if (mt == MOVETYPE_LADDER || mt == MOVETYPE_NOCLIP || GetEntProp(client, Prop_Data, "m_nWaterLevel") >= 2)
	{
		return false;
	}
	buttons &= ~IN_JUMP;
	return true;
}
