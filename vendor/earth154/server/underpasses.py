"""Bounded source geometry for short mapped overpasses and their tight abutments.

Both the terrain grader and bridge mesh use this description. A collar follows
only the same source road's ground continuation; it never connects nearby roads.
"""
import os
import numpy as np
import mapcells
import mapgeom

MAX_SPAN = 500.0
COLLAR_M = 12.0
GRID_SUPPORT = 2.0 * np.sqrt(2.0)  # furthest vertex of a 2m terrain triangle
ENABLED = os.environ.get('BNE_UNDERPASS', 'on') != 'off'


def arc(xy):
    return np.r_[0., np.cumsum(np.linalg.norm(np.diff(xy, axis=0), axis=1))]


def nearest(points, line):
    """Distance, segment index and unclipped parameter; bounded by caller batches."""
    a, d = line[:-1, :2], np.diff(line[:, :2], axis=0)
    t = ((points[:, None, :2] - a) * d).sum(2) / np.maximum((d*d).sum(1), 1e-12)
    ds = np.linalg.norm(points[:, None, :2] - a - np.clip(t, 0, 1)[:, :, None]*d, axis=2)
    k = ds.argmin(1)
    return ds[np.arange(len(points)), k], k, t[np.arange(len(points)), k]


def cut_run(frame, ring, distance):
    xy = np.stack(mapgeom.enu_xy(None, frame, ring), 1)
    s = arc(xy)
    at = min(distance, s[-1])
    point = np.array([np.interp(at, s, ring[:, c]) for c in (0, 1)])
    return np.vstack([ring[s < at], point]), np.vstack([point, ring[s > at]])


def propagate(plans, joints, grade=.06, max_drop=8.):
    """Carry an existing cut through explicit ground connectors, never XY crossings.

    Dijkstra visits the connector graph once; each road receives the lower
    envelope of its connector constraints. Original profiles bound the cut.
    """
    import heapq
    from grading import Plan
    by_fid = {}
    ss = []
    for i,(fid,p) in enumerate(plans):
        by_fid.setdefault(fid,[]).append(i)
        ss.append(arc(p.xy[:,:2]))
    members, on_road = {}, {}
    for cid, refs in sorted(joints.items()):
        hits = []
        for fid,point in refs:
            for i in by_fid.get(fid,()):
                p=plans[i][1]
                dist,k,t=nearest(np.asarray(point)[None],p.xy)
                if dist[0]>.25 or not -1e-5<=t[0]<=1+1e-5:
                    continue
                s=float(ss[i][k[0]]+np.clip(t[0],0,1)*(ss[i][k[0]+1]-ss[i][k[0]]))
                hits.append((i,s))
        hits=list(dict.fromkeys(hits))
        if len({plans[i][0] for i,_ in hits})<2:
            continue
        members[cid]=hits
        for i,s in hits:
            on_road.setdefault(i,[]).append((s,cid))
    neighbours={cid:[] for cid in members}
    for i,refs in on_road.items():
        ordered=sorted(refs)
        for (a,ca),(b,cb) in zip(ordered,ordered[1:]):
            neighbours[ca].append((cb,grade*(b-a)))
            neighbours[cb].append((ca,grade*(b-a)))
    height, floor, baseline, queue = {}, {}, {}, []
    for cid,refs in members.items():
        old=[]; seeds=[]
        for i,s in refs:
            p=plans[i][1]
            before=p.under if p.under is not None else p.z
            was=float(np.interp(s,ss[i],before)); now=float(np.interp(s,ss[i],p.z))
            old.append(was)
            if was-now>.1: seeds.append(now)
        baseline[cid]=min(old)
        floor[cid]=max(old)-max_drop
        if seeds:
            h=max(min(seeds),floor[cid]);height[cid]=h;heapq.heappush(queue,(h,cid))
    while queue:
        h,cid=heapq.heappop(queue)
        if h!=height[cid]:continue
        for other,step in neighbours[cid]:
            value=max(h+step,floor[other])
            if value<baseline[other]-.1 and value<height.get(other,np.inf)-1e-9:
                height[other]=value;heapq.heappush(queue,(value,other))
    out=list(plans)
    for i,refs in on_road.items():
        fid,p=plans[i]
        constraints=[(s,height[cid]) for s,cid in refs if cid in height]
        if not constraints:continue
        # Keep exact connector samples: interpolating a V through coarse samples
        # otherwise leaves up to 12cm between two incident roads.
        s=np.unique(np.r_[ss[i],[a for a,_ in constraints]])
        xy=np.stack([np.interp(s,ss[i],p.xy[:,c]) for c in range(p.xy.shape[1])],1)
        z=np.interp(s,ss[i],p.z)
        old=np.interp(s,ss[i],p.under if p.under is not None else p.z)
        for at,h in constraints:z=np.minimum(z,h+grade*np.abs(s-at))
        z=np.maximum(z,old-max_drop)
        q=Plan(xy,z,p.half)
        q.road_half,q.under,q.collar=p.road_half,old,p.collar
        q.junctions = tuple((np.asarray(joints[cid][0][1],float),
                             max(plans[ii][1].half for ii,_ in members[cid]))
                            for _,cid in refs if cid in height)
        out[i]=(fid,q)
    return out


