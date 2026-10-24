"""Street furniture (the user, 2026-10-13: "make the world look alive"): street lights along the sidewalks
of town streets and wooden utility poles along country roads, placed per road tile with the road tile's
geometry (roads.build_tile) and shown by the mod as Forest items (trees.lua groups "streetlight", "pole").

Spacing from the stock maps and US practice: West Coast USA's lamps stand ~17 m from their nearest
neighbour (often across the street; ~30-40 m along one side), residential streets in US towns light one
side every ~35-45 m, arterials both sides staggered every ~30 m; utility poles every ~40-50 m on one side.
Each road's lamps sit at fixed distances along the road from its first vertex with a phase hashed from the
road's id, so every tile (and every anchor) puts them in the same places (ROADMAP "deterministic
generation"). Pure.
"""
import hashlib
import math

import numpy as np

# class: (spacing m, both sides staggered)
LAMP_SPACING = {"trunk": (30.0, True), "primary": (30.0, True), "secondary": (32.0, True), "tertiary": (36.0, True),
                "residential": (40.0, False), "unclassified": (42.0, False), "living_street": (36.0, False)}
LAMP_CURB_M = 0.5           # m from the curb onto the sidewalk
POLE_SPACING = 45.0
POLE_CLASSES = {"secondary", "tertiary", "residential", "unclassified"}
POLE_OFF_M = 2.4            # m beyond the carriageway's edge
END_CLEAR_M = 4.0           # m kept clear of a run's ends (corners, crosswalk ramps)
CLEAR_OTHER_M = 1.5         # m kept off another road's carriageway
G_LAMP, G_POLE = "streetlight", "pole"


def _h01(*vals):
    h = hashlib.blake2b(repr(vals).encode(), digest_size=8).digest()
    return int.from_bytes(h, "little") / 2.0 ** 64


def _arclen(p):
    d = np.hypot(*np.diff(p[:, :2], axis=0).T)
    return np.concatenate([[0.0], np.cumsum(d)])


def _project(line, q):
    """Arc length along polyline line (k, 2+) of the nearest point to each q (m, 2)."""
    a, b = line[:-1, :2], line[1:, :2]
    d = b - a
    L2 = np.maximum((d * d).sum(1), 1e-12)
    t = np.clip(((q[:, None, :] - a[None]) * d[None]).sum(2) / L2[None], 0, 1)
    p = a[None] + t[..., None] * d[None]
    dist = np.hypot(*(q[:, None, :] - p).transpose(2, 0, 1))
    k = dist.argmin(1)
    s = _arclen(line)
    return s[k] + t[np.arange(len(q)), k] * np.sqrt(L2[k])


def _at(line, s):
    """Points (and unit directions) at arc lengths s along line (k, 3)."""
    ss = _arclen(line)
    s = np.clip(s, 0, ss[-1])
    k = np.clip(np.searchsorted(ss, s, side="right") - 1, 0, len(line) - 2)
    seg = line[k + 1] - line[k]
    L = np.maximum(ss[k + 1] - ss[k], 1e-9)
    t = (s - ss[k]) / L
    p = line[k] + seg * t[:, None]
    d = seg[:, :2] / L[:, None]
    return p, d


def _yaw(dx, dy):
    """Degrees of a Forest item's rotation that turns its local +x toward (dx, dy) (trees.lua: setFromEuler
    with -yaw about z, the engine's clockwise headings)."""
    return math.degrees(math.atan2(dy, dx)) % 360.0


def _clear(p, others, own):
    import fast
    if isinstance(others, fast.Others):         # the Rust check (Python below without the module)
        r = others.clear(p, own, CLEAR_OTHER_M)
        if r is not None:
            return r
        others = others.others
    for fid, half, line in others:
        if fid == own:
            continue
        a, b = line[:-1, :2], line[1:, :2]
        d = b - a
        L2 = np.maximum((d * d).sum(1), 1e-12)
        t = np.clip(((p[:2] - a) * d).sum(1) / L2, 0, 1)
        q = a + t[:, None] * d
        if np.hypot(*(q - p[:2]).T).min() < half + CLEAR_OTHER_M:
            return False
    return True


WIRE_ARMS = (-0.8, 0.0, 0.8)    # m along the cross-arm (the pole's local x) of the three wires
WIRE_Z = 7.9                    # m above the pole's base (s_powerpole_3wire: 8.2 m tall above its origin)
WIRE_SAG = 0.012                # sag per metre of span
WIRE_SEG_M = 5.0
WIRE_R = 0.012                  # m half width of a wire's ribbons


