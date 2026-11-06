// SPDX-License-Identifier: MIT
// Queue eight changing PBO uploads/render/readbacks across framebuffer redefinitions.
// Explicit software mode provides a GL API/oracle control; the default requires Apple/Zink.
#define GL_GLEXT_PROTOTYPES 1
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl3.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

static void fail(const char *where)
{
    fprintf(stderr, "FAIL %s egl=%#x gl=%#x\n", where, eglGetError(), glGetError());
    exit(1);
}
static GLuint shader(GLenum type, const char *source)
{
    GLuint s = glCreateShader(type);
    glShaderSource(s, 1, &source, NULL);
    glCompileShader(s);
    GLint ok = 0;
    glGetShaderiv(s, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[2048];
        glGetShaderInfoLog(s, sizeof(log), NULL, log);
        fprintf(stderr, "%s\n", log);
        fail("compile");
    }
    return s;
}
#ifndef RING
#define RING 8
#endif
#ifndef RESIZE
#define RESIZE 1
#endif
#define MAX_W 512
#define MAX_H 288
#define GUARD 64
#define CAPACITY (MAX_W*MAX_H*4 + 2*GUARD)
struct pending { GLuint buffer; GLsync fence; int frame, width, height; };
static void check_pending(struct pending *p, int tag)
{
    if (!p->fence) return;
    GLenum r=glClientWaitSync(p->fence, GL_SYNC_FLUSH_COMMANDS_BIT, 5000000000ull);
    if(r!=GL_ALREADY_SIGNALED && r!=GL_CONDITION_SATISFIED) fail("async fence wait");
    glDeleteSync(p->fence);p->fence=NULL;
    glBindBuffer(GL_PIXEL_PACK_BUFFER,p->buffer);
    unsigned char *mapped=glMapBufferRange(GL_PIXEL_PACK_BUFFER,0,CAPACITY,GL_MAP_READ_BIT);
    if(!mapped) fail("async pack mapping");
    for(int g=0;g<GUARD;g++) if(mapped[g]!=0xcd || mapped[GUARD+p->width*p->height*4+g]!=0xcd) fail("pack guard");
    for(int y=0;y<p->height;y++) for(int x=0;x<p->width;x++) {
        int t=(y&1)*2+((x&1)^(p->frame&1));
        unsigned char expected[4]={16,32,48,255};
        if(t==0 || t==3) {
            expected[0]=(unsigned char)(p->frame*13+tag*37+t*31);
            expected[1]=(unsigned char)(p->frame*7+tag*53+t*17);expected[2]=0;
        }
        unsigned char *actual=mapped+GUARD+4*(y*p->width+x);
        if(memcmp(actual,expected,4)) {
            fprintf(stderr,"tag=%d frame=%d size=%dx%d pixel=%d,%d actual=%u,%u,%u,%u expected=%u,%u,%u,%u\n",tag,p->frame,p->width,p->height,x,y,actual[0],actual[1],actual[2],actual[3],expected[0],expected[1],expected[2],expected[3]);
            // Diagnose whether a successful per-frame fence preceded the
            // write, or CPU mapping and GPU reads disagree about the bytes.
            glUnmapBuffer(GL_PIXEL_PACK_BUFFER);glBindBuffer(GL_PIXEL_PACK_BUFFER,0);
            glFinish();glBindBuffer(GL_PIXEL_PACK_BUFFER,p->buffer);
            unsigned char *after=glMapBufferRange(GL_PIXEL_PACK_BUFFER,0,CAPACITY,GL_MAP_READ_BIT);
            if(!after)fail("diagnostic remap");
            unsigned at=GUARD+4*(y*p->width+x);
            fprintf(stderr,"after_glFinish_cpu=%u,%u,%u,%u\n",after[at],after[at+1],after[at+2],after[at+3]);
            glUnmapBuffer(GL_PIXEL_PACK_BUFFER);glBindBuffer(GL_PIXEL_PACK_BUFFER,0);
            GLuint copy;glGenBuffers(1,&copy);
            glBindBuffer(GL_COPY_READ_BUFFER,p->buffer);glBindBuffer(GL_COPY_WRITE_BUFFER,copy);
            glBufferData(GL_COPY_WRITE_BUFFER,4,NULL,GL_STREAM_READ);
            glCopyBufferSubData(GL_COPY_READ_BUFFER,GL_COPY_WRITE_BUFFER,at,0,4);glFinish();
            after=glMapBufferRange(GL_COPY_WRITE_BUFFER,0,4,GL_MAP_READ_BIT);
            if(!after)fail("diagnostic GPU copy map");
            fprintf(stderr,"after_GPU_copy_cpu=%u,%u,%u,%u\n",after[0],after[1],after[2],after[3]);
            glUnmapBuffer(GL_COPY_WRITE_BUFFER);glDeleteBuffers(1,&copy);
            fail("queued resize/upload pixel oracle");
        }
    }
    if(!glUnmapBuffer(GL_PIXEL_PACK_BUFFER)) fail("unmap");
    glBindBuffer(GL_PIXEL_PACK_BUFFER,0);
}
int main(int argc, char **argv)
{
    int iterations = argc > 1 ? atoi(argv[1]) : 128;
    int tag = argc > 2 ? atoi(argv[2]) : 0;
    if (iterations < 1 || iterations > 4096 || tag < 0 || tag > 255) return 2;
    PFNEGLGETPLATFORMDISPLAYEXTPROC platform = (void *)eglGetProcAddress("eglGetPlatformDisplayEXT");
    if (!platform) fail("platform function");
    EGLDisplay d = platform(EGL_PLATFORM_SURFACELESS_MESA, EGL_DEFAULT_DISPLAY, NULL);
    if (!eglInitialize(d, NULL, NULL) || !eglBindAPI(EGL_OPENGL_ES_API)) fail("initialize");
    const EGLint attributes[] = {EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT, EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_NONE};
    EGLConfig config;
    EGLint count = 0;
    if (!eglChooseConfig(d, attributes, &config, 1, &count) || count != 1) fail("config");
    const EGLint context_attributes[] = {EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE};
    EGLContext context = eglCreateContext(d, config, EGL_NO_CONTEXT, context_attributes);
    if (context == EGL_NO_CONTEXT || !eglMakeCurrent(d, EGL_NO_SURFACE, EGL_NO_SURFACE, context)) fail("context");
    int software = argc > 3 && !strcmp(argv[3], "software");
    const char *renderer = (void *)glGetString(GL_RENDERER);
    if (!renderer || (software ? !strstr(renderer,"llvmpipe") : (!strstr(renderer,"zink") || !strstr(renderer,"Apple M4 Max")))) fail("requested renderer");
    printf("renderer=%s\n", renderer);
    const char *vs = "#version 300 es\nvoid main(){vec2 p=vec2(float((gl_VertexID<<1)&2),float(gl_VertexID&2));gl_Position=vec4(p*2.0-1.0,0.0,1.0);}";
    const char *fs = "#version 300 es\nprecision highp float;precision highp int;uniform sampler2D input_texture;uniform int phase;out vec4 color;void main(){ivec2 p=ivec2(gl_FragCoord.xy)&1;p.x^=phase;vec4 tex=texelFetch(input_texture,p,0);if(tex.a<0.5)discard;color=vec4(tex.xy,0.0,1.0);}";
    GLuint v = shader(GL_VERTEX_SHADER, vs), f = shader(GL_FRAGMENT_SHADER, fs);
    GLuint program = glCreateProgram();
    glAttachShader(program, v); glAttachShader(program, f); glLinkProgram(program);
    GLint linked = 0;
    glGetProgramiv(program, GL_LINK_STATUS, &linked);
    if (!linked) fail("link");
    glUseProgram(program);
    glDeleteShader(v); glDeleteShader(f);
    unsigned char texels[16] = {64,128,0,255, 192,32,0,0, 32,192,0,0, 128,64,0,255};
    GLuint textures[2], framebuffer;
    glGenTextures(2, textures);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, textures[0]);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, 2, 2, 0, GL_RGBA, GL_UNSIGNED_BYTE, texels);
    glBindTexture(GL_TEXTURE_2D, textures[1]);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, 64, 64, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);
    glGenFramebuffers(1, &framebuffer);
    glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, textures[1], 0);
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) fail("framebuffer");
    glBindTexture(GL_TEXTURE_2D, textures[0]);
    glUniform1i(glGetUniformLocation(program, "input_texture"), 0);
    GLint phase = glGetUniformLocation(program, "phase");
    glViewport(0, 0, 64, 64);
    glClearColor(16.0f/255, 32.0f/255, 48.0f/255, 1);
    struct pending pending[RING]={0};
    GLuint pack[RING];glGenBuffers(RING,pack);
    unsigned char *initial=malloc(CAPACITY);if(!initial)fail("allocation");memset(initial,0xcd,CAPACITY);
    for(unsigned j=0;j<RING;j++)pending[j].buffer=pack[j];
    GLuint upload;glGenBuffers(1,&upload);uint64_t pixels_checked=0;int max_pending=0;
    for(int i=0;i<iterations;i++) {
        struct pending *p=&pending[i%RING];check_pending(p,tag);
        int w=(RESIZE && (i/16)%2)?320:MAX_W,h=(RESIZE && (i/16)%2)?180:MAX_H;
        if(i==0 || (RESIZE && i%16==0)) {
            glBindBuffer(GL_PIXEL_UNPACK_BUFFER,0);
            glBindTexture(GL_TEXTURE_2D,textures[1]);
            glTexImage2D(GL_TEXTURE_2D,0,GL_RGBA8,w,h,0,GL_RGBA,GL_UNSIGNED_BYTE,NULL);
            glBindTexture(GL_TEXTURE_2D,textures[0]);glViewport(0,0,w,h);
            if(glCheckFramebufferStatus(GL_FRAMEBUFFER)!=GL_FRAMEBUFFER_COMPLETE)fail("resized framebuffer");
        }
        for(int t=0;t<4;t++) {
            texels[t*4]=(unsigned char)(i*13+tag*37+t*31);
            texels[t*4+1]=(unsigned char)(i*7+tag*53+t*17);
        }
        glBindBuffer(GL_PIXEL_UNPACK_BUFFER,upload);
        glBufferData(GL_PIXEL_UNPACK_BUFFER,sizeof(texels),texels,GL_STREAM_DRAW);
        glTexSubImage2D(GL_TEXTURE_2D,0,0,0,2,2,GL_RGBA,GL_UNSIGNED_BYTE,0);
        glBindBuffer(GL_PIXEL_UNPACK_BUFFER,0);
        glUniform1i(phase,i&1);glClear(GL_COLOR_BUFFER_BIT);glDrawArrays(GL_TRIANGLES,0,3);
        glBindBuffer(GL_PIXEL_PACK_BUFFER,p->buffer);
        glBufferData(GL_PIXEL_PACK_BUFFER,CAPACITY,initial,GL_STREAM_READ);
        glReadPixels(0,0,w,h,GL_RGBA,GL_UNSIGNED_BYTE,(void *)(uintptr_t)GUARD);
        glBindBuffer(GL_PIXEL_PACK_BUFFER,0);
        p->frame=i;p->width=w;p->height=h;p->fence=glFenceSync(GL_SYNC_GPU_COMMANDS_COMPLETE,0);
        glFlush();if(glGetError() || !p->fence)fail("queued upload/render/readback");
        int live=0;
        for(unsigned j=0;j<RING;j++)if(pending[j].fence){
            GLint state;glGetSynciv(pending[j].fence,GL_SYNC_STATUS,1,NULL,&state);
            live+=state==GL_UNSIGNALED;
        }
        if(live>max_pending)max_pending=live;
        pixels_checked+=(uint64_t)w*h;
    }
    for(unsigned j=0;j<RING;j++)check_pending(&pending[j],tag);
    glDeleteBuffers(RING,pack);free(initial);
    glDeleteBuffers(1, &upload);
    glDeleteFramebuffers(1, &framebuffer); glDeleteTextures(2, textures); glDeleteProgram(program);
    eglMakeCurrent(d, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroyContext(d, context); eglTerminate(d);
    printf("PASS queued uploads, %u in-flight frames and framebuffer resizing; %d frames, %llu exact pixels, max pending fences %d\n", RING, iterations, (unsigned long long)pixels_checked,max_pending);
    return 0;
}
