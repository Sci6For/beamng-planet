import {wrapLon} from './mapModel.mjs'

const radians=Math.PI/180, degrees=180/Math.PI
export const isGlobeZoom=z=>z<3

export function forwardGlobe(lat,lon,center) {
  const phi=lat*radians,phi0=center.lat*radians,delta=wrapLon(lon-center.lon)*radians
  const sin=Math.sin(phi),cos=Math.cos(phi),sin0=Math.sin(phi0),cos0=Math.cos(phi0)
  const depth=sin0*sin+cos0*cos*Math.cos(delta)
  return {x:cos*Math.sin(delta),y:-(cos0*sin-sin0*cos*Math.cos(delta)),visible:depth>=-1e-12}
}

export function inverseGlobe(x,y,center) {
  const rho=Math.hypot(x,y)
  if(rho>1)return null
  if(rho<1e-12)return {...center}
  const c=Math.asin(rho),sin=Math.sin(c),cos=Math.cos(c),phi0=center.lat*radians
  const lat=Math.asin(cos*Math.sin(phi0)-y*sin*Math.cos(phi0)/rho)*degrees
  const lon=wrapLon(center.lon+Math.atan2(x*sin,rho*Math.cos(phi0)*cos+y*Math.sin(phi0)*sin)*degrees)
  return {lat,lon}
}

export function rotateGlobe(center,dx,dy,radius) {
  if(!dx&&!dy)return {...center}
  const degreesPerPixel=90/Math.max(radius,1)
  return {lat:Math.max(-85,Math.min(85,center.lat+dy*degreesPerPixel)),lon:wrapLon(center.lon-dx*degreesPerPixel)}
}

export function globeLabels(places,center,radius) {
  const out=[]
  for(const p of places) {
    const position=forwardGlobe(p.lat,p.lon,center)
    if(!position.visible || Math.hypot(position.x,position.y)>.9)continue
    const name=p.english||p.name
    if(!name)continue
    const x=position.x*radius,y=position.y*radius,width=name.length*6+8
    if(Math.abs(x)+width/2>radius*.94 || Math.abs(y)>radius*.88)continue
    if(out.some(label=>Math.abs(label.x-x)<(label.width+width)/2&&Math.abs(label.y-y)<17))continue
    out.push({name,x,y,width})
    if(out.length>=24)break
  }
  return out
}

const renderers=new WeakMap()
let cachedImage,cachedSource,cachedPixels
function sourceTexture(image) {
  if(cachedImage===image)return cachedSource
  const source=document.createElement('canvas');source.width=512;source.height=512
  source.getContext('2d').drawImage(image,0,0,512,512)
  cachedImage=image;cachedSource=source;cachedPixels=null
  return source
}

// Orthographic inverse projection runs in a fragment shader. The UI thread only
// updates two center uniforms; no trigonometry or allocations per screen pixel.
function gpuRenderer(canvas) {
  const surface=document.createElement('canvas')
  const gl=surface.getContext('webgl',{alpha:true,antialias:false,depth:false,stencil:false,premultipliedAlpha:false})
  if(!gl)return null
  const shader=(type,source)=>{
    const value=gl.createShader(type);gl.shaderSource(value,source);gl.compileShader(value)
    if(!gl.getShaderParameter(value,gl.COMPILE_STATUS)){gl.deleteShader(value);throw new Error('globeShader')}
    return value
  }
  try {
    const vertex=shader(gl.VERTEX_SHADER,'attribute vec2 position; varying vec2 point; void main(){point=position;gl_Position=vec4(position,0.,1.);}')
    const fragment=shader(gl.FRAGMENT_SHADER,`precision highp float;
      varying vec2 point; uniform sampler2D world; uniform vec3 center;
      void main(){
        float r2=dot(point,point);if(r2>1.){gl_FragColor=vec4(0.);return;}
        float depth=sqrt(1.-r2);
        float s=clamp(depth*center.x+point.y*center.y,-0.9962720762,0.9962720762);
        float u=fract(.5+center.z+atan(point.x,depth*center.y-point.y*center.x)/6.28318530718);
        float v=.5-log((1.+s)/(1.-s))/12.56637061436;
        gl_FragColor=vec4(texture2D(world,vec2(u,v)).rgb*(.73+.27*depth),1.);
      }`)
    const program=gl.createProgram();gl.attachShader(program,vertex);gl.attachShader(program,fragment);gl.linkProgram(program)
    gl.deleteShader(vertex);gl.deleteShader(fragment)
    if(!gl.getProgramParameter(program,gl.LINK_STATUS))throw new Error('globeProgram')
    gl.useProgram(program)
    const buffer=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,buffer)
    gl.bufferData(gl.ARRAY_BUFFER,new Float32Array([-1,-1,1,-1,-1,1,-1,1,1,-1,1,1]),gl.STATIC_DRAW)
    const position=gl.getAttribLocation(program,'position');gl.enableVertexAttribArray(position);gl.vertexAttribPointer(position,2,gl.FLOAT,false,0,0)
    const uniform=gl.getUniformLocation(program,'center'),texture=gl.createTexture();gl.bindTexture(gl.TEXTURE_2D,texture)
    gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MIN_FILTER,gl.LINEAR);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MAG_FILTER,gl.LINEAR)
    gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_WRAP_S,gl.REPEAT);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_WRAP_T,gl.CLAMP_TO_EDGE)
    let imageUsed
    return {get lost(){return gl.isContextLost()},dispose(){gl.deleteTexture(texture);gl.deleteBuffer(buffer);gl.deleteProgram(program);gl.getExtension('WEBGL_lose_context')?.loseContext()},draw(image,center,d){
      if(gl.isContextLost())return false
      if(surface.width!==d||surface.height!==d){surface.width=d;surface.height=d;gl.viewport(0,0,d,d)}
      if(imageUsed!==image){gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,gl.RGBA,gl.UNSIGNED_BYTE,sourceTexture(image));imageUsed=image}
      const phi=center.lat*radians;gl.uniform3f(uniform,Math.sin(phi),Math.cos(phi),center.lon/360)
      gl.drawArrays(gl.TRIANGLES,0,6)
      canvas.getContext('2d').clearRect(0,0,d,d);canvas.getContext('2d').drawImage(surface,0,0)
      return true
    }}
  } catch {gl.getExtension('WEBGL_lose_context')?.loseContext();return null}
}

