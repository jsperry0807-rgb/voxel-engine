# Water and current textures

Status: M3.5-15 art data, 2026-10-31.

| Face | Slot | Alpha | Tiling | Use |
| --- | --- | --- | --- | --- |
| `water_still` | 8 | Translucent | Both | Still water; authored in M1.5-14 |
| `water_flow` | 70 | Translucent | Both | Rivers and current |
| `foam_strip` | 71 | Translucent | Both | River foam overlay |

**Flow axis.** `water_flow` and `foam_strip` flow along **+X (east)**. Both
patterns are stretched along X and vary across Y, so the same tile can be
rotated by the renderer for the other cardinal directions.

**UV scroll.** Nominal **0.25 tile/s along +X**. M3.5-08 scales that by the
current speed; the atlas tile is static and carries no time. The flow direction
and rate are renderer state, not a second set of textures.

**Alpha.** Both faces are `Translucent`. `water_flow` maps a periodic Fbm field
through `AlphaBand(42000, 56000)`; `foam_strip` maps a smoothed foam mask through
`AlphaBand(30000, 60000)`. Neither face uses binary alpha.

**Caustics.** Procedural in the shader; no `caustics` atlas tile. This is the
M3.5-15 decision and is recorded here so the shader task does not invent a
texture.
