#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Split the existing compute/indirect-render oracle into dependent ioctls.

Run first after boot with cpu_cache_pause_grid=32. A completed render must be
CPU-visible while its producer's public fence remains pending. All original
argument/color/query/guard/timestamp assertions are preserved exactly.
"""
import argparse,gzip,hashlib,json,shlex,stat,struct,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);p.add_argument('--base-initrd',type=Path,required=True);p.add_argument('--base-expected',type=Path,required=True);a=p.parse_args()
tests=Path(__file__).resolve().parent;out=a.evidence;out.mkdir(parents=True,exist_ok=False)
helper=r'''
#include <linux/sync_file.h>
static uint64_t dependency_clock(void){struct timespec t;assert(!clock_gettime(CLOCK_MONOTONIC,&t));return(uint64_t)t.tv_sec*1000000000ULL+(uint64_t)t.tv_nsec;}
static void dependency_ioctl(int fd,unsigned long request,void *arg){int rc=g17p_raw_ioctl(fd,request,arg);if(rc)fprintf(stderr,"DEPENDENCY_IOCTL_FAIL request=%#lx fd=%d rc=%d errno=%d %s\n",request,fd,rc,errno,strerror(errno));assert(rc==0);}
static void dependency_submit(int fd,const struct drm_asahi_submit *all,unsigned char *timestamps,unsigned generation){
 assert(all->in_sync_count==0&&all->out_sync_count==1&&all->cmdbuf_size>72);
 struct drm_syncobj_create producer={0},consumer={0};dependency_ioctl(fd,DRM_IOCTL_SYNCOBJ_CREATE,&producer);dependency_ioctl(fd,DRM_IOCTL_SYNCOBJ_CREATE,&consumer);
 struct drm_asahi_sync ps={.handle=producer.handle};struct drm_asahi_sync outs[2]={*(struct drm_asahi_sync*)(uintptr_t)all->syncs,{.handle=consumer.handle}};
 struct drm_asahi_submit cs=*all;cs.cmdbuf_size=72;cs.syncs=(uintptr_t)&ps;
 unsigned char *draw=malloc(all->cmdbuf_size-72);assert(draw);memcpy(draw,(void*)(uintptr_t)(all->cmdbuf+72),all->cmdbuf_size-72);
 unsigned offset=0,renders=0;
 while(offset<all->cmdbuf_size-72){
  assert(all->cmdbuf_size-72-offset>=sizeof(struct drm_asahi_cmd_header));
  struct drm_asahi_cmd_header *h=(void*)(draw+offset);
  assert(sizeof(*h)+h->size<=all->cmdbuf_size-72-offset);
  if(h->cmd_type==DRM_ASAHI_CMD_RENDER){
   assert(h->cdm_barrier==1);h->cdm_barrier=0;renders++;
  }
  offset+=sizeof(*h)+h->size;
 }
 assert(renders==1&&offset==all->cmdbuf_size-72);
 /* Preserve intervening metadata commands. Only the render dependency now
  * names accepted queue history instead of this ioctl's command array. */
 struct drm_asahi_submit rs=*all;rs.cmdbuf=(uintptr_t)draw;rs.cmdbuf_size-=72;rs.syncs=(uintptr_t)outs;rs.out_sync_count=2;
 dependency_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&cs);dependency_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&rs);
 struct drm_syncobj_handle export={.handle=producer.handle,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};dependency_ioctl(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&export);
 if(generation==1){
  volatile uint64_t *end=(void*)(timestamps+184);uint64_t deadline=dependency_clock()+5000000000ULL;
  struct sync_file_info info={0};
  while(*end==0||*end==0xa5a5a5a5a5a5a5a5ULL){
   info=(struct sync_file_info){0};dependency_ioctl(export.fd,SYNC_IOC_FILE_INFO,&info);assert(info.status==0);assert(dependency_clock()<deadline);struct timespec delay={.tv_nsec=100000};assert(!nanosleep(&delay,NULL));
  }
  info=(struct sync_file_info){0};dependency_ioctl(export.fd,SYNC_IOC_FILE_INFO,&info);assert(info.status==0);
  printf("RENDER_GPU_DEPENDENCY_EARLY_PASS consumer_GPU_completed=1 producer_public_fence=0 fragment_end=%"PRIu64"\n",*end);fflush(stdout);
 }
 struct drm_syncobj_wait wait={.handles=(uintptr_t)&consumer.handle,.count_handles=1,.timeout_nsec=dependency_clock()+15000000000ULL,.flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL};dependency_ioctl(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
 struct sync_file_info info={0};info=(struct sync_file_info){0};dependency_ioctl(export.fd,SYNC_IOC_FILE_INFO,&info);assert(info.status==1);assert(!close(export.fd));
 struct drm_syncobj_destroy d={.handle=producer.handle};dependency_ioctl(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&d);d.handle=consumer.handle;dependency_ioctl(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&d);free(draw);
}
'''
original=(tests/'g16c_drm_parameter_growth.c').read_text();anchor='int main(int argc,char **argv) {';assert original.count(anchor)==1
call='        checked_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit);';replacement='        dependency_submit(fd,&submit,stamp_map,generation);';assert original.count(call)==1
source=original.replace(anchor,helper+'\n'+anchor).replace(call,replacement)
assert source.replace(helper+'\n','').replace(replacement,call)==original
name='g16c_drm_render_dependency';(out/(name+'.c')).write_text(source)
host='lab@192.168.170.255';remote='/home/lab/linux-g16c-g17p-build/'+name+'-v1'
subprocess.run(['scp','-q',str(out/(name+'.c')),host+':'+remote+'.c'],check=True)
cmd=['aarch64-linux-gnu-gcc','-O2','-Wall','-Wextra','-Werror','-static','-I/home/lab/linux-g16c-g17p/drivers/gpu/drm/asahi/tests','-I/home/lab/linux-g16c-g17p-build/usr/include/drm','-I/home/lab/linux-g16c-g17p-build/usr/include',remote+'.c','-o',remote]
subprocess.run(['ssh',host,shlex.join(cmd)],check=True);subprocess.run(['scp','-q',host+':'+remote,str(out/name)],check=True)
elf=(out/name).read_bytes();assert elf[:4]==b'\x7fELF'and int.from_bytes(elf[18:20],'little')==183
def files(raw):
 pos=0;result={}
 while pos+110<=len(raw):
  if raw[pos:pos+6]!=b'070701':assert raw[pos:pos+4]==b'\0'*4;pos+=4;continue
  f=[int(raw[pos+6+i*8:pos+14+i*8],16)for i in range(13)];size,namesize=f[6],f[11];name=raw[pos+110:pos+110+namesize-1].decode().lstrip('./');pos=(pos+110+namesize+3)&~3
  if name!='TRAILER!!!':result[name]=(f[1],raw[pos:pos+size])
  pos=(pos+size+3)&~3
 return result
def entry(name,body,ino,mode):
 n=name.encode()+b'\0';f=[ino,mode,0,0,1,0,len(body),0,0,0,0,len(n),0];b=b'070701'+b''.join(f'{v:08x}'.encode()for v in f)+n;b+=b'\0'*((-len(b))%4);b+=body;b+=b'\0'*((-len(b))%4);return b
base=a.base_initrd.read_bytes();old=files(gzip.decompress(base));assert name not in old
# Validate the real fixture's command topology on the host too, before use.
fixture=old['fixture-pressure2/inputs.bin'][1];header=struct.unpack_from('<8I',fixture);commands=fixture[-header[4]:]
entries=[];cursor=0
while cursor<len(commands):
 kind,size,vdm,cdm=struct.unpack_from('<4H',commands,cursor);entries.append((cursor,kind,size,vdm,cdm));cursor+=8+size
assert cursor==len(commands)and entries[0][1:3]==(1,64)
assert sum(e[1]==0 for e in entries)==1 and next(e for e in entries if e[1]==0)[4]==1
assert entries[1][1]==3,'fixture includes metadata between compute and render'

image=base+gzip.compress(entry(name,elf,0x7a0000,stat.S_IFREG|0o755)+entry('TRAILER!!!',b'',0,0),mtime=0);(out/'initramfs.cpio.gz').write_bytes(image);new=files(gzip.decompress(image));assert all(new[n]==v for n,v in old.items())
expected=json.loads(a.base_expected.read_text())['guest_expected_sha256'];expected['/'+name]=hashlib.sha256(elf).hexdigest()
for n,h in expected.items():assert hashlib.sha256(new[n.lstrip('/')][1]).hexdigest()==h,n
(out/'preparation.json').write_text(json.dumps(dict(guest_expected_sha256=expected,initrd_sha256=hashlib.sha256(image).hexdigest(),all_base_entries_unchanged=len(old),original_growth_oracles_exact=True,original_source_sha256=hashlib.sha256(original.encode()).hexdigest(),generated_source_sha256=hashlib.sha256(source.encode()).hexdigest(),only_command_change='render cdm_barrier1 within same ioctl ->barrier0 accepted queue history in separate ioctl',requires_boot_cpu_cache_pause_grid=32,source_fixture_command_topology=entries,GPU_execution=False),indent=2)+'\n')
print('PASS strict ARM64 dependent-ioctl probe; original growth oracle exact;',len(expected),'guest inputs')
