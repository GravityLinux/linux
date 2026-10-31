// SPDX-License-Identifier: MIT
// Retained BO lifetime regression: allocate, GPU-copy, verify, delete and exit.
#define GL_GLEXT_PROTOTYPES 1
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl3.h>
#include <gbm.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define REQUIRE(x) do { if (!(x)) { fprintf(stderr, "FAIL %s line %d EGL=%#x GL=%#x\n", #x, __LINE__, eglGetError(), glGetError()); exit(1); } } while (0)
int main(int argc, char **argv)
{
   unsigned mb = argc > 1 ? strtoul(argv[1], NULL, 0) : 16;
   unsigned iters = argc > 2 ? strtoul(argv[2], NULL, 0) : 1;
   int submit = argc > 3 ? atoi(argv[3]) : 1;
   REQUIRE(mb && mb <= 256 && iters && iters <= 100);
   int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
   REQUIRE(fd >= 0);
   struct gbm_device *gbm = gbm_create_device(fd);
   REQUIRE(gbm);
   PFNEGLGETPLATFORMDISPLAYEXTPROC gpd = (void *)eglGetProcAddress("eglGetPlatformDisplayEXT");
   REQUIRE(gpd);
   EGLDisplay d = gpd(EGL_PLATFORM_GBM_KHR, gbm, NULL);
   REQUIRE(eglInitialize(d, NULL, NULL));
   REQUIRE(eglBindAPI(EGL_OPENGL_ES_API));
   EGLConfig cfg;
   EGLint nc = 0, at[] = { EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT, EGL_SURFACE_TYPE, 0, EGL_NONE };
   REQUIRE(eglChooseConfig(d, at, &cfg, 1, &nc) && nc);
   EGLint ca[] = { EGL_CONTEXT_MAJOR_VERSION, 3, EGL_NONE };
   EGLContext ctx = eglCreateContext(d, cfg, EGL_NO_CONTEXT, ca);
   REQUIRE(ctx != EGL_NO_CONTEXT && eglMakeCurrent(d, EGL_NO_SURFACE, EGL_NO_SURFACE, ctx));
   const char *renderer = (const char *)glGetString(GL_RENDERER);
   REQUIRE(renderer && strstr(renderer, "zink") && !strstr(renderer, "llvmpipe"));
   printf("RENDERER %s VERSION %s\n", renderer, glGetString(GL_VERSION));
   size_t bytes = (size_t)mb << 20;
   uint32_t *data = malloc(bytes);
   REQUIRE(data);
   for (unsigned i = 0; i < iters; ++i) {
      uint32_t token = 0xc0ff0000u ^ (uint32_t)getpid() ^ i;
      for (size_t j = 0; j < bytes / 4; ++j) data[j] = token ^ (uint32_t)j;
      GLuint b[2];
      glGenBuffers(2, b);
      glBindBuffer(GL_COPY_READ_BUFFER, b[0]);
      glBufferData(GL_COPY_READ_BUFFER, bytes, data, GL_STATIC_DRAW);
      glBindBuffer(GL_COPY_WRITE_BUFFER, b[1]);
      glBufferData(GL_COPY_WRITE_BUFFER, bytes, NULL, GL_DYNAMIC_READ);
      if (submit) glCopyBufferSubData(GL_COPY_READ_BUFFER, GL_COPY_WRITE_BUFFER, 0, 0, bytes);
      glFinish();
      REQUIRE(glGetError() == GL_NO_ERROR);
      if (submit) {
         uint32_t *result = glMapBufferRange(GL_COPY_WRITE_BUFFER, 0, bytes, GL_MAP_READ_BIT);
         REQUIRE(result);
         for (size_t j = 0; j < bytes / 4; ++j) REQUIRE(result[j] == data[j]);
         REQUIRE(glUnmapBuffer(GL_COPY_WRITE_BUFFER));
      }
      glDeleteBuffers(2, b);
      glFinish();
      REQUIRE(glGetError() == GL_NO_ERROR);
   }
   free(data);
   REQUIRE(eglMakeCurrent(d, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT));
   REQUIRE(eglDestroyContext(d, ctx));
   REQUIRE(eglTerminate(d));
   gbm_device_destroy(gbm);
   close(fd);
   printf("PASS buffers=2 bytes_each=%zu iters=%u gpu_copy=%d full_copy_oracle=%d\n", bytes, iters, submit, submit);
   return 0;
}
