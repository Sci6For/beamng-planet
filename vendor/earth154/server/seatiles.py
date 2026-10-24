"""The sea's water tiles near the player: square WaterBlocks that follow the Earth's curve (seacarve.py,
the mod's sea.lua; docs/research/2026-10-13-water-probe.md).

    tiles = plan(frame, x, y, sea_at)     # [Tile]: the anchor frame's sea tiles around (x, y)

A quadtree on a lattice of TILE_MAX squares in the anchor frame: a square is split while the player is
closer than SPLIT_K times its size (down to TILE_MIN), so the tiles are small near the player and large
far away (<= 34 over open sea; 83 tiles cost 3-9 % fps at Sandy Point, 2026-10-13). A leaf is kept when sea_at
says it holds sea. Each tile's surface is the sea level (seacarve.SEA_E) at its centre, its normal the
local vertical there, so its surface is at most (size/2)^2 / 2R off the true sea: 0.17 m at the corners
of 2048 m tiles, 1.3 m at the corners of 8192 m ones (which are >= 6 km away).
"""
import math
from dataclasses import dataclass

import numpy as np

TILE_MIN = 2048.0           # the sea's tiles: 6 inches off the curve at the corners of the smallest
PLAN_SQUARE = 512.0         # the server plans again when the player enters another square of this size
TILE_MAX = 8192.0
SPLIT_K = 0.75              # a square closer to the player than SPLIT_K * its size is split (<= 34 tiles)
RADIUS_M = 8192.0           # tiles within this of the player; beyond, the far tiles' photo
SAMPLES = 64                # sea_at is asked on a SAMPLES x SAMPLES grid per tile (32-128 m apart)
EARTH_R = 6371000.0


@dataclass(frozen=True)
class Tile:
    key: str                # "<size>_<i>_<j>": the square's lattice index at its size
    x: float                # centre in the anchor frame (m)
    y: float
    z: float                # the sea level there
    size: float
    nx: float               # the local vertical's tilt (its x and y; z = sqrt(1 - nx^2 - ny^2))
    ny: float
    w: float = None           # optional inland shoreline rectangle; legacy tiles remain square
    h: float = None

    @property
    def width(self):
        return self.size if self.w is None else self.w

    @property
    def height(self):
        return self.size if self.h is None else self.h

    def to_msg(self):
        msg = [self.key, round(self.x, 2), round(self.y, 2), round(self.z, 3), self.size,
               round(self.nx, 6), round(self.ny, 6)]
        if self.w is not None:
            msg += [self.w, self.h]
        return msg


def _dist(x, y, x0, y0, s):
    """Distance from (x, y) to the square [x0, x0 + s] x [y0, y0 + s]."""
    dx = max(x0 - x, 0.0, x - (x0 + s))
    dy = max(y0 - y, 0.0, y - (y0 + s))
    return math.hypot(dx, dy)


def square_of(x, y):
    """The PLAN_SQUARE the player is in: the plans are the same anywhere inside it."""
    return math.floor(x / PLAN_SQUARE), math.floor(y / PLAN_SQUARE)


def lake_square_of(x, y):
    """Smaller stable region for the bounded near-shore water budget."""
    return math.floor(x / 128.0), math.floor(y / 128.0)


def squares(x, y, radius=None):
    """The quadtree leaves around (x, y): [(x0, y0, size)], every point within `radius` (default
    RADIUS_M) of the centre of (x, y)'s TILE_MIN square in exactly one of them."""
    r = RADIUS_M if radius is None else radius
    # planned from the centre of the player's TILE_MIN square: the set only changes when the player
    # crosses into another square (no tiles made and deleted every few metres)
    x = (math.floor(x / TILE_MIN) + 0.5) * TILE_MIN
    y = (math.floor(y / TILE_MIN) + 0.5) * TILE_MIN
    out = []
    i0, i1 = math.floor((x - r) / TILE_MAX), math.floor((x + r) / TILE_MAX)
    j0, j1 = math.floor((y - r) / TILE_MAX), math.floor((y + r) / TILE_MAX)

    def visit(x0, y0, s):
        d = _dist(x, y, x0, y0, s)
        if d > r:
            return
        if s > TILE_MIN and d < SPLIT_K * s:
            h = s / 2
            for a in (0, 1):
                for b in (0, 1):
                    visit(x0 + a * h, y0 + b * h, h)
        else:
            out.append((x0, y0, s))
    for i in range(i0, i1 + 1):
        for j in range(j0, j1 + 1):
            visit(i * TILE_MAX, j * TILE_MAX, TILE_MAX)
    return out


