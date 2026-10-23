/* SPDX-License-Identifier: MIT */
/* Hold live compute VMs while a dormant render PB moves to the next context. */
#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
struct holder { pid_t pid; FILE *input,*output; int render; };
static void join(pid_t pid){int status;assert(waitpid(pid,&status,0)==pid);assert(WIFEXITED(status)&&WEXITSTATUS(status)==0);}
static void until(FILE *f,const char *marker){char line[1024];while(fgets(line,sizeof(line),f)){fputs(line,stdout);if(strstr(line,marker))return;}assert(!"compute holder exited before marker");}
static struct holder start(const char *fixture,int render){
 int in[2],out[2];assert(!pipe(in)&&!pipe(out));pid_t pid=fork();assert(pid>=0);
 if(!pid){assert(dup2(in[0],STDIN_FILENO)>=0&&dup2(out[1],STDOUT_FILENO)>=0);close(in[0]);close(in[1]);close(out[0]);close(out[1]);if(render)execl("/g16c_drm_render","g16c_drm_render",fixture,"--hold",(char*)NULL);else execl("/g16c_drm_compute","g16c_drm_compute",fixture,"--hold",(char*)NULL);_exit(127);}
 close(in[0]);close(out[1]);struct holder h={.pid=pid,.input=fdopen(in[1],"w"),.output=fdopen(out[0],"r"),.render=render};assert(h.input&&h.output);until(h.output,render?"G16C_RENDER_HOLD_READY":"G16C_COMPUTE_HOLD_READY");return h;
}
static void check(struct holder *h){assert(fputs("R\n",h->input)>=0&&!fflush(h->input));until(h->output,h->render?"G16C_RENDER_HOLD_OUTPUT_PASS":"G16C_COMPUTE_HOLD_OUTPUT_PASS");}
static void finish(struct holder *h){assert(fputs("Q\n",h->input)>=0&&!fclose(h->input));char line[1024];while(fgets(line,sizeof(line),h->output))fputs(line,stdout);assert(!fclose(h->output));join(h->pid);}
static void draw(const char *fixture){pid_t pid=fork();assert(pid>=0);if(!pid){execl("/g16c_drm_render","g16c_drm_render",fixture,"--kernel-base=0x7100004000",(char*)NULL);_exit(127);}join(pid);}
int main(int argc,char **argv){
 setbuf(stdout,NULL);assert(argc==3||argc==4);unsigned count=2;
 if(argc==4&&!strcmp(argv[3],"--active-render")){
  puts("G16C_ACTIVE_RENDER_PB_BEGIN");struct holder h=start(argv[2],1);draw(argv[2]);check(&h);finish(&h);
  puts("G16C_ACTIVE_RENDER_PB_PASS held_render_output_timestamps_survived_peer=1");return 0;
 }
 if(argc==4){char *end;assert(!strncmp(argv[3],"--holders=",10));unsigned long n=strtoul(argv[3]+10,&end,0);assert(!*end&&n>0&&n<=20);count=n;}
 struct holder h[20];printf("G16C_PB_SHARING_BEGIN holders=%u\n",count);draw(argv[2]);
 for(unsigned i=0;i<count;i++){
  h[i]=start(argv[1],0);draw(argv[2]);
  for(unsigned j=0;j<=i;j++)check(&h[j]);
  printf("G16C_PB_SHARING_STAGE_PASS live_compute=%u renderer_survivors=%u\n",i+1,i+1);
 }
 for(unsigned i=count;i>0;i--){finish(&h[i-1]);for(unsigned j=0;j+1<i;j++)check(&h[j]);printf("G16C_PB_SHARING_CLOSE_PASS remaining=%u\n",i-1);}
 printf("G16C_PB_SHARING_PASS holders=%u all_live_outputs_and_close_survivors=1\n",count);return 0;
}
