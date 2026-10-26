// Start states: capture (tas_setstart), JSON output, and the reset.
//
// Start_Apply is the ONLY place the plugin writes position or velocity, and
// it is only called before tick 0 (settle ticks and the tick-0 reset).

void SetPropIntIfPresent(int client, PropType type, const char[] prop, int value)
{
	if (HasEntProp(client, type, prop))
	{
		SetEntProp(client, type, prop, value);
	}
}

void SetPropFloatIfPresent(int client, PropType type, const char[] prop, float value)
{
	if (HasEntProp(client, type, prop))
	{
		SetEntPropFloat(client, type, prop, value);
	}
}

// settle: true for the reset ticks before tick 0, false for the final reset
// at tick 0. Only the ground state treats them differently (Ground_Auto).
void Start_Apply(int client, StartState st, bool settle)
{
	SetEntityMoveType(client, MOVETYPE_WALK);

	// Always reset to fully standing, whatever the previous run ended in.
	SetPropIntIfPresent(client, Prop_Send, "m_bDucked", 0);
	SetPropIntIfPresent(client, Prop_Send, "m_bDucking", 0);
	SetPropIntIfPresent(client, Prop_Send, "m_bInDuckJump", 0);
	SetPropFloatIfPresent(client, Prop_Send, "m_flDucktime", 0.0);
	SetPropFloatIfPresent(client, Prop_Send, "m_flDuckJumpTime", 0.0);
	SetPropFloatIfPresent(client, Prop_Send, "m_flJumpTime", 0.0);
	SetPropFloatIfPresent(client, Prop_Send, "m_flFallVelocity", 0.0);
	if (st.hasViewOffsetZ)
	{
		SetPropFloatIfPresent(client, Prop_Send, "m_vecViewOffset[2]", st.viewOffsetZ);
	}

	TeleportEntity(client, st.origin, st.angles, st.velocity);

	// After the teleport: teleporting with a velocity clears base velocity.
	SetEntPropVector(client, Prop_Data, "m_vecBaseVelocity", st.baseVelocity);
	SetEntityGravity(client, st.gravity);
	SetEntPropFloat(client, Prop_Data, "m_flLaggedMovementValue", st.laggedMovement);
	// CS:S slowdowns: stamina (after a jump or landing, scales ground speed for
	// ~1.3 s) and the velocity modifier (after damage). Always reset, defaulting
	// to "rested" (0 and 1) when the start state doesn't give them.
	SetPropFloatIfPresent(client, Prop_Send, "m_flVelocityModifier", st.velocityModifier);
	SetPropFloatIfPresent(client, Prop_Send, "m_flStamina", st.stamina);
	if (st.hasHealth)
	{
		SetEntityHealth(client, st.health);
	}

	int flags = GetEntityFlags(client) & ~(FL_DUCKING | FL_BASEVELOCITY);
	if (st.baseVelocityFlag)
	{
		flags |= FL_BASEVELOCITY;
	}

	// Ground state. Ground_Auto: airborne for the settle tick, then keep
	// whatever the engine's own ground check found at the end of it.
	int ground = -2;  // -2: leave as is
	if (st.onGround == Ground_Yes)
	{
		ground = IsValidEntity(st.groundEntity) ? st.groundEntity : 0;
	}
	else if (st.onGround == Ground_No || settle)
	{
		ground = -1;
	}
	if (ground != -2)
	{
		if (ground >= 0)
		{
			flags |= FL_ONGROUND;
		}
		else
		{
			flags &= ~FL_ONGROUND;
		}
		if (GetEntPropEnt(client, Prop_Data, "m_hGroundEntity") != ground)
		{
			SetEntPropEnt(client, Prop_Data, "m_hGroundEntity", ground);
		}
	}
	SetEntityFlags(client, flags);
}

// Capture the player's current state. Returns false (with a reason) if the
// state can't be used as a start.
bool Start_Capture(int client, StartState st, bool still, char[] note, int notelen)
{
	note[0] = '\0';
	if (GetEntityFlags(client) & FL_DUCKING)
	{
		strcopy(note, notelen, "you are ducked; ducked start states aren't supported yet. Stand up and try again.");
		return false;
	}

	GetClientAbsOrigin(client, st.origin);
	GetClientEyeAngles(client, st.angles);
	st.angles[2] = 0.0;
	GetEntPropVector(client, Prop_Data, "m_vecAbsVelocity", st.velocity);
	GetEntPropVector(client, Prop_Data, "m_vecBaseVelocity", st.baseVelocity);
	st.baseVelocityFlag = (GetEntityFlags(client) & FL_BASEVELOCITY) != 0;

	int ground = GetEntPropEnt(client, Prop_Data, "m_hGroundEntity");
	st.onGround = ground >= 0 ? Ground_Yes : Ground_No;
	st.groundEntity = ground >= 0 ? ground : 0;

	if (GetEntityMoveType(client) == MOVETYPE_NOCLIP)
	{
		still = true;
		st.onGround = Ground_Auto;
		strcopy(note, notelen, "captured in noclip: velocity set to 0 and ground state left to the engine (auto)");
	}
	if (still)
	{
		st.velocity[0] = 0.0;
		st.velocity[1] = 0.0;
		st.velocity[2] = 0.0;
		st.baseVelocity[0] = 0.0;
		st.baseVelocity[1] = 0.0;
		st.baseVelocity[2] = 0.0;
		st.baseVelocityFlag = false;
	}

	st.hasViewOffsetZ = HasEntProp(client, Prop_Send, "m_vecViewOffset[2]");
	if (st.hasViewOffsetZ)
	{
		st.viewOffsetZ = GetEntPropFloat(client, Prop_Send, "m_vecViewOffset[2]");
	}
	st.gravity = GetEntityGravity(client);
	st.laggedMovement = GetEntPropFloat(client, Prop_Data, "m_flLaggedMovementValue");
	st.hasVelocityModifier = HasEntProp(client, Prop_Send, "m_flVelocityModifier");
	if (st.hasVelocityModifier)
	{
		st.velocityModifier = GetEntPropFloat(client, Prop_Send, "m_flVelocityModifier");
	}
	st.hasStamina = HasEntProp(client, Prop_Send, "m_flStamina");
	if (st.hasStamina)
	{
		st.stamina = GetEntPropFloat(client, Prop_Send, "m_flStamina");
	}
	st.hasHealth = true;
	st.health = GetClientHealth(client);
	// Without a weapon in hand (edit mode) your maxspeed isn't a player's, so it isn't
	// captured: the replay bot holds a knife (250, the same as a pistol).
	st.hasMaxspeed = GetEntPropEnt(client, Prop_Send, "m_hActiveWeapon") != -1;
	st.maxspeed = st.hasMaxspeed ? GetEntPropFloat(client, Prop_Send, "m_flMaxspeed") : 0.0;
	if (!st.hasMaxspeed && note[0] == '\0')
	{
		strcopy(note, notelen, "no weapon in hand: maxspeed not captured (the replay bot holds a knife)");
	}
	return true;
}