def plan(frame, x, y, sea_at, radius=None):
    """The sea tiles around (x, y) in `frame` (a geodesy.EnuFrame). sea_at(lat, lon, size) -> bool array:
    which sample points of a tile of that size are sea."""
    leaves = squares(x, y, radius)
    if not leaves:
        return []
    f = (np.arange(SAMPLES) + 0.5) / SAMPLES
    out = []
    by_size = {}
    for x0, y0, s in leaves:
        by_size.setdefault(s, []).append((x0, y0))
    for s, corners in sorted(by_size.items()):
        c = np.asarray(corners, np.float64)
        gx = (c[:, 0:1] + f[None, :] * s)                  # [n, SAMPLES]
        gy = (c[:, 1:2] + f[None, :] * s)
        E = np.repeat(gx[:, None, :], SAMPLES, axis=1)     # [n, j, i]
        N = np.repeat(gy[:, :, None], SAMPLES, axis=2)
        lat, lon, _ = frame.enu_to_geodetic(E.ravel(), N.ravel(), 0.0)
        sea = np.asarray(sea_at(lat, lon, s), bool).reshape(len(c), -1).any(axis=1)
        for (x0, y0), keep in zip(corners, sea):
            if not keep:
                continue
            cx, cy = x0 + s / 2, y0 + s / 2
            la, lo, _ = frame.enu_to_geodetic(cx, cy, 0.0)
            z = float(np.asarray(frame.geodetic_to_enu(la, lo, 0.0)[2]))
            # the local vertical at (cx, cy): the sphere's normal, the frame's up tilted by the offset
            nx, ny = cx / EARTH_R, cy / EARTH_R
            out.append(Tile(f"{int(s)}_{int(x0 // s)}_{int(y0 // s)}", cx, cy, z, s, nx, ny))
    return out


# --- lakes and rivers ----------------------------------------------------------------------------------------
LAKE_RADIUS_M = 2000.0      # lake tiles within this of the player (the far tiles carved: L <= 2)
LAKE_TILE_MAX = 512.0
LAKE_TILE_MIN = 64.0
LAKE_TILE_FLOOR = 16.0      # ... split on to this where land under the water's plane would flood (LAKE_FLOOD_SHARE)
LAKE_SHORE_STEP = 4.0       # mixed 16 m leaves: aligned wet rectangles, no whole-leaf omission
LAKE_SAMPLES = 24           # samples per tile edge when checking a lake tile
LAKE_FLOOD_M = 0.0          # dry ground below the surface must not be covered by a lake tile
LAKE_NEAR_SHORE_M = 256.0   # exact shoreline budget around the player; distant water uses terrain occlusion
LAKE_MAX_TILES = 120        # nearest first (lakes and wide rivers)
LAKE_DEM_ZOOM = 15
LEVEL_TOL_M = 0.4            # a tile's water is at most this far from its plane (else it is split)


def _plane(u, v, z):
    """Least-squares plane z = a + b u + c v through the points (a level plane when they are too few or in
    a line)."""
    if len(z) < 3:
        return float(np.median(z)), 0.0, 0.0
    A = np.stack([np.ones_like(u), u, v], -1)
    sol, _, rank, _ = np.linalg.lstsq(A, z, rcond=None)
    if rank < 3:
        return float(np.median(z)), 0.0, 0.0
    return float(sol[0]), float(sol[1]), float(sol[2])


def _wet_rectangles(mask):
    """Merge equal horizontal runs on adjacent rows into nonoverlapping pixel rectangles."""
    active, out = {}, []
    for j, row in enumerate(mask):
        runs, i = [], 0
        while i < len(row):
            if not row[i]:
                i += 1
                continue
            start = i
            while i < len(row) and row[i]:
                i += 1
            runs.append((start, i))
        current = {}
        for run in runs:
            previous = active.pop(run, None)
            current[run] = (previous[0] if previous else j, j + 1)
        for (a, b), (j0, j1) in active.items():
            out.append((a, j0, b, j1))
        active = current
    for (a, b), (j0, j1) in active.items():
        out.append((a, j0, b, j1))
    return out


