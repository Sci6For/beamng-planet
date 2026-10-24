"""Optional OSM lane metadata. Exact way/version + geometry matching; never match by street name.

One persistent decision per source way/version keeps overlapping cells and workers consistent.
Failures are frozen too: missing data must never change the road under a moving car. These inputs
are included in mapdata's NPZ and the host's LAN snapshot. No HTTP requests from rendering code.
"""
import json
from contextlib import closing
import logging
from pathlib import Path
import re
import sqlite3
import time
from urllib.parse import urlencode
import numpy as np
import netfetch

REF = re.compile(r'w([1-9][0-9]*)@([1-9][0-9]*)\Z')
MAX_BATCH, MAX_ROWS = 512, 24000
LOG = logging.getLogger('earthd.roadlanes')

def count(value):
    if isinstance(value, str) and value.isascii() and value.isdigit():
        n=int(value)
        if 0 <= n <= 12: return n
    return None

def orientation(line, source):
    """Direction of a matching sub-line, or zero for stale/ambiguous/parallel geometry."""
    try:
        a=np.asarray(line,dtype=float)
        b=np.array([[p['lon'],p['lat']] for p in source],dtype=float)
        if len(a)<2 or len(b)<2 or len(b)>2000 or not np.isfinite(b).all(): return 0
        scale=np.array([111320*np.cos(np.radians(a[0,1])),111320])
        b=(b-a[0])*scale; a=(a-a[0])*scale
        d=np.diff(b,axis=0); l2=(d*d).sum(1); lengths=np.sqrt(l2)
        if lengths.sum()<1: return 0
        cumulative=np.r_[0,np.cumsum(lengths)]
        positions=[]
        for p in a[np.unique(np.linspace(0,len(a)-1,min(7,len(a))).astype(int))]:
            t=np.clip(((p-b[:-1])*d).sum(1)/np.maximum(l2,1e-12),0,1)
            dist=((b[:-1]+t[:,None]*d-p)**2).sum(1); j=int(np.argmin(dist))
            if dist[j]>36: return 0
            positions.append(cumulative[j]+t[j]*lengths[j])
        delta=np.diff(positions)
        if np.linalg.norm(b[-1]-b[0])<.1:
            delta=(delta+lengths.sum()/2)%lengths.sum()-lengths.sum()/2
        if abs(delta.sum())<1: return 0
        direction=1 if delta.sum()>0 else -1
        return direction if np.all(direction*delta>=-0.5) else 0
    except (KeyError,TypeError,ValueError,IndexError): return 0

def apply(feats, records):
    """Attach confirmed lane attributes; retain zero for unknown (never means a closed road)."""
    values={k:np.zeros(len(feats),dtype=np.int16) for k in ('osm_lanes','osm_forward','osm_backward','osm_oneway')}
    matched=0
    for i,ref in enumerate(feats.attrs.get('src_record',[])):
        ref=str(ref); match=REF.fullmatch(ref); item=records.get(ref)
        if not match or not isinstance(item,dict): continue
        if str(feats.attrs.get('src_dataset',['']*len(feats))[i])!='OpenStreetMap': continue
        if item.get('id')!=int(match[1]) or item.get('version')!=int(match[2]): continue
        tags=item.get('tags') or {}
        if not isinstance(tags,dict): continue
        n=count(tags.get('lanes'))
        forward,backward=count(tags.get('lanes:forward')),count(tags.get('lanes:backward'))
        one=tags.get('oneway') in ('yes','1','true','-1') or tags.get('junction')=='roundabout'
        if count(tags.get('lanes:both_ways')) not in (None,0): continue  # shared turn lanes need a different layout
        if n is None and forward is not None and backward is not None: n=forward+backward
        if n is None or not 1<=n<=12: continue
        if one: forward,backward=(0,n) if tags.get('oneway')=='-1' else (n,0)
        else:
            if forward is not None and backward is None: backward=n-forward
            if backward is not None and forward is None: forward=n-backward
            if forward is None and backward is None and n%2==0: forward=backward=n//2
            if forward is None or backward is None or min(forward,backward)<1 or forward+backward!=n: continue
        parts=feats.parts(i)
        if len(parts)!=1 or len(parts[0])!=1: continue
        direction=orientation(parts[0][0],item.get('geometry',[]))
        if not direction: continue
        if direction<0: forward,backward=backward,forward
        for k,v in zip(values,(n,forward,backward,int(one))): values[k][i]=v
        matched+=1
    feats.attrs.update(values)
    return matched