def wire_lines(poles):
    """Polylines (k, 3) of the wires between consecutive poles [(x, y, z, yaw_deg)] of one road side."""
    out = []
    for (x0, y0, z0, w0), (x1, y1, z1, w1) in zip(poles[:-1], poles[1:]):
        span = math.hypot(x1 - x0, y1 - y0)
        if span < 5.0 or span > 80.0:
            continue
        k = max(2, int(math.ceil(span / WIRE_SEG_M)))
        t = np.linspace(0.0, 1.0, k + 1)
        for off in WIRE_ARMS:
            a = np.array([x0 + math.cos(math.radians(w0)) * off, y0 + math.sin(math.radians(w0)) * off, z0 + WIRE_Z])
            b = np.array([x1 + math.cos(math.radians(w1)) * off, y1 + math.sin(math.radians(w1)) * off, z1 + WIRE_Z])
            p = a[None] + (b - a)[None] * t[:, None]
            p[:, 2] -= 4.0 * WIRE_SAG * span * t * (1.0 - t)
            out.append(p)
    return out


def wire_mesh(lines, origin):
    """A mesh of the wires: two crossed ribbons per segment (seen from any side): (positions, tris, uvs)."""
    P, T = [], []
    ox, oy, oz = origin
    for line in lines:
        for a, b in zip(line[:-1], line[1:]):
            d = b - a
            L = float(np.linalg.norm(d))
            if L < 1e-6:
                continue
            d /= L
            side = np.cross(d, [0.0, 0.0, 1.0])
            side = side / (np.linalg.norm(side) or 1.0) * WIRE_R
            up = np.cross(side, d)
            up = up / (np.linalg.norm(up) or 1.0) * WIRE_R
            for v in (side, up):
                i = len(P)
                P += [a - v, b - v, b + v, a + v]
                T += [(i, i + 1, i + 2), (i, i + 2, i + 3), (i, i + 2, i + 1), (i, i + 3, i + 2)]
    if not P:
        return None
    P = np.array(P) - np.array([ox, oy, oz])
    return P, np.array(T, np.int64), np.zeros((len(P), 2))


MAJOR = {"primary", "secondary", "tertiary", "trunk"}
MINOR = {"residential", "unclassified", "living_street"}
G_SIGNAL, G_STOP, G_YIELD = "signal", "stop", "giveway"
JUNCTION_SNAP_M = 1.0       # road ends this close are one junction


def _arms(roads):
    """Junctions: {(x, y) rounded: [(fid, spec, unit vector away from the junction, point, at its start, road
    length), ...]} of road ends that meet (Overture splits segments at their connectors)."""
    ends = {}
    for fid, spec, pts, _ in roads:
        if len(pts) < 2:
            continue
        length = float(np.sum(np.hypot(*np.diff(pts[:, :2], axis=0).T)))
        for p, q, start in ((pts[0], pts[1], True), (pts[-1], pts[-2], False)):
            d = q[:2] - p[:2]
            L = float(np.hypot(*d))
            if L < 1e-3:
                continue
            k = (round(float(p[0]) / JUNCTION_SNAP_M), round(float(p[1]) / JUNCTION_SNAP_M))
            ends.setdefault(k, []).append((fid, spec, d / L, p, start, length))
    return ends


def _outbound(a):
    """Does traffic on arm a (fid, spec, u, p[, at its start[, length]]) only leave the junction (one-way away)?"""
    if len(a) < 5 or not a[1].oneway or not getattr(a[1], "dir", 0):
        return False
    return (a[1].dir > 0) == bool(a[4])


def junction_controls(arms, k, link_ok=None, ring=None, controls=None):
    """Controls confirmed by mapped point evidence, keyed by (road id, at start).

    Road class, junction shape and roundabout geometry do not prove a legal control exists.
    Missing evidence therefore leaves both the physical props and the AI uncontrolled.
    """
    controls = controls or {}
    return [(a, controls[(str(a[0]), bool(a[4]))]) for a in arms
            if len(a) >= 5 and (str(a[0]), bool(a[4])) in controls and not _outbound(a)]