// CEF may disable WebGL. Reuse a bounded pixel buffer and hemisphere lookup;
// center trigonometry is calculated once, not by inverseGlobe per pixel.
function softwareFrame(renderer,image,center,d) {
  const source=sourceTexture(image)
  if(!cachedPixels)cachedPixels=source.getContext('2d',{willReadFrequently:true}).getImageData(0,0,512,512).data
  if(renderer.d!==d) {
    const frame=renderer.ctx.createImageData(d,d),points=[]
    for(let y=0;y<d;y++)for(let x=0;x<d;x++) {
      const nx=(2*x+1)/d-1,ny=1-(2*y+1)/d,r2=nx*nx+ny*ny
      if(r2<=1)points.push((y*d+x)*4,nx,ny,Math.sqrt(1-r2))
    }
    renderer.d=d;renderer.frame=frame;renderer.points=new Float32Array(points)
  }
  const out=renderer.frame.data,pixels=cachedPixels,points=renderer.points
  const sin=Math.sin(center.lat*radians),cos=Math.cos(center.lat*radians),lon=center.lon/360+.5
  for(let i=0;i<points.length;i+=4) {
    const to=points[i],x=points[i+1],y=points[i+2],z=points[i+3]
    const s=Math.max(-.9962720762,Math.min(.9962720762,z*sin+y*cos))
    const u=lon+Math.atan2(x,z*cos-y*sin)/(2*Math.PI)
    const sx=Math.floor((u-Math.floor(u))*512),sy=Math.max(0,Math.min(511,Math.floor((.5-Math.log((1+s)/(1-s))/(4*Math.PI))*512)))
    const from=(sy*512+sx)*4,light=.73+.27*z
    out[to]=pixels[from]*light;out[to+1]=pixels[from+1]*light;out[to+2]=pixels[from+2]*light;out[to+3]=255
  }
  renderer.ctx.putImageData(renderer.frame,0,0)
}

export function drawGlobe(canvas,image,center,diameter) {
  if(!canvas || !image?.complete || !image.naturalWidth)return
  let renderer=renderers.get(canvas)
  if(!renderer){renderer={ctx:canvas.getContext('2d'),gpu:gpuRenderer(canvas)};renderers.set(canvas,renderer)}
  if(renderer.gpu?.lost){renderer.gpu.dispose();renderer.gpu=null;renderer.image=null}
  const d=Math.max(1,Math.min(renderer.gpu?600:240,Math.round(diameter)))
  if(renderer.image===image&&renderer.lat===center.lat&&renderer.lon===center.lon&&renderer.size===d)return
  if(canvas.width!==d)canvas.width=d
  if(canvas.height!==d)canvas.height=d
  if(!renderer.gpu?.draw(image,center,d))softwareFrame(renderer,image,center,Math.min(d,240))
  renderer.image=image;renderer.lat=center.lat;renderer.lon=center.lon;renderer.size=d
}

export function disposeGlobe(canvas) {
  renderers.get(canvas)?.gpu?.dispose()
  renderers.delete(canvas)
}