class Cache:
    def __init__(self, directory, fetcher):
        self.directory=Path(directory); self.fetcher=fetcher

    def get(self, refs):
        try:
            return self._get(refs)
        except (sqlite3.Error,OSError,ValueError) as e:
            LOG.warning('Optional lane cache unavailable; using estimated roads: %s',e)
            return {}

    def _get(self, refs):
        refs=sorted({str(r) for r in refs if REF.fullmatch(str(r))})
        if not refs: return {}
        self.directory.mkdir(parents=True,exist_ok=True)
        # The write transaction spans the bounded request: workers cannot observe a half-filled batch.
        with closing(sqlite3.connect(self.directory/'lanes-v1.sqlite',timeout=15)) as db, db:
            db.execute('CREATE TABLE IF NOT EXISTS ways(ref TEXT PRIMARY KEY, data TEXT)')
            db.execute('CREATE TABLE IF NOT EXISTS state(key TEXT PRIMARY KEY, value REAL)')
            db.execute('BEGIN IMMEDIATE')
            result={}; missing=[]
            for ref in refs:
                row=db.execute('SELECT data FROM ways WHERE ref=?',(ref,)).fetchone()
                if row is None: missing.append(ref)
                elif row[0]: result[ref]=json.loads(row[0])
            state=dict(db.execute('SELECT key,value FROM state'))
            now=time.time(); day=int(now//86400)
            used=state.get('used',0) if state.get('day')==day else 0
            room=max(0,MAX_ROWS-db.execute('SELECT count(*) FROM ways').fetchone()[0])
            byte_count=db.execute('SELECT coalesce(sum(length(data)),0) FROM ways').fetchone()[0]
            # Hard row cap bounds the cache. No eviction of decisions used by already-generated cells.
            pending=missing[:room]
            ask=pending[:MAX_BATCH] if used<100 and now>=state.get('next',0) else []
            if ask:
                ids=','.join(str(int(REF.fullmatch(r)[1])) for r in ask)
                query=f'[out:json][timeout:2][maxsize:8388608];way(id:{ids})[lanes];out meta geom;'
                next_time=now+5
                try:
                    response=self.fetcher.get('https://overpass-api.de/api/interpreter?'+urlencode({'data':query}),kind='demand')
                    if len(response.body)>4*1024*1024: raise ValueError('lane response too large')
                    data=json.loads(response.body)
                    if not isinstance(data,dict) or not isinstance(data.get('elements'),list):
                        raise ValueError('invalid lane response')
                    for item in data.get('elements',[]):
                        if not isinstance(item,dict) or item.get('type')!='way': continue
                        ref=f"w{item.get('id')}@{item.get('version')}"
                        geometry=item.get('geometry'); tags=item.get('tags')
                        if ref in ask and isinstance(geometry,list) and len(geometry)<=2000 and isinstance(tags,dict):
                            # No contributor identifiers are retained.
                            compact={k:item.get(k) for k in ('id','version','geometry')}
                            compact['tags']={k:v for k,v in tags.items() if k in
                                {'highway','lanes','lanes:forward','lanes:backward','lanes:both_ways','oneway','junction'}}
                            size=len(json.dumps(compact))
                            if size<12000 and byte_count+size<16*1024*1024:
                                result[ref]=compact; byte_count+=size
                except (netfetch.FetchError,OSError,ValueError,TypeError) as e:
                    next_time=now+600; LOG.info('OSM lanes unavailable; keeping estimated roads: %s',e)
                for k,v in {'day':day,'used':used+1,'next':next_time}.items():
                    db.execute('INSERT OR REPLACE INTO state VALUES (?,?)',(k,v))
            for ref in pending:
                db.execute('INSERT INTO ways VALUES (?,?)',(ref,json.dumps(result[ref],separators=(',',':')) if ref in result else None))
            return result