def mapped_controls(frame, infrastructure, roads):
    """Associate explicit Overture/OSM traffic-control points with nearby incoming approaches.

    A signal at the junction centre describes the junction; an approach point describes only
    that approach. Ambiguous stop/yield points and non-ground features are left out. Multiple
    cells or source nodes selecting the same approach produce one control, deterministically.
    """
    import json
    import mapgeom
    if frame is None:
        return {}
    junctions = [(k, arms) for k, arms in sorted(_arms(roads).items()) if len(arms) >= 2]
    if not junctions:
        return {}
    groups = {'traffic_signals': G_SIGNAL, 'stop': G_STOP, 'give_way': G_YIELD}
    points = {}
    for f in infrastructure:
        if f is None:
            continue
        classes, raw_tags, levels = (f.attrs.get(key) for key in ('class', 'tags', 'level'))
        for i in range(len(f)):
            if f.kind[i] != mapgeom.POINT:
                continue
            if levels is not None and np.isfinite(levels[i]) and abs(float(levels[i])) > .1:
                continue
            native = groups.get(str(classes[i]) if classes is not None else '')
            raw = str(raw_tags[i]) if raw_tags is not None else ''
            if native is None and not any(value in raw for value in groups):
                continue
            tags = {}
            if raw_tags is not None:
                try:
                    tags = json.loads(raw)
                except (ValueError, TypeError):
                    pass
            if not isinstance(tags, dict):
                tags = {}
            if tags.get('bridge') == 'yes' or tags.get('tunnel') == 'yes':
                continue
            # A point does not identify the original OSM way or its orientation. Without
            # that relationship, forward/backward (or a bearing) cannot select an approach.
            if tags.get('traffic_signals:direction') or tags.get('direction'):
                continue
            group = groups.get(tags.get('highway')) or native
            if group is None:
                continue
            for j, part in enumerate(f.parts(i)):
                if not part or len(part[0]) != 1 or not np.isfinite(part[0]).all():
                    continue
                lon, lat = part[0][0]
                x, y, _ = frame.geodetic_to_enu(lat, lon, 0.)
                points.setdefault((str(f.ids[i]), j), (np.array([x, y]), group, tags))
    controls = {}
    for _, (point, group, tags) in sorted(points.items()):
        candidates = []
        for k, arms in junctions:
            centre = np.mean([a[3][:2] for a in arms], axis=0)
            distance = float(np.linalg.norm(point - centre))
            if distance > 35.:
                continue
            incoming = [a for a in arms if not _outbound(a)]
            if distance <= 2. and (group == G_SIGNAL or (group == G_STOP and tags.get('stop') == 'all')):
                chosen = incoming
            else:
                ranked = []
                for a in incoming:
                    rel = point - a[3][:2]
                    along = float(rel @ a[2])
                    lateral = abs(float(rel[0] * a[2][1] - rel[1] * a[2][0]))
                    if 2. < along <= min(35., a[5]) and lateral <= a[1].width / 2 + 4.:
                        ranked.append((lateral, str(a[0]), a))
                ranked.sort(key=lambda r: r[:2])
                chosen = [ranked[0][2]] if ranked else []
                if len(ranked) > 1 and ranked[1][0] - ranked[0][0] < 1.:
                    if float(ranked[0][2][2] @ ranked[1][2][2]) < .95:
                        chosen = []       # two different approaches fit; do not guess which is controlled
            if chosen:
                candidates.append((distance, k, chosen))
        if not candidates:
            continue
        _, _, chosen = min(candidates, key=lambda r: r[:2])
        for arm in chosen:
            key = (str(arm[0]), bool(arm[4]))
            # A confirmed traffic light wins over a duplicated stop/yield source record.
            if key not in controls or group == G_SIGNAL:
                controls[key] = group
    return controls


SIGN_CLEAR_M = 0.6          # m a junction sign stands clear of every carriageway's edge


def _off_carriageways(c, u, right, along, out, lines, road_clear=None):
    """The point along m past the junction c along u and out m to the right, moved further along and out (up to
    10 m and 3 m) until it is SIGN_CLEAR_M clear of every carriageway in lines [(half width, centreline)]; None
    when nowhere is: stop signs and signals stood in the middle of a wide road beside the junction (Boston Street,
    Baltimore, the user's report 2026-10-17: 6 in one tile)."""
    for da in (0.0, 2.0, 4.0, 6.0, 8.0, 10.0):
        for do in (0.0, 1.0, 2.0, 3.0):
            q = c + u * (along + da) + right * (out + do)
            if (all(_seg_dist_min(q, ln) >= h + SIGN_CLEAR_M for h, ln in lines)
                    and (road_clear is None or road_clear(q, SIGN_CLEAR_M))):
                return q
    return None


def _seg_dist_min(p, line):
    a, b = line[:-1], line[1:]
    d = b - a
    L2 = np.maximum((d * d).sum(1), 1e-12)
    t = np.clip(((p - a) * d).sum(1) / L2, 0, 1)
    return float(np.hypot(*(a + t[:, None] * d - p).T).min())


def _road_clearance(road_rows):
    """Cache actual asphalt ribbons/miter corners and junction fills once per tile."""
    import roads
    from polygon import contains_points
    polygons = roads.junction_fill_polygons(road_rows)
    for _, spec, pts, _ in road_rows:
        xy = np.asarray(pts)[:, :2]
        if len(xy) < 2:
            continue
        left = roads.offset_polyline(xy, spec.width / 2)
        right = roads.offset_polyline(xy, -spec.width / 2)
        polygons.extend(np.array([a, b, c, d]) for a, b, c, d in
                        zip(left[:-1], right[:-1], right[1:], left[1:]))
    rings = [(np.vstack([p, p[0]]), p.min(axis=0), p.max(axis=0)) for p in polygons]

    def clear(q, margin=0.3):
        for ring, lo, hi in rings:
            if np.any(q < lo - margin) or np.any(q > hi + margin):
                continue
            if contains_points(ring, q[None])[0] or _seg_dist_min(q, ring) < margin:
                return False
        return True
    return clear