void Json_Vec(File f, const char[] key, const float v[3])
{
	f.WriteLine("  \"%s\": [%.6f, %.6f, %.6f],", key, v[0], v[1], v[2]);
}

void Json_Bits(File f, const char[] key, const float v[3])
{
	f.WriteLine("  \"%s\": [\"%08X\", \"%08X\", \"%08X\"],", key, view_as<int>(v[0]), view_as<int>(v[1]), view_as<int>(v[2]));
}

void Json_Bool(File f, const char[] key, bool value)
{
	if (value)
	{
		f.WriteLine("  \"%s\": true,", key);
	}
	else
	{
		f.WriteLine("  \"%s\": false,", key);
	}
}

// Written in exactly the timeline's "start" format (docs/timeline-format.md).
bool Start_WriteJson(const char[] path, StartState st, const char[] map)
{
	File f = OpenFile(path, "w");
	if (f == null)
	{
		return false;
	}
	f.WriteLine("{");
	f.WriteLine("  \"format\": \"tasurf-start\",");
	f.WriteLine("  \"version\": 1,");
	f.WriteLine("  \"map\": \"%s\",", map);
	Json_Vec(f, "origin", st.origin);
	Json_Bits(f, "origin_bits", st.origin);
	Json_Vec(f, "angles", st.angles);
	Json_Bits(f, "angles_bits", st.angles);
	Json_Vec(f, "velocity", st.velocity);
	Json_Bits(f, "velocity_bits", st.velocity);
	Json_Vec(f, "base_velocity", st.baseVelocity);
	Json_Bool(f, "base_velocity_flag", st.baseVelocityFlag);
	if (st.onGround == Ground_Auto)
	{
		f.WriteLine("  \"on_ground\": \"auto\",");
	}
	else
	{
		Json_Bool(f, "on_ground", st.onGround == Ground_Yes);
	}
	f.WriteLine("  \"ground_entity\": %d,", st.groundEntity);
	f.WriteLine("  \"ducked\": false,");
	if (st.hasViewOffsetZ)
	{
		f.WriteLine("  \"view_offset_z\": %.6f,", st.viewOffsetZ);
	}
	f.WriteLine("  \"gravity\": %.6f,", st.gravity);
	f.WriteLine("  \"lagged_movement\": %.6f,", st.laggedMovement);
	if (st.hasVelocityModifier)
	{
		f.WriteLine("  \"velocity_modifier\": %.6f,", st.velocityModifier);
	}
	if (st.hasStamina)
	{
		f.WriteLine("  \"stamina\": %.6f,", st.stamina);
	}
	if (st.hasMaxspeed)
	{
		f.WriteLine("  \"maxspeed\": %.6f,", st.maxspeed);
	}
	f.WriteLine("  \"health\": %d", st.health);
	f.WriteLine("}");
	delete f;
	return true;
}

// "# start.*" header lines for the run CSV.
void Start_WriteHeader(File f, StartState st)
{
	f.WriteLine("# start.origin %.6f %.6f %.6f", st.origin[0], st.origin[1], st.origin[2]);
	f.WriteLine("# start.origin_bits %08X %08X %08X", view_as<int>(st.origin[0]), view_as<int>(st.origin[1]), view_as<int>(st.origin[2]));
	f.WriteLine("# start.angles %.6f %.6f %.6f", st.angles[0], st.angles[1], st.angles[2]);
	f.WriteLine("# start.velocity %.6f %.6f %.6f", st.velocity[0], st.velocity[1], st.velocity[2]);
	f.WriteLine("# start.velocity_bits %08X %08X %08X", view_as<int>(st.velocity[0]), view_as<int>(st.velocity[1]), view_as<int>(st.velocity[2]));
	f.WriteLine("# start.base_velocity %.6f %.6f %.6f", st.baseVelocity[0], st.baseVelocity[1], st.baseVelocity[2]);
	if (st.onGround == Ground_Yes)
	{
		f.WriteLine("# start.on_ground yes %d", st.groundEntity);
	}
	else if (st.onGround == Ground_No)
	{
		f.WriteLine("# start.on_ground no");
	}
	else
	{
		f.WriteLine("# start.on_ground auto");
	}
	f.WriteLine("# start.gravity %.6f", st.gravity);
	f.WriteLine("# start.lagged_movement %.6f", st.laggedMovement);
}
