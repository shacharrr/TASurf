"""TASurf: offline tool-assisted-speedrun tooling for Counter-Strike: Source surf.

The Python side turns a hand-edited timeline (JSON) into a flat per-tick input
file ("baked" file) for the SourceMod plugin, and analyses the CSVs the plugin
records. The game engine is always the ground truth for movement; nothing here
simulates movement except the optional reference-physics sanity check.
"""

__version__ = "1.0.1"