def _walk_height(wk, p, offset, curb_h):
    """Match the slab profile and its sampled crossfall at a furniture station."""
    import sidewalks
    run = np.asarray(wk['pts'], np.float64)
    s = float(_project(run, p[None, :2])[0])
    ss = _arclen(run)
    curb, _ = sidewalks.run_geometry(run, wk['side'], wk.get('w', 2.5), wk.get('outward'))
    cs = float(np.interp(s, ss, _arclen(curb)))
    hr = sidewalks.curb_heights(np.array([cs]), wk.get('ramps', ()), curb_h)[0]
    w = wk.get('w', 2.5)
    profile = sidewalks.profile(curb_h, w, hr)[3:-1]
    rise = float(np.interp(offset, profile[:, 0], profile[:, 1]))
    outer = np.interp(s, ss, wk.get('outer_z', run[:, 2]))
    return float(p[2] + (outer - p[2]) * np.clip(offset / w, 0, 1) + rise)


def junction_props(roads, box, curb_h=0.15, left_hand=False, legacy_signs=True, sites=None, road_clear=None,
                   controls=None):
    """Mapped signals, stop and give-way signs on the confirmed incoming approaches.
    Each on the approach's right (its left where traffic drives on the left), at the junction's edge, facing
    the drivers who come in. sites optionally maps (fid, at_start)
    to these exact rows so the AI's working heads replace, rather than duplicate, the static props."""
    x0, y0, x1, y1 = box
    rows = []
    junctions = _arms(sorted(roads, key=lambda r: str(r[0])))
    road_clear = road_clear or _road_clearance(roads)
    lines = [(spec.width / 2, np.asarray(pts, np.float64)[:, :2]) for _, spec, pts, _ in roads if len(pts) >= 2]
    occupied = {}
    for k, arms in sorted(junctions.items()):
        if len(arms) < 2:
            continue
        p = arms[0][3]
        # Plan neighbours too before ownership filtering: duplicate source connectors either side
        # of a tile boundary must select the same one winning pole in both tile builds.
        near = [(h, ln) for h, ln in lines if _seg_dist_min(p[:2], ln) < 80.0 + h]
        put = junction_controls(arms, k, controls=controls)
        if not put:
            continue
        wide = max(a[1].width for a in arms) / 2
        for (fid, spec, u, c, start, *_), what in put:
            # the approach comes in along -u; its right is (-u_y, u_x)... rotated: right of travel (-u)
            inc = -u
            right = np.array([inc[1], -inc[0]]) * (-1 if left_hand else 1)     # the kerb side of the approach
            q = _off_carriageways(c[:2], u, right, wide + 2.5, spec.width / 2 + 1.0, near, road_clear)
            if q is None:
                continue
            q = np.round(q, 2)
            gx, gy = np.floor(q / 2.).astype(int)
            if any(np.linalg.norm(q - other) < 1.5 for dx in (-1, 0, 1) for dy in (-1, 0, 1)
                   for other in occupied.get((gx + dx, gy + dy), ())):
                continue
            occupied.setdefault((gx, gy), []).append(q)
            if not (x0 <= p[0] < x1 and y0 <= p[1] < y1):
                continue
            row = [what, float(q[0]), float(q[1]), round(float(c[2] + curb_h), 2),
                   round(_yaw(right[0], right[1]), 1), 1.0, 9.4 if what == G_SIGNAL else 3.0, 0.0]
            rows.append(row)
            if sites is not None:
                sites[(str(fid), bool(start))] = row
    return rows


SIGN_MPH = (30, 40, 50, 60, 70)     # speed-limit sign models (trees.lua: Utah's ut_<n>_speed_limit)
# ... and the round km/h ones (trees.lua: Driver Training's sign_speed<n>kph) where limits are posted in km/h
# (2026-10-15: Swiss valley roads had American "SPEED LIMIT 50" signs)
SIGN_KMH = (30, 40, 50, 60, 80, 90, 100)
ACTIVE_SIGN_KMH = (30, 50, 90)     # newly added metric signs retained in actual tile placement
SIGN_CLASSES = {"tertiary", "secondary", "primary", "trunk", "unclassified", "residential", "living_street"}
SIGN_MIN_M = 150.0          # a road segment at least this long gets a sign after each of its ends
SIGN_IN_M = 30.0            # m past the junction
SIGN_OFF_M = 1.5            # m beyond the carriageway's edge


