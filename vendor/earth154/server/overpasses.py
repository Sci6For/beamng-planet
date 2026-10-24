"""Raise connected overpass approaches without excavating the lower road.

The graph contains source connectors and same-way part boundaries, never mere
XY crossings. Bridge parts participate in continuity but are not terrain fills.
"""
import heapq
import numpy as np
from underpasses import arc, nearest


def propagate(plans, joints, seeds, grade=.05):
    """Least raise-only height envelope on the complete source road graph.

seeds are (source ID, ENU point, absolute height). Insert exact connector and
seed samples before solving so incident roads cannot disagree by interpolation.
    """
    from grading import Plan
    by_fid = {}
    ss = []
    for i, (fid, p) in enumerate(plans):
        by_fid.setdefault(fid, []).append(i)
        ss.append(arc(p.xy[:, :2]))
    samples = [list(s) for s in ss]

    def locations(fid, point):
        for i in by_fid.get(fid, ()):
            d, k, t = nearest(np.asarray(point)[None], plans[i][1].xy)
            if d[0] < .25 and -1e-6 <= t[0] <= 1+1e-6:
                at = float(ss[i][k[0]] + np.clip(t[0], 0, 1)*np.diff(ss[i])[k[0]])
                samples[i].append(at)
                yield i, at

    members = {cid: list(dict.fromkeys(hit for fid, point in refs for hit in locations(fid, point)))
               for cid, refs in sorted(joints.items())}
    starts = [(hit, height) for fid, point, height in seeds for hit in locations(fid, point)]
    axes = [np.unique(s) for s in samples]
    offsets = np.r_[0, np.cumsum([len(s) for s in axes])]
    parent = np.arange(offsets[-1])

    def node(hit):
        i, at = hit
        return int(offsets[i] + np.searchsorted(axes[i], at))

    def root(a):
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a

    for hits in members.values():
        if not hits:
            continue
        a = root(node(hits[0]))
        for hit in hits[1:]:
            b = root(node(hit))
            parent[b] = a
    roots = np.array([root(i) for i in range(offsets[-1])])
    baseline = np.concatenate([np.interp(s, ss[i], p.z) for i, ((_, p), s) in enumerate(zip(plans, axes))])
    edges = {}
    for i, s in enumerate(axes):
        for j, distance in enumerate(np.diff(s)):
            a, b = roots[offsets[i]+j:offsets[i]+j+2]
            if a != b:
                edges.setdefault(a, []).append((b, grade*distance))
                edges.setdefault(b, []).append((a, grade*distance))
    floor = np.full(len(parent), -np.inf)
    np.maximum.at(floor, roots, baseline)
    lowest = np.full(len(parent), np.inf)
    np.minimum.at(lowest, roots, baseline)
    height = np.full(len(parent), -np.inf)
    queue = []
    for hit, h in starts:
        n = roots[node(hit)]
        if h > height[n]:
            height[n] = h
            heapq.heappush(queue, (-h, n))
    while queue:
        neg, a = heapq.heappop(queue)
        h = -neg
        if h < height[a]-1e-9:
            continue
        # The shared source junction keeps the highest original incident height.
        # Only touched nodes propagate; unrelated hilly roads are unchanged.
        h = max(h, floor[a])
        height[a] = h
        for b, loss in edges.get(a, ()):
            target = h-loss
            if target > lowest[b]+1e-5 and target > height[b]+1e-9:
                height[b] = target
                heapq.heappush(queue, (-target, b))
    out = []
    for i, (fid, p) in enumerate(plans):
        s = axes[i]
        old = baseline[offsets[i]:offsets[i+1]]
        z = np.maximum(old, height[roots[offsets[i]:offsets[i+1]]])
        if np.max(z-old) < 1e-5:
            out.append((fid, p))
            continue
        xy = np.stack([np.interp(s, ss[i], p.xy[:, c]) for c in range(p.xy.shape[1])], 1)
        q = Plan(xy, z, p.half)
        q.road_half, q.raised = p.road_half, old
        q.junctions = tuple((np.asarray(joints[cid][0][1], float),
                            max(plans[j][1].half for j, _ in hits))
                           for cid, hits in members.items()
                           if any(j == i and np.isfinite(height[roots[node((j, at))]]) for j, at in hits))
        out.append((fid, q))
    return out
