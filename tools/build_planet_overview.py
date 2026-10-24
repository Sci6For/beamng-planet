"""Build the offline Mercator overview from the audited Natural Earth snapshot."""
import hashlib
import json
import math
from pathlib import Path
import sys

EXPECTED='6866c877d39cba9c357620878839b336d569f8c662d3cfab4cb1dbe2d39c977f'

def build(source, destination):
    data=Path(source).read_bytes()
    if hashlib.sha256(data).hexdigest()!=EXPECTED:
        raise ValueError('Natural Earth source hash mismatch')
    features=json.loads(data)['features']; paths=[]; places=[]
    def project(lon,lat):
        phi=math.radians(max(-85.05112878,min(85.05112878,lat)))
        return (lon+180)/360*256,(1-math.log(math.tan(phi)+1/math.cos(phi))/math.pi)/2*256
    for feature in features:
        p=feature['properties']; g=feature['geometry']
        polygons=[g['coordinates']] if g['type']=='Polygon' else g['coordinates']
        for polygon in polygons:
            parts=[]
            for ring in polygon:
                points=[project(*point[:2]) for point in ring]
                parts.append('M'+'L'.join(f'{x:.2f},{y:.2f}' for x,y in points)+'Z')
            paths.append('<path d="'+''.join(parts)+'"/>')
        name=p.get('NAME_RU') or p['NAME_EN']; lon=p['LABEL_X']; lat=p['LABEL_Y']
        places.append(dict(name=name,english=p['NAME_EN'],lat=lat,lon=lon))
    destination=Path(destination); destination.mkdir(parents=True,exist_ok=True)
    svg='<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256"><title>Мир — Natural Earth</title><desc>Natural Earth public domain; Mercator overview. Source SHA256 '+EXPECTED+'</desc><rect width="256" height="256" fill="#234654"/><g fill="#61776d" stroke="#a7b3a1" stroke-width=".12" fill-rule="evenodd">'+''.join(paths)+'</g></svg>'
    (destination/'world-overview.svg').write_text(svg,encoding='utf-8')
    (destination/'places.mjs').write_text('// Natural Earth public domain, source hash '+EXPECTED+'\nexport default '+json.dumps(places,ensure_ascii=False,separators=(',',':'))+'\n',encoding='utf-8')

if __name__=='__main__': build(*sys.argv[1:])