def speed_signs(roads, speeds, box, left_hand=False, mph_signs=True, legacy_signs=True):
    """mph: the country posts mph (else the km/h models). Prop rows of speed-limit signs: on roads of SIGN_CLASSES whose limit (speeds: fid -> mph, the
    road graph's) has a sign model, SIGN_IN_M after each end of a segment of at least SIGN_MIN_M, on the kerb
    side of the traffic leaving that end, facing it. Actual tile placement disables legacy models;
    the default preserves historical helper output."""
    if mph_signs and not legacy_signs:
        return []
    x0, y0, x1, y1 = box
    rows = []
    for fid, spec, line, _ in roads:
        mph_v = speeds.get(fid)
        if mph_v is None or spec.cls not in SIGN_CLASSES or len(line) < 2:
            continue
        if mph_signs:
            n = int(round(mph_v / 5.0)) * 5
            group = f"speed{n}" if n in SIGN_MPH else None
        else:
            n = int(round(mph_v * 1.609344 / 10.0)) * 10
            group = f"speedk{n}" if n in (SIGN_KMH if legacy_signs else ACTIVE_SIGN_KMH) else None
        if group is None:
            continue
        L = _arclen(line)[-1]
        if L < SIGN_MIN_M:
            continue
        for s0, sgn in ((SIGN_IN_M, 1.0), (L - SIGN_IN_M, -1.0)):
            if spec.oneway and getattr(spec, "dir", 0) != sgn:
                continue             # only the mapped direction; unknown one-way directions stay unsigned
            p, d = _at(line, np.array([s0]))
            f = d[0] * sgn                                  # the travel direction leaving that end
            side = np.array([f[1], -f[0]]) * (-1 if left_hand else 1)
            q = p[0][:2] + side * (spec.width / 2 + SIGN_OFF_M)
            if x0 <= q[0] < x1 and y0 <= q[1] < y1:
                rows.append([group, round(float(q[0]), 2), round(float(q[1]), 2), round(float(p[0][2]), 2),
                             round(_yaw(side[0], side[1]), 1), 1.0, 2.6, 0.0])
    return rows


def mapped_bus_stops(frame, infrastructure, roads, box, elev, building_layers=None):
    """Real mapped bus boarding points, never road-centre stop_positions or inferred periodic stops.

    Coordinates stay at the mapped point. A stock shelter is only used for shelter=yes, with a
    conservative 2.6 m footprint radius clear of every road; otherwise use a sign. Points on the
    carriageway, far from a drivable road or not on ground level are omitted. The source supplies
    locations and shelter presence, not the shelter's architectural appearance or orientation.
    """
    import json
    import mapgeom
    from polygon import contains_points
    x0, y0, x1, y1 = box
    lines = [(str(fid), spec, np.asarray(pts, np.float64)) for fid, spec, pts, _ in roads if len(pts) >= 2]
    if not lines:
        return []
    lines.sort(key=lambda r: r[0])
    road_clear = _road_clearance(roads)
    buildings_known = building_layers is not None and all(f is not None for f in building_layers)
    outlines = []
    for f in building_layers or ():
        if f is None:
            continue
        for i in range(len(f)):
            if f.kind[i] != mapgeom.POLYGON:
                continue
            for part in f.parts(i):
                if not part or len(part[0]) < 3:
                    continue
                ll = part[0]
                e, n, _ = frame.geodetic_to_enu(ll[:, 1], ll[:, 0], 0.)
                ring = np.c_[e, n]
                if (ring[:, 0].max() < x0 - 3 or ring[:, 0].min() > x1 + 3 or
                        ring[:, 1].max() < y0 - 3 or ring[:, 1].min() > y1 + 3):
                    continue
                outlines.append(np.vstack([ring, ring[0]]) if not np.array_equal(ring[0], ring[-1]) else ring)
    sites = {}
    for f in infrastructure:
        if f is None or not len(f) or "tags" not in f.attrs:
            continue
        for i in range(len(f)):
            if f.kind[i] != mapgeom.POINT:
                continue
            raw = str(f.attrs["tags"][i])
            if "bus" not in raw:
                continue
            try:
                tags = json.loads(raw)
            except (ValueError, TypeError):
                continue
            if not isinstance(tags, dict) or tags.get("public_transport") == "stop_position":
                continue
            if not (tags.get("highway") == "bus_stop" or
                    (tags.get("public_transport") == "platform" and tags.get("bus") == "yes")):
                continue
            level = f.attrs.get("level")
            if level is not None and np.isfinite(level[i]) and abs(float(level[i])) > 0.1:
                continue
            for j, part in enumerate(f.parts(i)):
                if not part or len(part[0]) != 1:
                    continue
                lon, lat = part[0][0]
                x, y, _ = frame.geodetic_to_enu(lat, lon, 0.)
                # Avoid floating-point round-trip noise assigning a border point to both neighbours.
                x, y = round(float(x), 6), round(float(y), 6)
                if math.isfinite(x) and math.isfinite(y) and x0 <= x < x1 and y0 <= y < y1:
                    sites.setdefault((str(f.ids[i]), j), (x, y, float(lat), float(lon), tags))
    rows = []
    for _, (x, y, lat, lon, tags) in sorted(sites.items()):
        q = np.array([x, y])
        distances = [_seg_dist_min(q, line[:, :2]) for _, _, line in lines]
        k = int(np.argmin(distances))
        if distances[k] > 40.0 or not road_clear(q):
            continue
        if any(math.hypot(x - r[1], y - r[2]) < 2.0 for r in rows):
            continue
        if any(contains_points(ring, q[None])[0] or _seg_dist_min(q, ring) < 0.3 for ring in outlines):
            continue
        _, _, line = lines[k]
        p, _ = _at(line, _project(line, q[None]))
        facing = p[0, :2] - q
        shelter = buildings_known and tags.get("shelter") == "yes" and road_clear(q, 2.6) and all(
            _seg_dist_min(q, ring) >= 2.6 for ring in outlines)
        # Stock shelter rear glass/bench are at +x: its open -x side faces the road.
        # The sign's face follows the sign conventions above (local x points toward the kerb).
        yaw = _yaw(*(-facing))
        height = float(np.asarray(elev(np.array([lat]), np.array([lon]))).reshape(-1)[0])
        _, _, z = frame.geodetic_to_enu(lat, lon, height)
        if not math.isfinite(float(z)):
            continue
        rows.append(["busshelter" if shelter else "busstop", round(x, 2), round(y, 2),
                     round(float(z) + 0.15, 2), round(yaw, 1), 1., 3.11 if shelter else 3.0, 0.])
        if len(rows) >= 64:          # bound dense or malformed source data per 256 m tile
            break
    return rows


