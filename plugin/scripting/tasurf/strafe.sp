// Auto-strafe ticks (@strafe in the baked file): the yaw comes from the real velocity.
//
// A hand-made yaw curve can't know how a ramp bends the velocity, and a view that
// trails the velocity by asin(30/speed) gains nothing at all. On an auto-strafe tick
// the plugin aims the held keys from the velocity the engine has right now: the wish
// direction at the max-gain angle to it, acos(c*/speed), plus the range's offset, on
// the side the keys strafe to.
//
//   air:     c* = max(30 - A, 0), A = sv_airaccelerate * wishspeed * tick interval
//            (exactly sideways whenever A >= 30, as on surf)
//   ground:  friction first, as the engine does (speed -= max(speed, sv_stopspeed) *
//            sv_friction * tick interval), then c* = max(wishspeed - A, 0) with
//            A = sv_accelerate * wishspeed * tick interval: the prestrafe angle. Below
//            c* the best is straight along the velocity. A tick that jumps moves in
//            the air (the engine leaves the ground before it moves), so it aims as air.
//
// A straight range (a heading in the baked file) also picks the side each tick: A when the heading is to the
// left of the velocity, D otherwise (AutoStrafe_Side). With a swing it keeps the side it played the tick before
// until the direction is that many degrees past the heading: fewer, longer sweeps, smoother to watch. At the max-gain angle each tick turns the velocity by up
// to atan(30/speed) towards the heading, so it zigzags around it: straight, with the most speed. Same rule as
// straight_sidemove() in tasurf/physics.py.
//
// Only the input is chosen; position and velocity are still the engine's alone, and
// the same state gives the same yaw, so runs stay bit-identical. Same rule as
// strafe_yaw() in tasurf/physics.py. Without a movement key or without speed, the
// timeline's own yaw is played. Not modelled (the aim is then a little off, never
// wrong): a surface's own friction (taken as 1), the landing slowdown (stamina).

#define AIR_SPEED_CAP 30.0
#define DUCK_SPEED_MULTIPLIER 0.33333334  // ducked on the ground: wish speed cropped to a third

float StrafeCvar(ConVar &cv, const char[] name, float fallback)
{
	if (cv == null)
	{
		cv = FindConVar(name);
	}
	return cv != null ? cv.FloatValue : fallback;
}

bool AutoStrafe_Yaw(int client, int tick, TickInput ti, float &yaw)
{
	static ConVar cvAir, cvAccel, cvFriction, cvStop, cvMax;
	if (ti.forwardmove == 0.0 && ti.sidemove == 0.0)
	{
		return false;
	}
	float v[3];
	GetEntPropVector(client, Prop_Data, "m_vecAbsVelocity", v);
	float speed = SquareRoot(v[0] * v[0] + v[1] * v[1]);
	if (speed < 1.0)
	{
		return false;
	}
	TickInput prev;
	bool havePrev = tick > 0;
	if (havePrev)
	{
		g_Inputs.GetArray(tick - 1, prev, sizeof(prev));
	}
	int flags = GetEntityFlags(client);
	bool ground = (flags & FL_ONGROUND) != 0;
	if (ground && (ti.buttons & IN_JUMP) && !(havePrev && (g_LastButtons & IN_JUMP)))
	{
		ground = false;  // this tick jumps: it moves in the air (held from the tick before, as played: auto bhop lets go)
	}

	float maxspeed = GetEntPropFloat(client, Prop_Send, "m_flMaxspeed");
	float svMaxspeed = StrafeCvar(cvMax, "sv_maxspeed", 320.0);
	if (maxspeed <= 0.0 || svMaxspeed < maxspeed)
	{
		maxspeed = svMaxspeed;
	}
	float wishspeed = SquareRoot(ti.forwardmove * ti.forwardmove + ti.sidemove * ti.sidemove);
	if (maxspeed > 0.0 && wishspeed > maxspeed)
	{
		wishspeed = maxspeed;
	}
	float dt = GetTickInterval();
	float cap, accel;
	if (ground)
	{
		if (flags & FL_DUCKING)
		{
			wishspeed *= DUCK_SPEED_MULTIPLIER;
		}
		float stop = StrafeCvar(cvStop, "sv_stopspeed", 75.0);
		float control = speed < stop ? stop : speed;
		speed -= control * StrafeCvar(cvFriction, "sv_friction", 4.0) * dt;
		if (speed < 1.0)
		{
			return false;  // friction stops it: no direction to aim from
		}
		cap = wishspeed;
		accel = StrafeCvar(cvAccel, "sv_accelerate", 5.0) * wishspeed * dt;
	}
	else
	{
		cap = AIR_SPEED_CAP;
		accel = StrafeCvar(cvAir, "sv_airaccelerate", 10.0) * wishspeed * dt;
	}
	float cstar = cap - accel;
	if (cstar < 0.0)
	{
		cstar = 0.0;
	}
	float theta = cstar < speed ? RadToDeg(ArcCosine(cstar / speed)) : 0.0;
	theta += ti.strafeOffset;

	// wish direction relative to the view, left positive (A: +90, D: -90, W+A: +45)
	float alpha = RadToDeg(Atan2(-ti.sidemove, ti.forwardmove));
	float side = 0.0;
	if (alpha > 0.5 && alpha < 179.5)
	{
		side = 1.0;
	}
	else if (alpha < -0.5 && alpha > -179.5)
	{
		side = -1.0;
	}
	else if (havePrev)
	{
		// W or S alone: strafe to the side the timeline's own yaw turns
		float turn = WrapDegrees(ti.yaw - prev.yaw);
		side = turn > 0.0 ? 1.0 : (turn < 0.0 ? -1.0 : 0.0);
	}
	if (side == 0.0)
	{
		if (theta != 0.0)
		{
			return false;  // W or S alone, no turn in the timeline: no side to strafe to
		}
		side = 1.0;  // straight along the velocity: either side is the same
	}
	float velYaw = RadToDeg(Atan2(v[1], v[0]));
	yaw = WrapDegrees(velYaw + side * theta - alpha);
	return true;
}

