// SPDX-License-Identifier: MIT
// GPU oracle for a texture value live after discard. Run multiple independent
// processes with production Zink; this is a correctness probe, not an FPS test.
#define GL_GLEXT_PROTOTYPES 1
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl3.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

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
int main(int argc, char **argv)
{
    int iterations = argc > 1 ? atoi(argv[1]) : 128;
    if (iterations < 1 || iterations > 4096) return 2;
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
    const char *renderer = (void *)glGetString(GL_RENDERER);
    if (!renderer || !strstr(renderer, "zink") || !strstr(renderer, "Apple M4 Max")) fail("hardware renderer");
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
    const unsigned char texels[16] = {64,128,0,255, 192,32,0,0, 32,192,0,0, 128,64,0,255};
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
    unsigned char pixels[64*64*4];
    for (int i = 0; i < iterations; i++) {
        glUniform1i(phase, i & 1);
        glClear(GL_COLOR_BUFFER_BIT);
        glDrawArrays(GL_TRIANGLES, 0, 3);
        glReadPixels(0, 0, 64, 64, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
        if (glGetError()) fail("render/readback");
        for (int y = 0; y < 64; y++) for (int x = 0; x < 64; x++) {
            const unsigned char *t = texels + 4*((y&1)*2+((x&1)^(i&1)));
            unsigned char expected[4] = {16,32,48,255};
            if (t[3]) { expected[0] = t[0]; expected[1] = t[1]; expected[2] = 0; }
            if (memcmp(pixels+4*(y*64+x), expected, 4)) fail("pixel oracle");
        }
    }
    glDeleteFramebuffers(1, &framebuffer); glDeleteTextures(2, textures); glDeleteProgram(program);
    eglMakeCurrent(d, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroyContext(d, context); eglTerminate(d);
    printf("PASS texture result after discard; %d frames, %d exact pixels\n", iterations, iterations*64*64);
    return 0;
}
