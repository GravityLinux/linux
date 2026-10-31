"""Passive ALSA control-event subscriber. Contains no PCM API or DMA access."""
from pathlib import Path
import ctypes as c,ctypes.util,json,re,signal,subprocess as s,time
O=Path('/dev/shm/audio-stage-a');O.mkdir(exist_ok=True);halt=False

def stop(*a):
 global halt
 halt=True
signal.signal(signal.SIGTERM,stop);signal.signal(signal.SIGINT,stop)
lib=c.CDLL(ctypes.util.find_library('asound'));vp=c.c_void_p
sig={'snd_ctl_open':([c.POINTER(vp),c.c_char_p,c.c_int],c.c_int),'snd_ctl_close':([vp],c.c_int),'snd_ctl_subscribe_events':([vp,c.c_int],c.c_int),'snd_ctl_event_malloc':([c.POINTER(vp)],c.c_int),'snd_ctl_event_free':([vp],None),'snd_ctl_wait':([vp,c.c_int],c.c_int),'snd_ctl_read':([vp,vp],c.c_int),'snd_ctl_event_get_type':([vp],c.c_int),'snd_ctl_event_elem_get_name':([vp],c.c_char_p),'snd_ctl_event_elem_get_mask':([vp],c.c_uint),'snd_ctl_elem_value_malloc':([c.POINTER(vp)],c.c_int),'snd_ctl_elem_value_free':([vp],None),'snd_ctl_elem_value_set_numid':([vp,c.c_uint],None),'snd_ctl_elem_read':([vp,vp],c.c_int),'snd_ctl_elem_value_get_boolean':([vp,c.c_uint],c.c_int)}
for n,(args,ret)in sig.items():getattr(lib,n).argtypes=args;getattr(lib,n).restype=ret
ctl=vp();ev=vp();value=vp();assert lib.snd_ctl_open(c.byref(ctl),b'hw:AppleJ713',0)==0;assert lib.snd_ctl_subscribe_events(ctl,1)==0;assert lib.snd_ctl_event_malloc(c.byref(ev))==0;assert lib.snd_ctl_elem_value_malloc(c.byref(value))==0
controls=s.check_output(['amixer','-c','AppleJ713','controls'],text=True);nid=int(re.search(r"numid=(\d+),iface=CARD,name='Headphone Jack'",controls)[1]);lib.snd_ctl_elem_value_set_numid(value,nid)
def report(kind,**kw):
 assert lib.snd_ctl_elem_read(ctl,value)==0
 q=dict(kind=kind,epoch=time.time(),monotonic_ns=time.monotonic_ns(),inserted=bool(lib.snd_ctl_elem_value_get_boolean(value,0)),IRQ=[l for l in Path('/proc/interrupts').read_text().splitlines()if'cs42l84'in l],**kw)
 print(json.dumps(q),flush=True)
report('ready',card='AppleJ713',mode='ALSA control events only; no PCM or DMA')
(O/'JACK-READY').write_text(str(time.time()))
try:
 while not halt:
  n=lib.snd_ctl_wait(ctl,60000)
  if n<0:continue
  if n==0:continue
  if lib.snd_ctl_read(ctl,ev)<0:continue
  if lib.snd_ctl_event_get_type(ev)==0:
   name=lib.snd_ctl_event_elem_get_name(ev).decode()
   if name=='Headphone Jack':report('jack-event',name=name,mask=lib.snd_ctl_event_elem_get_mask(ev))
finally:lib.snd_ctl_elem_value_free(value);lib.snd_ctl_event_free(ev);lib.snd_ctl_close(ctl)