G_HYDRANT, G_BENCH, G_BIN = "hydrant", "bench", "bin"
G_STREET_TREE = "deciduous"     # trees.lua's broadleaf group (the variant nearest each tree's height)
STREET_TREE_CLASSES = {"residential", "tertiary", "secondary", "living_street"}
STREET_TREE_SHARE = 0.5         # streets with a row of trees (hashed per street)
STREET_TREE_SPACING = 11.0
STREET_TREE_CURB_M = 0.9
G_POLELAMP = "polelamp"     # a cobra-head arm on a utility pole (East Coast USA's electrical_pole_lamp)
TOWN_POLE_CLASSES = {"residential", "tertiary", "unclassified", "secondary", "living_street", "primary", "trunk"}
TOWN_POLE_SPACING = 40.0
POLE_LAMP_Z = 7.0           # m above the pole's base
HYDRANT_SPACING = 120.0
BENCH_SPACING = 140.0
BIN_SPACING = 90.0
BENCH_YAW = 90.0            # the bench model's front is its local +y (the yaw puts +x toward the road)


def _stations(run, line, spacing, phase, clear, side):
    """(point (3,), away-from-the-road unit (2,)) on a sidewalk run at the road's stations phase + k spacing
    (arc length along the road line, so every tile and anchor gets the same ones), END_CLEAR_M from the run's
    ends."""
    s_run = _project(line, run[:, :2])
    lo, hi = min(s_run[0], s_run[-1]), max(s_run[0], s_run[-1])
    k0, k1 = math.ceil((lo - phase) / spacing), math.floor((hi - phase) / spacing)
    if k1 < k0:
        return []
    p_road, _ = _at(line, phase + spacing * np.arange(k0, k1 + 1))
    s_on_run = _project(run, p_road[:, :2])
    L = _arclen(run)[-1]
    out = []
    for s in s_on_run:
        if s < clear or s > L - clear:
            continue
        p, d = _at(run, np.array([s]))
        out.append((p[0], np.array([-d[0][1], d[0][0]]) * side))   # away from the road (sidewalks.run_geometry)
    return out


DECK_HEADROOM_M = 0.5      # m a prop's top stays below a bridge deck over it
DECK_SIDE_M = 1.0          # m beside a deck's edge that still counts as under it
DECK_WIRE_M = 1.5          # m a wire stays below a deck over it


def under_decks(rows, decks, wires=None):
    """rows without the props that would reach up into a bridge deck above them (a road's lamps and poles
    under a motorway bridge stood 9 m tall through its deck: a car hit one at 75 mph on I-80, 2026-10-15).
    decks: [(points (k, 3) of the deck's centreline, half width)]. wires: removed in place where one passes
    under a deck closer than the headroom."""
    if not decks:
        return rows
    segs = []
    for pts, half in decks:
        p = np.asarray(pts, np.float64)
        if len(p) >= 2:
            segs.append((p[:-1], p[1:], half + DECK_SIDE_M))

    def deck_z(x, y):
        """The lowest deck surface over (x, y), or None."""
        best = None
        for a, b, reach in segs:
            d = b[:, :2] - a[:, :2]
            L2 = np.maximum((d * d).sum(1), 1e-12)
            t = np.clip(((np.array([x, y]) - a[:, :2]) * d).sum(1) / L2, 0, 1)
            q = a[:, :2] + t[:, None] * d
            near = np.hypot(q[:, 0] - x, q[:, 1] - y) < reach
            if near.any():
                z = float((a[:, 2] + t * (b[:, 2] - a[:, 2]))[near].min())
                best = z if best is None else min(best, z)
        return best
    out = []
    for r in rows:
        z = deck_z(r[1], r[2])
        if z is not None and r[3] < z and r[3] + r[6] > z - DECK_HEADROOM_M:
            continue
        out.append(r)
    if wires is not None:
        keep = []
        for w in wires:
            w = np.asarray(w, np.float64)
            hit = False
            for x, y, wz in w[:, :3]:
                z = deck_z(x, y)
                if z is not None and wz > z - DECK_WIRE_M:     # through the deck, or just under it
                    hit = True
                    break
            if not hit:
                keep.append(w)
        wires[:] = keep
    return out


