"""Verified LAN envelopes. Generated resources are data; scripts never enter this graph."""
import hashlib
import json
import os
import re
from pathlib import Path

CHUNK = 256 * 1024
MAX_ARTIFACT = 128 * 1024 * 1024
MAX_ENVELOPE = 4 * 1024 * 1024
MAX_RESOURCES = 512
ALLOWED = {'.raw', '.ter', '.png', '.dds', '.jpg', '.jpeg', '.json', '.cdae', '.dae'}
KINDS = set('hello status anchor_ready terrain_ready anchor_error terrain_error roads_tile roads_drop veg_tile veg_drop bldfar_tile bldfar_drop far_tile far_remove far_swap far_done far_retex block_base sea ground_ahead navgraph credits weather gravity cache route_result where_result geocode_result map_tiles_ready place_previews_ready random_place_result'.split())
PATH_FIELDS = set('path shape texture heightmap ter layers base prewarm diffuseMap normalMap detailMap colorMap opacityMap'.split())

class ResourceRejected(ValueError): pass

def digest_file(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for b in iter(lambda: f.read(CHUNK), b''): h.update(b)
    return h.hexdigest()

class EnvelopeBuilder:
    def __init__(self, profile, room, epoch):
        if not re.fullmatch('[0-9a-f]{32}', room): raise ResourceRejected('room')
        self.profile = Path(profile).resolve()
        self.room, self.epoch = room, str(epoch)
        self.root = self.profile / 'temp/beamngEarth/planet' / room
        self.root.mkdir(parents=True, exist_ok=True)
        self.files = {}
        self.cache = {}

    def _path(self, text):
        if not text.startswith('/temp/beamngEarth/') or '\\' in text or ':' in text:
            raise ResourceRejected('resource must be generated VFS data')
        parts = text.lstrip('/').split('/')
        if any(p in ('', '.', '..') for p in parts): raise ResourceRejected('unsafe resource path')
        p = self.profile.joinpath(*parts)
        for parent in (p, *p.parents):
            if parent == self.profile: break
            if parent.is_symlink() or (hasattr(parent,'is_junction') and parent.is_junction()):
                raise ResourceRejected('resource link/reparse point')
        if not p.resolve().is_relative_to(self.profile): raise ResourceRejected('resource escape')
        if p.suffix.lower() not in ALLOWED: raise ResourceRejected('executable/unknown artifact')
        if not p.is_file() or p.stat().st_size > MAX_ARTIFACT: raise ResourceRejected('missing/oversize artifact')
        return p

    def envelope(self, payload, frame_epoch):
        if payload.get('type') not in KINDS: raise ResourceRejected('unknown Earth physical message')
        manifests, visiting, collectors = {}, set(), []
        terrain=payload.get("terrain",payload)
        physical_bounds=[0.0]*6
        if "heightmap" in terrain:
            half=float(terrain["size"])*float(terrain["squareSize"])/2
            cx,cy=float(terrain["cx"]),float(terrain["cy"])
            z=float(terrain["zBase"])
            physical_bounds=[cx-half,cy-half,z,cx+half,cy+half,z+float(terrain["maxHeight"])]
        def rewrite(v, key=''):
            if isinstance(v, dict): return {k:rewrite(x,k) for k,x in v.items()}
            if isinstance(v, list): return [rewrite(x,key) for x in v]
            if isinstance(v,str):
                if v.startswith('/temp/beamngEarth/') or (key in PATH_FIELDS and (v.startswith(('/', '..')) or ':' in v)):
                    # Packaged immutable assets are identified by the agreed client fingerprint.
                    if v.startswith(('/levels/beamng_earth/', '/art/')) and '..' not in v.split('/'): return v
                    return resource(v)
                if key in PATH_FIELDS and (v.startswith('..') or re.match('[A-Za-z]:',v)):
                    raise ResourceRejected('unsafe path')
            return v
        def resource(vfs):
            p = self._path(vfs)
            if p in visiting: raise ResourceRejected('resource dependency cycle')
            visiting.add(p)
            collectors.append(set())
            suffix = p.suffix.lower()
            data = None
            if suffix == '.json':
                if p.stat().st_size > 8*1024*1024: raise ResourceRejected('JSON artifact parse bound')
                data = json.dumps(rewrite(json.loads(p.read_text(encoding='utf-8'))),separators=(',',':'),ensure_ascii=False,allow_nan=False).encode()
            elif suffix == '.dae':
                # Generated Collada URI elements must be explicit generated or packaged paths.
                import xml.etree.ElementTree as ET
                doc = ET.parse(p)
                for node in doc.iter():
                    if node.tag.rsplit('}',1)[-1] == 'init_from' and node.text:
                        node.text = rewrite(node.text,'path')
                        if not node.text.startswith(('/temp/beamngEarth/','/levels/beamng_earth/','/art/')):
                            raise ResourceRejected('relative Collada dependency')
                data = ET.tostring(doc.getroot())
            elif suffix == '.cdae':
                # Generated TSShape materials contain identifiers, not texture paths. Parse every
                # msgpack value, including names/materials, so new embedded paths cannot be missed.
                import struct, cdae
                raw = p.read_bytes()
                if len(raw) >= 8 and struct.unpack_from('<I',raw)[0] == cdae.VERSION_WORD:
                    n = struct.unpack_from('<I',raw,4)[0]
                    header, end = cdae.unpack_one(raw,8)
                    if end != 8+n or header.get('compression'): raise ResourceRejected('unsupported CDAE')
                    values=[]; i=end
                    while i<len(raw):
                        val,i=cdae.unpack_one(raw,i); values.append(rewrite(val,'path') if isinstance(val,str) and '/' in val else val)
                    body=cdae.packb(*values); header['bodysize']=len(body); head=cdae.packb(header)
                    data=struct.pack('<II',cdae.VERSION_WORD,len(head))+head+body
                else: raise ResourceRejected('unknown CDAE encoding')
            sha = hashlib.sha256(data).hexdigest() if data is not None else digest_file(p)
            name = sha+suffix
            dest = self.root/name
            if not dest.exists():
                tmp = dest.with_name(dest.name+'.'+os.urandom(8).hex()+'.tmp')
                if data is not None: tmp.write_bytes(data)
                else:
                    import shutil
                    with p.open('rb') as src, tmp.open('wb') as dst: shutil.copyfileobj(src,dst,CHUNK)
                try: os.link(tmp,dest)
                except FileExistsError: pass
                finally: tmp.unlink(missing_ok=True)
            size=dest.stat().st_size
            if size>MAX_ARTIFACT: raise ResourceRejected('rewritten artifact exceeds bound')
            self.files[sha]=str(dest)
            manifests[sha]={'sha256':sha,'byte_length':str(size),'kind':('Terrain' if suffix=='.ter' or p.name.startswith('hm_') and suffix=='.png' else 'GroundModel' if suffix=='.png' and 'heightmap' in terrain and p.name.startswith('hm_') else 'Visual' if suffix in ('.png','.dds','.jpg','.jpeg') else 'Collision'),
                'dependencies':sorted(collectors.pop()),'world_epoch':self.epoch,'frame_epoch':str(frame_epoch),
                'logical_key':name,'bounds':physical_bounds,'surface_semantic_digest':None}
            if collectors: collectors[-1].add(sha)
            visiting.remove(p)
            if len(manifests)>MAX_RESOURCES: raise ResourceRejected('resource count')
            return f'/temp/beamngEarth/planet/{self.room}/{name}'
        out={'room':self.room,'world_epoch':self.epoch,'frame_epoch':str(frame_epoch),'payload':rewrite(payload),'resources':list(manifests.values())}
        if len(json.dumps(out).encode())>MAX_ENVELOPE: raise ResourceRejected('envelope byte bound')
        return out
