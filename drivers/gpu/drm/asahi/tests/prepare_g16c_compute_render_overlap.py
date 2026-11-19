#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Build an overlap probe around the unchanged full compute/render C oracles.

Only add orchestration hooks and a render timestamp print. Original assertions,
outputs, guards and two-compute overlap requirements remain byte-for-byte.
"""
import argparse, gzip, hashlib, json, shlex, stat, subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence',type=Path,required=True)
p.add_argument('--base-initrd',type=Path,required=True)
p.add_argument('--base-expected',type=Path,required=True)
a=p.parse_args();tests=Path(__file__).resolve().parent;out=a.evidence
out.mkdir(parents=True,exist_ok=False)
peer=r'''
#include <sys/wait.h>
struct render_peer {pid_t pid;FILE *input,*output;uint64_t times[4];int recorded;};
static struct render_peer peer;
static uint64_t peer_clock_ns(void) {
 struct timespec now;CHECK(!clock_gettime(CLOCK_MONOTONIC,&now));
 return (uint64_t)now.tv_sec*1000000000ULL+(uint64_t)now.tv_nsec;
}
static void peer_until(const char *marker) {
 char line[1024];
 while(fgets(line,sizeof(line),peer.output)) {
  fputs(line,stdout);
  if(sscanf(line,"G16C_RENDER_HOLD_TIMESTAMPS TA=%"SCNu64",%"SCNu64" FR=%"SCNu64",%"SCNu64,
   &peer.times[0],&peer.times[1],&peer.times[2],&peer.times[3])==4)peer.recorded++;
  if(strstr(line,marker))return;
 }
 CHECK(!"render peer exited before marker");
}
static void peer_start(const char *fixture) {
 int in[2],output[2];CHECK(!pipe(in)&&!pipe(output));peer.pid=fork();CHECK(peer.pid>=0);
 if(!peer.pid) {
  CHECK(dup2(in[0],STDIN_FILENO)>=0&&dup2(output[1],STDOUT_FILENO)>=0);
  close(in[0]);close(in[1]);close(output[0]);close(output[1]);
  execl("/g16c_drm_render_overlap_probe","g16c_drm_render_overlap_probe",fixture,"--hold","--check-inputs",(char*)NULL);_exit(127);
 }
 close(in[0]);close(output[1]);peer.input=fdopen(in[1],"w");peer.output=fdopen(output[0],"r");CHECK(peer.input&&peer.output);
 peer_until("G16C_RENDER_HOLD_READY");
}
static void peer_warm(int fd,struct drm_asahi_submit submits[2],struct drm_syncobj_create done[2],
 unsigned char **seed,unsigned char **maps,unsigned char **expected,size_t *sizes,unsigned char *ts) {
 for(unsigned j=0;j<2;j++) {
  OK(fd,DRM_IOCTL_ASAHI_SUBMIT,&submits[j]);
  struct drm_syncobj_wait w={.handles=(uintptr_t)&done[j].handle,.count_handles=1,
   .timeout_nsec=peer_clock_ns()+15000000000ULL,.flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL};
  OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&w);
 }
 for(unsigned id=0;id<128;id++)if(seed[id])CHECK(!memcmp(maps[id],expected[id]?expected[id]:seed[id],sizes[id]));
 for(unsigned j=0;j<2;j++){uint64_t t[2];memcpy(t,ts+PAGE+64+j*32,16);CHECK(t[0]&&t[0]!=0xa5a5a5a5a5a5a5a5ULL&&t[1]>t[0]);}
 for(unsigned i=0;i<PAGE*3;i++)if(!(i>=PAGE+64&&i<PAGE+80)&&!(i>=PAGE+96&&i<PAGE+112))CHECK(ts[i]==0xa5);
 uint32_t handles[2]={done[0].handle,done[1].handle};
 struct drm_syncobj_array reset={.handles=(uintptr_t)handles,.count_handles=2};OK(fd,DRM_IOCTL_SYNCOBJ_RESET,&reset);
 for(unsigned id=0;id<128;id++)if(seed[id])memcpy(maps[id],seed[id],sizes[id]);
 memset(ts,0xa5,PAGE*3);puts("COMPUTE_RENDER_WARM_BINDINGS_PASS full_compute_outputs_and_timestamp_guards=1");
}
static void peer_kick(unsigned char *ts,unsigned phase) {
 (void)ts;CHECK(phase==0);
 CHECK(fputs("R\n",peer.input)>=0&&!fflush(peer.input));
 puts("COMPUTE_RENDER_PEER_KICK two_compute_commands_accepted=1");
}
static void peer_finish(uint64_t times[2][2],unsigned phase) {
 CHECK(phase==0);peer_until("G16C_RENDER_HOLD_OUTPUT_PASS");CHECK(peer.recorded==1);
 CHECK(peer.times[0]&&peer.times[1]>peer.times[0]&&peer.times[2]&&peer.times[3]>peer.times[2]);
 int ta=0,fragment=0;
 for(unsigned j=0;j<2;j++) {
  ta|=times[j][0]<peer.times[1]&&peer.times[0]<times[j][1];
  fragment|=times[j][0]<peer.times[3]&&peer.times[2]<times[j][1];
 }
 printf("COMPUTE_RENDER_PEER_TIMESTAMPS TA=%"PRIu64",%"PRIu64" FR=%"PRIu64",%"PRIu64" overlap_TA=%d overlap_FR=%d\n",peer.times[0],peer.times[1],peer.times[2],peer.times[3],ta,fragment);
 CHECK(ta||fragment);
 CHECK(fputs("Q\n",peer.input)>=0&&!fclose(peer.input));
 char line[1024];while(fgets(line,sizeof(line),peer.output))fputs(line,stdout);CHECK(!fclose(peer.output));
 int status;CHECK(waitpid(peer.pid,&status,0)==peer.pid);CHECK(WIFEXITED(status)&&WEXITSTATUS(status)==0);
 puts("COMPUTE_RENDER_OVERLAP_PASS actual_GPU_overlap=1 original_compute_and_render_full_outputs_guards_timestamps=1");
}
'''
original_compute=(tests/'g16c_drm_compute_overlap.c').read_text()
original_render=(tests/'g16c_drm_render.c').read_text()
main='int main(int argc,char **argv)\n';assert original_compute.count(main)==1
warm_anchor='    struct drm_asahi_sync dependency_syncs[2]={sync[0],sync[1]};'
assert original_compute.count(warm_anchor)==1
hook1='    int closed_fences[2]={-1,-1};'
hook2='        printf("OVERLAP_PHASE_PASS phase=%u actual_overlap=%d full_outputs_and_guards=1\\n",phase,'
assert original_compute.count(hook1)==1 and original_compute.count(hook2)==1
compute=original_compute.replace(main,peer+'\nstatic int compute_overlap_main(int argc,char **argv)\n')
compute=compute.replace(warm_anchor,'    peer_warm(fd,submits,done,seed,maps,expected,sizes,ts);\n'+warm_anchor)
compute=compute.replace(hook1,'    peer_kick(ts,phase);\n'+hook1).replace(hook2,'    peer_finish(times,phase);\n'+hook2)
wrapper='\nint main(int argc,char **argv){setbuf(stdout,NULL);CHECK(argc==3);peer_start(argv[2]);return compute_overlap_main(2,argv);}\n'
compute+=wrapper
restored=compute.removesuffix(wrapper).replace('    peer_warm(fd,submits,done,seed,maps,expected,sizes,ts);\n','').replace('    peer_kick(ts,phase);\n','').replace('    peer_finish(times,phase);\n','').replace(peer+'\nstatic int compute_overlap_main(int argc,char **argv)\n',main)
assert restored==original_compute
anchor='   puts("G16C_RENDER_HOLD_OUTPUT_PASS");'
assert original_render.count(anchor)==1
stamp='   printf("G16C_RENDER_HOLD_TIMESTAMPS TA=%llu,%llu FR=%llu,%llu\\n",(unsigned long long)stamps[0],(unsigned long long)stamps[1],(unsigned long long)stamps[2],(unsigned long long)stamps[3]);\n'
render=original_render.replace(anchor,stamp+anchor);assert render.replace(stamp,'')==original_render
host='lab@192.168.170.255';remote='/home/lab/linux-g16c-g17p-build/compute-render-overlap-v1'
subprocess.run(['ssh',host,'mkdir -p '+shlex.quote(remote)],check=True)
expected=json.loads(a.base_expected.read_text())['guest_expected_sha256'];binaries={}
for name,source in [('g16c_drm_compute_render_overlap',compute),('g16c_drm_render_overlap_probe',render)]:
 path=out/(name+'.c');path.write_text(source)
 subprocess.run(['scp','-q',str(path),host+':'+remote+'/'+path.name],check=True)
 cmd=['aarch64-linux-gnu-gcc','-O2','-Wall','-Wextra','-Werror','-static','-I/home/lab/linux-g16c-g17p/drivers/gpu/drm/asahi/tests','-I/home/lab/linux-g16c-g17p-build/usr/include/drm','-I/home/lab/linux-g16c-g17p-build/usr/include',remote+'/'+path.name,'-o',remote+'/'+name]
 subprocess.run(['ssh',host,shlex.join(cmd)],check=True)
 subprocess.run(['scp','-q',host+':'+remote+'/'+name,str(out/name)],check=True)
 elf=(out/name).read_bytes();assert elf[:4]==b'\x7fELF'and int.from_bytes(elf[18:20],'little')==183
 binaries[name]=elf;expected['/'+name]=hashlib.sha256(elf).hexdigest()
def files(raw):
 pos=0;result={}
 while pos+110<=len(raw):
  if raw[pos:pos+6]!=b'070701':assert raw[pos:pos+4]==b'\0'*4;pos+=4;continue
  f=[int(raw[pos+6+i*8:pos+14+i*8],16)for i in range(13)];size,namesize=f[6],f[11]
  name=raw[pos+110:pos+110+namesize-1].decode().lstrip('./');pos=(pos+110+namesize+3)&~3
  if name!='TRAILER!!!':result[name]=(f[1],raw[pos:pos+size])
  pos=(pos+size+3)&~3
 return result
def entry(name,body,ino,mode):
 n=name.encode()+b'\0';f=[ino,mode,0,0,1,0,len(body),0,0,0,0,len(n),0]
 b=b'070701'+b''.join(f'{v:08x}'.encode()for v in f)+n;b+=b'\0'*((-len(b))%4);b+=body;b+=b'\0'*((-len(b))%4);return b
base=a.base_initrd.read_bytes();old=files(gzip.decompress(base));assert all(n not in old for n in binaries)
extra=b''.join(entry(n,b,0x780000+i,stat.S_IFREG|0o755)for i,(n,b)in enumerate(binaries.items()))+entry('TRAILER!!!',b'',0,0)
image=base+gzip.compress(extra,mtime=0);(out/'initramfs.cpio.gz').write_bytes(image);new=files(gzip.decompress(image));assert all(new[n]==v for n,v in old.items())
for n,h in expected.items():assert hashlib.sha256(new[n.lstrip('/')][1]).hexdigest()==h,n
receipt=dict(guest_expected_sha256=expected,initrd_sha256=hashlib.sha256(image).hexdigest(),base_initrd_sha256=hashlib.sha256(base).hexdigest(),all_base_entries_unchanged=len(old),original_assertions_and_oracles_exact=True,original_source_sha256={n:hashlib.sha256((tests/n).read_bytes()).hexdigest()for n in ['g16c_drm_compute_overlap.c','g16c_drm_render.c']},generated_source_sha256={n:hashlib.sha256((out/(n+'.c')).read_bytes()).hexdigest()for n in binaries},GPU_execution=False)
(out/'preparation.json').write_text(json.dumps(receipt,indent=2)+'\n')
print('PASS strict ARM64 overlap probes; original C assertions/oracles exact;',len(expected),'guest inputs')