// A straight auto-strafe tick: the held side key's sidemove, made A (negative) or D (positive) to steer towards
// the heading. False (keep the keys as held) without a side key or without speed.
bool AutoStrafe_Side(int client, TickInput ti, float &sidemove)
{
	if (!ti.strafeStraight || ti.sidemove == 0.0)
	{
		return false;
	}
	float v[3];
	GetEntPropVector(client, Prop_Data, "m_vecAbsVelocity", v);
	if (SquareRoot(v[0] * v[0] + v[1] * v[1]) < 1.0)
	{
		return false;
	}
	float err = WrapDegrees(ti.strafeHeading - RadToDeg(Atan2(v[1], v[0])));
	float size = ti.sidemove < 0.0 ? -ti.sidemove : ti.sidemove;
	if (ti.strafeSwing > 0.0 && g_LastSidemove != 0.0)
	{
		// the side played the tick before, until the direction is the swing past the heading on that side
		if (g_LastSidemove < 0.0)
		{
			sidemove = err < -ti.strafeSwing ? size : -size;
		}
		else
		{
			sidemove = err > ti.strafeSwing ? -size : size;
		}
		return true;
	}
	sidemove = err > 0.0 ? -size : size;
	return true;
}

// A smooth-swinging straight tick in the air: the direction sweeps the swing either side of the heading, its turn
// rate changing by at most a step a tick (no corners in the yaw): it speeds up to the most a tick turns,
// atan(30 / speed), brakes in time to stop turning at the edge of the sweep, and goes on through zero to turn the
// other way. The step is what a sine of that size needs at strafeClose 0 (the smoothest, about three quarters of the
// speed gain) and two whole turns at 1 (always the most turn: the most speed). Turning less than the most takes the
// wish direction short of square to the velocity: some W besides A or D. The view stays along the velocity. Same
// rule as smooth_swing_input() in tasurf/physics.py. False (play it as a straight tick) on the ground, without a
// side key or without speed.
bool AutoStrafe_Smooth(int client, TickInput ti, float &fmove, float &smove, float &yaw)
{
	if (!ti.strafeSmooth || ti.sidemove == 0.0 || (GetEntityFlags(client) & FL_ONGROUND))
	{
		return false;
	}
	float v[3];
	GetEntPropVector(client, Prop_Data, "m_vecAbsVelocity", v);
	float speed = SquareRoot(v[0] * v[0] + v[1] * v[1]);
	if (speed < 1.0)
	{
		return false;
	}
	float size = FloatAbs(ti.sidemove), swing = ti.strafeSwing;
	float dir = RadToDeg(Atan2(v[1], v[0]));
	float d = WrapDegrees(dir - ti.strafeHeading);  // left of the heading: positive
	float most = RadToDeg(ArcTangent(30.0 / speed));
	float sine = most * most / swing;
	if (sine > 2.0 * most)
	{
		sine = 2.0 * most;
	}
	float step = sine + ti.strafeClose * (2.0 * most - sine);  // up to two whole turns: one side's most to the other's
	float way = g_LastSidemove > 0.0 ? -1.0 : (g_LastSidemove < 0.0 ? 1.0 : (d < 0.0 ? 1.0 : -1.0));  // 1: turning left (A)
	float u = g_HavePrevStartYaw ? way * WrapDegrees(dir - g_PrevStartYaw) : 0.0;  // how fast, that way
	float ahead = swing - way * d;  // to the edge it's turning towards
	float faster = u + step < most ? u + step : most;
	float w;
	if (ahead >= faster + SmoothStop(faster, step))
	{
		w = faster;  // speed up: it can still stop in time
	}
	else if (u >= step && ahead >= u + SmoothStop(u, step))
	{
		w = u;  // hold a real turn
	}
	else
	{
		w = u - step;  // brake (a creep at the edge turns back)
	}
	w = w > most ? most : (w < -most ? -most : w);
	float turn = way * w;  // this tick's turn, positive left
	float cosT = (30.0 - speed * Tangent(DegToRad(FloatAbs(turn)))) / speed;
	cosT = cosT > 1.0 ? 1.0 : (cosT < 0.0 ? 0.0 : cosT);
	fmove = size * cosT;
	smove = (turn > 0.0 ? -size : size) * SquareRoot(1.0 - cosT * cosT);
	yaw = WrapDegrees(dir);
	return true;
}

// how far a smooth swing still turns braking from w, a step a tick
float SmoothStop(float w, float step)
{
	float n = w > 0.0 ? float(RoundToFloor(w / step)) : 0.0;
	return n * w - step * n * (n + 1.0) / 2.0;
}

// atan2(y, x). ArcTangent2's documentation doesn't say which argument is which, so ask it once.
float Atan2(float y, float x)
{
	static int order = 0;
	if (order == 0)
	{
		order = ArcTangent2(1.0, 0.0) > 1.0 ? 1 : -1;  // atan2(1, 0) is pi/2
	}
	return order == 1 ? ArcTangent2(y, x) : ArcTangent2(x, y);
}

// to (-180, 180], what a client sends
float WrapDegrees(float a)
{
	while (a > 180.0)
	{
		a -= 360.0;
	}
	while (a <= -180.0)
	{
		a += 360.0;
	}
	return a;
}