class Geometry:
    def __init__(self, frame, cell_fn):
        self.frame, self.cell_fn, self.cache = frame, cell_fn, {}

    def info(self, feats, i, j):
        import roads
        fid = str(feats.ids[i])
        key = fid, j
        found = self.cache.get(key)
        if found is not None:
            return found
        ring = roads.bridge_parts(feats, i)[j]
        spec = roads.bridge_specs(feats)[i]
        out = {'fid': fid, 'ring': ring, 'collars': [], 'lower': set(), 'width': spec.width}
        xy = np.stack(mapgeom.enu_xy(feats, self.frame, ring), 1)
        length = arc(xy)[-1]
        if spec.cls not in ('motorway', 'trunk') or not 12. <= length <= MAX_SPAN:
            return self._keep(key, out)
        # Canonical intersected cells, independent of the first requested mesh tile.
        lat, lon = ring[:, 1], ring[:, 0]
        cells = []
        keys = mapcells.cells_for_bbox(float(lat.min())-.0003, float(lon.min())-.0005,
                                      float(lat.max())+.0003, float(lon.max())+.0005)
        for ck in sorted(keys, key=str):
            f = self.cell_fn(ck)
            if f is not None:
                cells.append(f)
        if not cells:
            cells = [feats]
        own_connectors = {cid: p for cid, _, p in roads._connector_points(feats, i)}
        lower, seen, joined = [], set(), False
        for f in cells:
            rs, bs = roads.road_specs(f), roads.bridge_specs(f)
            for ii in range(len(f)):
                other = str(f.ids[ii])
                if other in seen:
                    continue
                seen.add(other)
                if bs[ii] is not None:
                    for jj, r in enumerate(roads.bridge_parts(f, ii)):
                        if (other, jj) == key:
                            continue
                        ln = np.stack(mapgeom.enu_xy(f, self.frame, r), 1)
                        if np.linalg.norm(xy[[0,-1], None]-ln[None, [0,-1]], axis=2).min() < .75:
                            joined = True
                if rs[ii] is None or other == fid:
                    continue
                common = own_connectors.keys() & {cid for cid, _, _ in roads._connector_points(f, ii)}
                for r in roads.ground_parts(f, ii):
                    ln = np.stack(mapgeom.enu_xy(f, self.frame, r), 1)
                    for _, at, point in roads._crossings([xy], [ln]):
                        if not 6. <= at <= length-6.:
                            continue
                        connected = False
                        for cid in common:
                            p = own_connectors[cid]
                            q = np.array(self.frame.geodetic_to_enu(p[1], p[0], 0.)[:2])
                            connected |= np.linalg.norm(q-point) < .75
                        if not connected:
                            sp = rs[ii]
                            width = max(sp.width, roads.carriageway(sp.cls, sp.kind, True, None, sp.oneway)) if sp.kind in roads.BUILT else sp.width
                            lower.append((other, ln, width/2))
                            out['lower'].add(other)
        # Connected spans are solved with their source road graph. They must
        # retain crossing requirements rather than silently exempting a chain.
        tight = np.zeros(2, bool)
        for _, ln, half in lower:
            dist, _, _ = nearest(xy[[0,-1]], ln)
            tight |= dist < half + spec.width/2 + GRID_SUPPORT
        extended = ring.copy()
        for end, on in zip((0, -1), tight):
            if not on:
                continue
            for g in roads.ground_parts(feats, i):
                gx = np.stack(mapgeom.enu_xy(feats, self.frame, g), 1)
                if np.linalg.norm(gx[0]-xy[end]) < .25:
                    run = g
                elif np.linalg.norm(gx[-1]-xy[end]) < .25:
                    run = g[::-1]
                else:
                    continue
                if arc(gx)[-1] <= COLLAR_M + 4.:
                    break  # do not extend across a source junction or another span
                collar, _ = cut_run(self.frame, run, COLLAR_M)
                # A ground part can contain interior source connectors. Its
                # length alone does not prove this continuation is unbranched.
                # Leave such junctions on the ground instead of silently
                # converting their upper road into an elevated collar.
                if own_connectors:
                    ll = np.asarray(list(own_connectors.values()))
                    cx = np.stack(self.frame.geodetic_to_enu(ll[:,1],ll[:,0],0.)[:2],1)
                    line = np.stack(mapgeom.enu_xy(feats,self.frame,collar),1)
                    dist,k,t = nearest(cx,line)
                    s = arc(line)
                    at = s[k]+np.clip(t,0,1)*np.diff(s)[k]
                    if np.any((dist<.25) & (t>=-1e-5) & (t<=1+1e-5) & (at>.25)):
                        break
                out['collars'].append(collar)
                extended = np.vstack([collar[:0:-1], extended]) if end == 0 else np.vstack([extended, collar[1:]])
                break
        out['ring'] = extended
        return self._keep(key, out)

    def _keep(self, key, value):
        if len(self.cache) >= 256:
            self.cache.clear()
        self.cache[key] = value
        return value

    def parts(self, feats, i):
        import roads
        if not ENABLED:
            return roads.bridge_parts(feats, i)
        return [self.info(feats, i, j)['ring'] for j in range(len(roads.bridge_parts(feats, i)))]

    def ground(self, feats, i):
        import roads
        parts = roads.ground_parts(feats, i)
        if not ENABLED:
            return parts
        if roads.bridge_specs(feats)[i] is None:
            return parts
        collars = [c for j in range(len(roads.bridge_parts(feats, i))) for c in self.info(feats, i, j)['collars']]
        if not collars:
            return parts
        result = []
        for g in parts:
            for c in collars:
                scale = np.array([111320.*np.cos(np.radians(c[0,1])), 111320.])
                if np.linalg.norm((g[0]-c[0])*scale) < .25:
                    _, g = cut_run(self.frame, g, COLLAR_M)
                elif np.linalg.norm((g[-1]-c[0])*scale) < .25:
                    _, g = cut_run(self.frame, g[::-1], COLLAR_M)
                    g = g[::-1]
            if len(g) >= 2:
                result.append(g)
        return result