def park_polygons(frame, features):
    """Mapped parks/gardens in anchor coordinates, preserving each polygon's interior holes."""
    import json
    import mapgeom
    out = []
    for f in features:
        if f is None or f.layer != "land_use":
            continue
        classes, tags = f.attrs.get("class"), f.attrs.get("tags")
        for i in range(len(f)):
            if f.kind[i] != mapgeom.POLYGON:
                continue
            mapped = classes is not None and str(classes[i]) in ("park", "garden")
            if not mapped and tags is not None:
                try:
                    value = json.loads(str(tags[i]))
                    mapped = isinstance(value, dict) and value.get("leisure") in ("park", "garden")
                except (ValueError, TypeError):
                    pass
            if not mapped:
                continue
            for part in f.parts(i):
                rings = []
                for ll in part:
                    if len(ll) < 3 or not np.isfinite(ll).all():
                        rings = []             # malformed holes must not turn into furnished park land
                        break
                    e, n, _ = frame.geodetic_to_enu(ll[:, 1], ll[:, 0], 0.)
                    rings.append(np.c_[e, n])
                if rings:
                    out.append(rings)
    return out


def place(roads, walks, box, curb_h=0.15, wires=None, town=False, left_hand=False, speeds=None, mph=True, decks=(),
          park_areas=(), frame=None, elev=None, infrastructure=()):
    """Rows [group, x, y, z, yaw, scale, height, rank] (trees.lua's tile rows) of the lamps and poles of a
    tile box (x0, y0, x1, y1). roads: build_tile's (fid, spec, centreline (k, 3), lon); walks: its sidewalk
    runs (dicts with fid, pts, side, w); decks: the bridge decks over the tile [(centreline (k, 3), half
    width)]: nothing tall under them (under_decks)."""
    x0, y0, x1, y1 = box
    by_fid = {fid: (spec, pts) for fid, spec, pts, _ in roads}
    import fast
    others = fast.Others([(fid, spec.width / 2, pts) for fid, spec, pts, _ in roads])
    road_clear = _road_clearance(roads)
    rows = []
    ground_targets, wire_runs = [], []
    from polygon import contains_points

    def in_park(q):
        return any(contains_points(part[0], q[None])[0] and
                   not any(contains_points(hole, q[None])[0] for hole in part[1:]) for part in park_areas)

    lit = set()
    for wk in walks:
        fid = wk.get("fid")
        if fid is None or "pts" not in wk or fid not in by_fid:
            continue
        if wk.get("median"):        # a divided road's median: no lamps, poles, benches or bins on it (Boston
            continue                # Street's is a bare curb, Mapillary 2026-10-17)
        spec, line = by_fid[fid]
        run = np.asarray(wk["pts"], np.float64)
        if len(run) < 2:
            continue
        side = wk["side"]
        w = wk.get("w", 2.5)

        def put(group, spacing, phase, curb_off, height, yaw_fn, clear=END_CLEAR_M):
            for p, out in _stations(run, line, spacing, phase, clear, side):
                q = p[:2] - out * (w / 2 - curb_off)
                if not road_clear(q):
                    continue
                if group in (G_BENCH, G_BIN) and not in_park(q):
                    continue
                if x0 <= q[0] < x1 and y0 <= q[1] < y1:
                    rows.append([group, round(float(q[0]), 2), round(float(q[1]), 2), round(_walk_height(wk, p, curb_off, curb_h), 2),
                                 round(yaw_fn(out), 1), 1.0, height, 0.0])
        rule = LAMP_SPACING.get(spec.cls)
        if town and spec.cls in TOWN_POLE_CLASSES:
            # towns and suburbs: wooden utility poles along one side, wired, a cobra-head lamp on every other
            pole_side = 1 if _h01(fid, "side") < 0.5 else -1
            if side == pole_side:
                lit.add(fid)
                run_poles = []
                ph = _h01(fid, "tphase") * TOWN_POLE_SPACING
                for p, out in _stations(run, line, TOWN_POLE_SPACING, ph, END_CLEAR_M, side):
                    q = p[:2] - out * (w / 2 - 0.6)
                    if not road_clear(q):
                        wire_runs.append(run_poles)
                        run_poles = []
                        continue
                    yaw = _yaw(out[0], out[1])
                    z = _walk_height(wk, p, 0.6, curb_h)
                    run_poles.append((float(q[0]), float(q[1]), z, yaw))
                    if x0 <= q[0] < x1 and y0 <= q[1] < y1:
                        rows.append([G_POLE, round(float(q[0]), 2), round(float(q[1]), 2), round(z, 2),
                                     round(yaw, 1), 1.0, 9.2, 0.0])
                        k = int(round((float(_project(line, q[None])[0]) - ph) / TOWN_POLE_SPACING))
                        if k % 2 == 0:
                            rows.append([G_POLELAMP, round(float(q[0]), 2), round(float(q[1]), 2),
                                         round(z + POLE_LAMP_Z, 2), round(yaw, 1), 1.0, 1.1, 0.0])
                if wires is not None and len(run_poles) > 1:
                    wire_runs.append(run_poles)
            rule = None
        if rule is not None:
            spacing, both = rule
            lamp_side = 1 if _h01(fid, "side") < 0.5 else -1
            if both or side == lamp_side:
                lit.add(fid)
                phase = _h01(fid, "phase") * spacing + (spacing / 2 if (both and side < 0) else 0.0)
                put(G_LAMP, spacing, phase, LAMP_CURB_M, 9.0, lambda out: _yaw(-out[0], -out[1]))
        # No inferred tree pits on pedestrian slabs: furniture must not bypass
        # the final sidewalk/path exclusion applied to canopy vegetation.
        if spec.urban:
            # fire hydrants at the curb (US practice ~90-150 m apart), alternating sides
            put(G_HYDRANT, HYDRANT_SPACING, _h01(fid, "hyd") * HYDRANT_SPACING + (HYDRANT_SPACING / 2 if side < 0 else 0),
                0.45, 0.9, lambda out: _h01(fid, "hy") * 360.0)
        if spec.urban and spec.cls in MAJOR and spec.cls not in ("trunk", "motorway") and park_areas:
            # Existing sidewalk candidates only inside mapped parks/gardens; never highway furniture.
            put(G_BENCH, BENCH_SPACING, _h01(fid, side, "bench") * BENCH_SPACING, w - 0.45, 0.9,
                lambda out: _yaw(-out[0], -out[1]) + BENCH_YAW)
            put(G_BIN, BIN_SPACING, _h01(fid, side, "bin") * BIN_SPACING, 0.7, 1.1, lambda out: _yaw(-out[0], -out[1]))
    controls = mapped_controls(frame, infrastructure, roads)
    signs = junction_props(roads, box, curb_h, left_hand, legacy_signs=False, road_clear=road_clear, controls=controls)
    if speeds:
        signs.extend(speed_signs(roads, speeds, box, left_hand, mph, legacy_signs=False))
    for row in signs:
        if road_clear(np.array(row[1:3])):
            rows.append(row)
            ground_targets.append((row, 1, 3, curb_h))
    for fid, spec, line, _ in roads:
        if spec.urban or fid in lit or spec.cls not in POLE_CLASSES or len(line) < 2:
            continue
        side = 1 if _h01(fid, "pside") < 0.5 else -1
        phase = _h01(fid, "pphase") * POLE_SPACING
        L = _arclen(line)[-1]
        s = phase + POLE_SPACING * np.arange(0, int((L - phase) // POLE_SPACING) + 1)
        if not len(s):
            continue
        P, D = _at(line, s)
        run = []
        for p, d in zip(P, D):
            n = np.array([-d[1], d[0]]) * side
            q = p[:2] + n * (spec.width / 2 + POLE_OFF_M)
            inside = x0 <= q[0] < x1 and y0 <= q[1] < y1
            if not road_clear(q) or not _clear(np.array([q[0], q[1], p[2]]), others, fid):
                if len(run) > 1 and wires is not None:
                    wire_runs.append(run)
                run = []
                continue
            # the cross-arm (local x) across the road, the wires (local y) along it
            yaw = _yaw(n[0], n[1])
            pole = [float(q[0]), float(q[1]), float(p[2]), yaw]
            run.append(pole)
            ground_targets.append((pole, 0, 2, 0.0))
            if inside:
                rows.append([G_POLE, round(float(q[0]), 2), round(float(q[1]), 2), round(float(p[2]), 2),
                             round(yaw, 1), 1.0, 9.2, 0.0])
                ground_targets.append((rows[-1], 1, 3, 0.0))
        if len(run) > 1 and wires is not None:
            wire_runs.append(run)
    if ground_targets and frame is not None and elev is not None:
        xy = np.array([item[start:start + 2] for item, start, _, _ in ground_targets])
        la, lo, _ = frame.enu_to_geodetic(xy[:, 0], xy[:, 1], 0.0)
        z = frame.geodetic_to_enu(la, lo, elev(la, lo))[2]
        for (item, _, zi, rise), height in zip(ground_targets, z):
            item[zi] = round(float(height) + rise, 2)
    if wires is not None:
        for run in wire_runs:
            wires.extend(_own_spans(run, box))
    return under_decks(rows, list(decks), wires)


def _own_spans(run, box):
    """The wires of the spans of a run of poles that start in the tile (each span drawn by one tile)."""
    x0, y0, x1, y1 = box
    out = []
    for a, b in zip(run[:-1], run[1:]):
        if x0 <= a[0] < x1 and y0 <= a[1] < y1:
            out.extend(wire_lines([a, b]))
    return out
