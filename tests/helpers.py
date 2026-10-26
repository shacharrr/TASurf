"""Build run CSVs in the plugin's format (see plugin/scripting/tasurf/record.sp)."""

import math

from tasurf.timeline import float_to_bits

COLUMNS = ("tick,yaw,pitch,forwardmove,sidemove,buttons,x,y,z,vx,vy,vz,hspeed,on_ground,ground_entity,"
           "ducked,base_vx,base_vy,base_vz,x_bits,y_bits,z_bits,vx_bits,vy_bits,vz_bits,yaw_bits,pitch_bits")

DEFAULT_HEADER = {
    "tasurf-run": "1",
    "name": "t",
    "baked_hash": "0123456789abcdef",
    "map": "surf_test",
    "status": "complete",
    "settle_ticks": "1",
    "tick_interval": "0.015000",
    "tick_interval_bits": float_to_bits(0.015),
    "cvar.sv_gravity": "800",
    "cvar.sv_airaccelerate": "150",
    "cvar.sv_maxspeed": "320",
    "cvar.sv_maxvelocity": "3500",
    "player.maxspeed": "250.000000",
    "start.gravity": "1.000000",
    "start.lagged_movement": "1.000000",
}


def f32(x):
    from tasurf.timeline import bits_to_float
    return bits_to_float(float_to_bits(x))


def row(tick, yaw, fwd, side, buttons, pos, vel, on_ground=0, ground_entity=-1, ducked=0, base=(0.0, 0.0, 0.0)):
    pos = [f32(c) for c in pos]
    vel = [f32(c) for c in vel]
    hs = math.hypot(vel[0], vel[1])
    cells = [str(tick), f"{yaw:.6f}", "0.000000", f"{fwd:.6f}", f"{side:.6f}", str(buttons),
             *(f"{c:.6f}" for c in pos), *(f"{c:.6f}" for c in vel), f"{hs:.6f}",
             str(on_ground), str(ground_entity), str(ducked), *(f"{c:.6f}" for c in base),
             *(float_to_bits(c) for c in pos), *(float_to_bits(c) for c in vel), float_to_bits(yaw), float_to_bits(0.0)]
    return ",".join(cells)


def make_csv(rows, header=None):
    h = dict(DEFAULT_HEADER)
    h.update(header or {})
    lines = [f"# {k} {v}" for k, v in h.items()]
    lines.append(COLUMNS)
    lines += rows
    return "\n".join(lines) + "\n"