def _coalesce_lake_rectangles(rects):
    """Join coplanar, edge-adjacent rectangles without adding any dry area."""
    def join(items, horizontal):
        grouped = {}
        for x0, y0, w, h, level, bx, by in items:
            # Flat connected lakes share a level. Leave slopes alone: their centre levels differ,
            # and rounding them into one group would make a height step after merging.
            plane = (round(level, 2),) if abs(bx) < 1e-6 and abs(by) < 1e-6 else (x0, y0, w, h)
            fixed = (y0, h) if horizontal else (x0, w)
            grouped.setdefault((plane, fixed), []).append((x0, y0, w, h, level, bx, by))
        merged = []
        for group in grouped.values():
            group.sort(key=lambda t: t[0] if horizontal else t[1])
            current = list(group[0])
            for t in group[1:]:
                touching = (current[0] + current[2] == t[0] if horizontal
                            else current[1] + current[3] == t[1])
                # A merged block is still one tangent plane. Bound its span so joining a very
                # large lake does not introduce decimetre height steps beside small shore tiles.
                span = current[2 if horizontal else 3] + t[2 if horizontal else 3]
                if touching and span <= 1024.0:
                    current[2 if horizontal else 3] += t[2 if horizontal else 3]
                else:
                    merged.append(tuple(current))
                    current = list(t)
            merged.append(tuple(current))
        return merged
    return join(join(rects, True), False)


def _lake_distance(x, y, rect):
    x0, y0, w, h = rect[:4]
    return math.hypot(max(x0 - x, 0.0, x - x0 - w), max(y0 - y, 0.0, y - y0 - h))


def _bounded_lake_rectangles(rects, x, y):
    """Keep a complete near-water neighborhood under the object budget, never a partial distance band."""
    rects.sort(key=lambda t: (_lake_distance(x, y, t), t[0], t[1]))
    if len(rects) <= LAKE_MAX_TILES:
        return rects
    # A partial tie cuts a rectangular hole through a same-distance bank. Leave that whole band for
    # the far photograph and keep the complete closer region instead.
    cutoff = _lake_distance(x, y, rects[LAKE_MAX_TILES])
    return [t for t in rects if _lake_distance(x, y, t) < cutoff]


