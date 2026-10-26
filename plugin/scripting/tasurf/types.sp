// Shared types and constants. Included first by tasurf.sp.

#define TAS_DATA_DIR "data/tasurf"
#define TAS_MAX_NAME 64
#define TAS_HASH_LEN 32

enum
{
	Ground_Auto = 0,  // settle tick decides (the start origin's real ground state)
	Ground_Yes,       // on the ground entity in groundEntity
	Ground_No         // airborne
};

enum
{
	Run_Idle = 0,
	Run_Settling,     // reset ticks before tick 0 (inputs neutral, start state re-applied every tick)
	Run_Playing
};

// Everything the reset writes. Position and velocity are only ever written
// from here, and only before tick 0.
enum struct StartState
{
	float origin[3];
	float angles[3];
	float velocity[3];
	float baseVelocity[3];
	bool baseVelocityFlag;
	int onGround;
	int groundEntity;
	bool hasViewOffsetZ;
	float viewOffsetZ;
	float gravity;
	float laggedMovement;
	bool hasVelocityModifier;
	float velocityModifier;
	bool hasStamina;
	float stamina;
	bool hasHealth;
	int health;
	bool hasMaxspeed;
	float maxspeed;
}

enum struct TickInput
{
	float yaw;            // on auto-strafe ticks: the fallback (ground, no key, no speed)
	float pitch;
	float forwardmove;
	float sidemove;
	int buttons;
	bool strafe;          // auto-strafe: the yaw comes from the real velocity (strafe.sp)
	float strafeOffset;   // degrees past the max-gain angle, positive turns harder
	bool strafeStraight;  // straight: the plugin also picks A or D, to keep going strafeHeading (strafe.sp)
	float strafeHeading;  // world yaw, degrees
	float strafeSwing;    // straight, 0: perfect (the side towards the heading each tick); more: keep a side until
	                      // the direction is that many degrees past the heading (fewer, smoother sweeps)
	bool strafeSmooth;    // with a swing and a gain: turn smoothly (AutoStrafe_Smooth)
	float strafeClose;    // ... this close to perfect, 0 to 1, from the gain kept (about 1 - (1 - close)^2 / 4)
	float viewPitch;      // what replays show (g_HasViewPitch); the movement uses pitch
	bool hasViewYaw;      // replays show viewYaw (auto-strafe's camera, @view_yaw); the movement uses the yaw played
	float viewYaw;
}

// One row of the run CSV: the inputs the engine actually processed for the
// tick, and the player state after the engine finished the tick.
enum struct TickRecord
{
	float yaw;
	float pitch;
	float forwardmove;
	float sidemove;
	int buttons;
	float origin[3];
	float velocity[3];
	float baseVelocity[3];
	float hspeed;
	int onGround;
	int groundEntity;
	int ducked;
}

enum struct PathBeam
{
	float a[3];
	float b[3];
	int color[4];
	float width;
}

ConVar g_cvSettleTicks;
ConVar g_cvStrictSettings;
ConVar g_cvAutoDraw;
ConVar g_cvDrawMaxSegments;
ConVar g_cvDrawBudget;
ConVar g_cvMarkerInterval;
ConVar g_cvBeamWidth;
ConVar g_cvBeamMaterial;
ConVar g_cvSpeedMin;
ConVar g_cvSpeedMax;
ConVar g_cvDrawZOffset;
ConVar g_cvRecordMaxSeconds;
ConVar g_cvRecordLeadIn;
ConVar g_cvViewFollow;
ConVar g_cvDrawGhost;
ConVar g_cvPreviewTimescale;
ConVar g_cvPreviewLockView;
ConVar g_cvBot;
ConVar g_cvDrawBox;
ConVar g_cvSessionCfg;
ConVar g_cvBotName;

char g_LogPath[PLATFORM_MAX_PATH];

bool g_Preview;  // the run in progress is a fast preview (preview.sp)
int g_LastButtons;  // the buttons the run's last tick played (after auto bhop and a straight range's A or D)
float g_LastSidemove;  // and its sidemove (a straight range's swing keeps that side); 0 before the first tick
float g_PendingStartYaw;  // the velocity's direction as this tick starts (degrees)...
float g_PrevStartYaw;     // ... and as the run's last tick started (a smooth swing goes on from how it turned)
bool g_HavePrevStartYaw;

// What the human player is doing (mode.sp). Edit mode needs the replay bot (tas_bot 1).
enum
{
	Mode_Edit = 0,    // noclip, no weapons, no damage; runs and previews play on the replay bot
	Mode_Record       // a normal player, for tas_record (prestrafes, rough live runs)
};

char g_RequestId[64];  // the editor request being played (remote.sp), answered when it ends
int g_RunViewer;  // the human who sees a run: messages and path drawing go to them

// A render (render.sp): its phase and what it records; and what a render request asked for (remote.sp).
enum RenderPhase
{
	Render_None,
	Render_PreRoll,  // you're being moved to the spectators
	Render_Playing,  // the run plays (the movie from its tick 0)
	Render_Tail      // after the last tick, until endmovie and putting things back
}

RenderPhase g_RenderPhase = Render_None;
char g_RenderFile[128];
float g_RenderTail = 0.5;
bool g_RenderViewmodel;
int g_RenderHost;    // userid
int g_RenderRunner;  // userid
int g_RenderTeam;
bool g_RenderSetCheats;
bool g_RenderMovie;  // startmovie was sent

// What the request asked for (Remote_ReadRequest fills these).
bool g_ReqRender;
char g_ReqRenderFile[128];
float g_ReqRenderTail = 0.5;
bool g_ReqRenderViewmodel;
