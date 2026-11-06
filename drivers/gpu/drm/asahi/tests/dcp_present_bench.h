/* SPDX-License-Identifier: MIT */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
#include <drm_fourcc.h>
static unsigned perf_width, perf_height, perf_frames;
static int client_socket;
static uint64_t now_ns(void) { struct timespec t; if(clock_gettime(CLOCK_MONOTONIC,&t)) abort(); return (uint64_t)t.tv_sec*1000000000+t.tv_nsec; }
static void exact_read(int fd,void *p,size_t n) { while(n) { ssize_t r=read(fd,p,n);if(r<0&&errno==EINTR)continue;if(r<=0){perror("read");exit(1);}p=(char*)p+r;n-=r;} }
static void exact_write(int fd,const void *p,size_t n) { while(n) { ssize_t r=write(fd,p,n);if(r<0&&errno==EINTR)continue;if(r<=0){perror("write");exit(1);}p=(const char*)p+r;n-=r;} }
static void send_dma_fd(int socket,int fd) {
 char b=0,control[CMSG_SPACE(sizeof(int))]={0};struct iovec iov={&b,1};
 struct msghdr msg={.msg_iov=&iov,.msg_iovlen=1,.msg_control=control,.msg_controllen=sizeof(control)};
 struct cmsghdr *c=CMSG_FIRSTHDR(&msg);c->cmsg_level=SOL_SOCKET;c->cmsg_type=SCM_RIGHTS;c->cmsg_len=CMSG_LEN(sizeof(int));memcpy(CMSG_DATA(c),&fd,sizeof(fd));
 if(sendmsg(socket,&msg,0)!=1){perror("sendmsg");exit(1);}
}
static int recv_dma_fd(int socket) {
 char b,control[CMSG_SPACE(sizeof(int))]={0};struct iovec iov={&b,1};
 struct msghdr msg={.msg_iov=&iov,.msg_iovlen=1,.msg_control=control,.msg_controllen=sizeof(control)};
 if(recvmsg(socket,&msg,0)!=1 || msg.msg_flags&MSG_CTRUNC){perror("recvmsg");exit(1);}
 struct cmsghdr *c=CMSG_FIRSTHDR(&msg);if(!c||c->cmsg_level!=SOL_SOCKET||c->cmsg_type!=SCM_RIGHTS||c->cmsg_len!=CMSG_LEN(sizeof(int)))abort();
 int fd;memcpy(&fd,CMSG_DATA(c),sizeof(fd));return fd;
}