def plan_lakes(frame, x, y, lake_at, ground_at, radius=None):
    """Water tiles for the lakes and rivers around (x, y): lake_at(lat, lon) -> the inland water's level at
    the points (NaN away from it, seacarve.SeaCarver.lake_at), ground_at(lat, lon) -> the DEM there. A square of the
    LAKE_TILE_MAX lattice holding lake water becomes one tile at that level when every other point in it
    is land no lower than the water's plane by more than LAKE_FLOOD_M and the water is within LEVEL_TOL_M
    of that plane (a least-squares fit: a river falls across a tile, which is tilted to match), else it
    is split to 16 m. Mixed leaves become 4 m aligned rectangles of wet cells. Within 256 m of the
    player's 128 m planning neighborhood, any dry sample below the water surface forces a split;
    farther away the old 1 m terrain-occlusion tolerance limits object count. The complete nearest
    distance bands are retained under the 120-object cap. Tilted to the local vertical like sea tiles."""
    r = LAKE_RADIUS_M if radius is None else radius
    # The root set is stable over 512 m, but its corner is too far from the player for a capped
    # shoreline budget. Keep budget selection stable over smaller 128 m neighborhoods.
    lx, ly = lake_square_of(x, y)
    px, py = (lx + 0.5) * 128.0, (ly + 0.5) * 128.0
    x = (math.floor(x / PLAN_SQUARE) + 0.5) * PLAN_SQUARE
    y = (math.floor(y / PLAN_SQUARE) + 0.5) * PLAN_SQUARE
    s0 = LAKE_TILE_MAX
    i0, i1 = math.floor((x - r) / s0), math.floor((x + r) / s0)
    j0, j1 = math.floor((y - r) / s0), math.floor((y + r) / s0)
    roots = [(i * s0, j * s0) for i in range(i0, i1 + 1) for j in range(j0, j1 + 1)
             if _dist(x, y, i * s0, j * s0, s0) <= r]
    if not roots:
        return []
    f = (np.arange(LAKE_SAMPLES) + 0.5) / LAKE_SAMPLES
    out = []

    def samples(sq, s):
        c = np.asarray(sq, np.float64)
        E = np.repeat((c[:, 0:1] + f[None, :] * s)[:, None, :], LAKE_SAMPLES, axis=1)
        N = np.repeat((c[:, 1:2] + f[None, :] * s)[:, :, None], LAKE_SAMPLES, axis=2)
        lat, lon, _ = frame.enu_to_geodetic(E.ravel(), N.ravel(), 0.0)
        return lat, lon

    todo, s = roots, s0
    while todo and s >= LAKE_TILE_FLOOR:
        lat, lon = samples(todo, s)
        lv = np.asarray(lake_at(lat, lon), np.float64).reshape(len(todo), -1)
        has = np.isfinite(lv).any(axis=1)
        todo = [sq for sq, h in zip(todo, has) if h]
        lv = lv[has]
        if not todo:
            break
        lat, lon = samples(todo, s)
        g = np.asarray(ground_at(lat, lon), np.float64).reshape(len(todo), -1)
        nxt = []
        u = (f - 0.5) * s                               # sample offsets from the tile's centre
        U = np.tile(u, LAKE_SAMPLES)                    # [j, i] raveled: i fastest
        V = np.repeat(u, LAKE_SAMPLES)
        for (x0, y0), l, gr in zip(todo, lv, g):
            wet = np.isfinite(l)
            # the water as a plane over the tile (a river falls across it): level at the centre, slope
            a, bx, by = _plane(U[wet], V[wet], l[wet])
            fit = np.abs(l[wet] - (a + bx * U[wet] + by * V[wet])).max() <= LEVEL_TOL_M
            plane = a + bx * U + by * V
            near = _lake_distance(px, py, (x0, y0, s, s)) < LAKE_NEAR_SHORE_M
            margin = LAKE_FLOOD_M if near else -1.0
            low = gr[~wet] < plane[~wet] + margin  # water could be visible above dry ground
            clear = not low.any()
            # a tile at least half water may hold a little low land (a shore's noise), but not a trench or a
            # valley: those split on down to LAKE_TILE_FLOOR (a reservoir's 64 m tile flooded Central Park's
            # sunken 86th Street Transverse beside it, the user's report 2026-10-17)
            if fit and clear:
                out.append((x0, y0, s, s, a, bx, by))
            elif s > LAKE_TILE_FLOOR:
                h = s / 2
                nxt += [(x0 + a * h, y0 + b * h) for a in (0, 1) for b in (0, 1)]
            else:
                # A mixed leaf used to vanish altogether, leaving a square hole in a connected lake.
                # Sample on the same global 2 m lattice as adjacent leaves; only wet cells become
                # WaterBlocks. Dry valley cells are never enclosed by a whole 16 m square.
                step = LAKE_SHORE_STEP
                n = int(round(s / step))
                xx, yy = np.meshgrid(x0 + (np.arange(n) + 0.5) * step,
                                     y0 + (np.arange(n) + 0.5) * step)
                rla, rlo, _ = frame.enu_to_geodetic(xx.ravel(), yy.ravel(), 0.0)
                local = np.asarray(lake_at(rla, rlo), np.float64).reshape(n, n)
                finite = np.isfinite(local)
                groups = [finite] if fit else [finite & (np.rint(local / LEVEL_TOL_M) == k)
                                              for k in np.unique(np.rint(local[finite] / LEVEL_TOL_M))]
                for group in groups:
                    for i0, j0, i1, j1 in _wet_rectangles(group):
                        rx, ry = x0 + i0 * step, y0 + j0 * step
                        rw, rh = (i1 - i0) * step, (j1 - j0) * step
                        if fit:
                            # Preserve the parent's continuous plane across all its wet pieces.
                            rz = a + bx * (rx + rw / 2 - x0 - s / 2) + by * (ry + rh / 2 - y0 - s / 2)
                            rb, rc = bx, by
                        else:
                            wet_lv = local[j0:j1, i0:i1][group[j0:j1, i0:i1]]
                            rz = float(np.median(wet_lv))
                            rb = rc = 0.0
                        out.append((rx, ry, rw, rh, rz, rb, rc))
        todo, s = nxt, s / 2
    out = _bounded_lake_rectangles(_coalesce_lake_rectangles(out), px, py)
    tiles = []
    for x0, y0, w, h, level, bx, by in out:
        cx, cy = x0 + w / 2, y0 + h / 2
        la, lo, _ = frame.enu_to_geodetic(cx, cy, 0.0)
        z = float(np.asarray(frame.geodetic_to_enu(la, lo, level)[2]))
        # the tilt: the local vertical, then the water's own slope (a surface rising by b per metre in x
        # leans its normal by -b in x)
        rect = w != h or w < LAKE_TILE_FLOOR
        size = max(w, h)
        key = (f"Lr{int(x0 / LAKE_SHORE_STEP)}_{int(y0 / LAKE_SHORE_STEP)}_{int(w)}_{int(h)}_"
               f"{int(round(level * 10))}")
        tiles.append(Tile(key, cx, cy, z, size, cx / EARTH_R - bx, cy / EARTH_R - by,
                          w if rect else None, h if rect else None))
    return tiles
