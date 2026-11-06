/* SPDX-License-Identifier: MIT
 * Native Vulkan scissored triangle and guarded GPU readback test.
 */
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <vulkan/vulkan.h>
#include "apple9-test-device.h"
#include "dcp_present_bench.h"
#define CHECK(x)                                                               \
   do {                                                                        \
      VkResult r = (x);                                                        \
      if (r != VK_SUCCESS) {                                                   \
         fprintf(stderr, "%s: %d (line %d)\n", #x, r, __LINE__);               \
         exit(1);                                                              \
      }                                                                        \
   } while (0)
int
client_main(int argc, char **argv)
{
   if ((argc != 3 && argc != 4) || (argc == 4 && strcmp(argv[3], "bounds") &&
                                    strcmp(argv[3], "bounds-scalar") &&
                                    strcmp(argv[3], "bounds-wrap") &&
                                    strcmp(argv[3], "sample-mask"))) {
      fprintf(stderr,
              "Usage: %s vertex.spv fragment.spv [bounds|bounds-scalar|bounds-wrap|sample-mask]\n",
              argv[0]);
      return 1;
   }
   bool sample_mask = argc == 4 && !strcmp(argv[3], "sample-mask");
   bool bounds = argc == 4 && !sample_mask;
   bool scalar_vertex = bounds && !strcmp(argv[3], "bounds-scalar");
   bool far_vertex = bounds && !strcmp(argv[3], "bounds-wrap");
   setbuf(stdout, NULL);
   VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
                            .pApplicationName = "Apple9 offscreen draw",
                            .apiVersion = VK_API_VERSION_1_1};
   VkInstanceCreateInfo ici = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
                               .pApplicationInfo = &app};
   VkInstance instance;
   CHECK(vkCreateInstance(&ici, NULL, &instance));
   uint32_t n = 0;
   CHECK(vkEnumeratePhysicalDevices(instance, &n, NULL));
   if (!n) {
      fprintf(stderr, "No Vulkan device\n");
      return 1;
   }
   VkPhysicalDevice *devices = calloc(n, sizeof(*devices));
   CHECK(vkEnumeratePhysicalDevices(instance, &n, devices));
   VkPhysicalDevice phy = devices[0];
   free(devices);
   VkPhysicalDeviceProperties prop;
   vkGetPhysicalDeviceProperties(phy, &prop);
   printf("device: %s vendor=%04x device=%04x API=%u.%u\n", prop.deviceName,
          prop.vendorID, prop.deviceID, VK_VERSION_MAJOR(prop.apiVersion),
          VK_VERSION_MINOR(prop.apiVersion));
   if (!apple9_test_native_device(&prop)) {
      fprintf(stderr, "Expected native G16G/G16C/G17P renderer\n");
      return 1;
   }
   uint32_t nq = 0;
   vkGetPhysicalDeviceQueueFamilyProperties(phy, &nq, NULL);
   VkQueueFamilyProperties *qp = calloc(nq, sizeof(*qp));
   vkGetPhysicalDeviceQueueFamilyProperties(phy, &nq, qp);
   uint32_t qi;
   for (qi = 0; qi < nq; qi++)
      if (qp[qi].queueFlags & VK_QUEUE_GRAPHICS_BIT)
         break;
   free(qp);
   if (qi == nq)
      return 1;
   float priority = 1;
   VkDeviceQueueCreateInfo qci = {
      .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
      .queueFamilyIndex = qi,
      .queueCount = 1,
      .pQueuePriorities = &priority};
   VkPhysicalDeviceFeatures features = {.robustBufferAccess = bounds};
   const char *extensions[] = {VK_KHR_EXTERNAL_MEMORY_FD_EXTENSION_NAME,
                              VK_EXT_EXTERNAL_MEMORY_DMA_BUF_EXTENSION_NAME};
   VkDeviceCreateInfo dci = {.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
                             .queueCreateInfoCount = 1,
                             .pQueueCreateInfos = &qci,
                             .pEnabledFeatures = &features,
                             .enabledExtensionCount = 2,
                             .ppEnabledExtensionNames = extensions};
   VkDevice dev;
   CHECK(vkCreateDevice(phy, &dci, NULL, &dev));
   VkQueue queue;
   vkGetDeviceQueue(dev, qi, 0, &queue);
   const unsigned W = perf_width, H = perf_height;
   const unsigned PIXELS = W * H, BYTES = PIXELS * 4, GUARD = 64;
   VkImageCreateInfo ii = {.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
                           .imageType = VK_IMAGE_TYPE_2D,
                           .format = VK_FORMAT_R8G8B8A8_UNORM,
                           .extent = {W, H, 1},
                           .mipLevels = 1,
                           .arrayLayers = 1,
                           .samples = VK_SAMPLE_COUNT_1_BIT,
                           .tiling = VK_IMAGE_TILING_OPTIMAL,
                           .usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT |
                                    VK_IMAGE_USAGE_TRANSFER_SRC_BIT};
   VkImage image;
   CHECK(vkCreateImage(dev, &ii, NULL, &image));
   VkExternalMemoryBufferCreateInfo external = {
      .sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_BUFFER_CREATE_INFO,
      .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT};
   VkBufferCreateInfo bi = {.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
                            .pNext = &external,
                            .size = BYTES + GUARD,
                            .usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT};
   VkBuffer buffer;
   CHECK(vkCreateBuffer(dev, &bi, NULL, &buffer));
   VkPhysicalDeviceMemoryProperties mp;
   vkGetPhysicalDeviceMemoryProperties(phy, &mp);
   VkDeviceMemory memory[2];
   for (unsigned i = 0; i < 2; ++i) {
      VkMemoryRequirements mr;
      if (i)
         vkGetBufferMemoryRequirements(dev, buffer, &mr);
      else
         vkGetImageMemoryRequirements(dev, image, &mr);
      unsigned t;
      for (t = 0; t < mp.memoryTypeCount; ++t)
         if ((mr.memoryTypeBits & (1u << t)) &&
             (!i || (mp.memoryTypes[t].propertyFlags &
                     (VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT |
                      VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)) ==
                       (VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT |
                        VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)))
            break;
      if (t == mp.memoryTypeCount)
         return 1;
      VkExportMemoryAllocateInfo export = {
         .sType = VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO,
         .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT};
      VkMemoryDedicatedAllocateInfo dedicated = {
         .sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
         .pNext = &export, .buffer = buffer};
      VkMemoryAllocateInfo ai = {
         .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
         .pNext = i ? &dedicated : NULL,
         .allocationSize = mr.size,
         .memoryTypeIndex = t};
      CHECK(vkAllocateMemory(dev, &ai, NULL, &memory[i]));
   }
   CHECK(vkBindImageMemory(dev, image, memory[0], 0));
   CHECK(vkBindBufferMemory(dev, buffer, memory[1], 0));
   uint8_t *pixels;
   CHECK(vkMapMemory(dev, memory[1], 0, VK_WHOLE_SIZE, 0, (void **)&pixels));
   VkBuffer vertex_buffer = VK_NULL_HANDLE;
   VkDeviceMemory vertex_memory = VK_NULL_HANDLE;
   if (bounds) {
      VkBufferCreateInfo vb = {.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
                               .size = 4,
                               .usage = VK_BUFFER_USAGE_VERTEX_BUFFER_BIT};
      CHECK(vkCreateBuffer(dev, &vb, NULL, &vertex_buffer));
      VkMemoryRequirements req;
      vkGetBufferMemoryRequirements(dev, vertex_buffer, &req);
      unsigned t;
      for (t = 0; t < mp.memoryTypeCount; ++t)
         if ((req.memoryTypeBits & (1u << t)) &&
             (mp.memoryTypes[t].propertyFlags &
              (VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT |
               VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)) ==
                (VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT |
                 VK_MEMORY_PROPERTY_HOST_COHERENT_BIT))
            break;
      if (t == mp.memoryTypeCount)
         return 1;
      VkMemoryAllocateInfo ai = {
         .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
         .allocationSize = req.size,
         .memoryTypeIndex = t};
      CHECK(vkAllocateMemory(dev, &ai, NULL, &vertex_memory));
      CHECK(vkBindBufferMemory(dev, vertex_buffer, vertex_memory, 0));
      float *mapped;
      CHECK(vkMapMemory(dev, vertex_memory, 0, VK_WHOLE_SIZE, 0,
                        (void **)&mapped));
      /* Only red belongs to the VkBuffer. A scalar attribute is in bounds
       * and must retain it. The vec4 attribute is out of bounds as a whole:
       * core robustBufferAccess permits returning zero for all components.
       * G16 checks whole attributes, so the padding must never leak. */
      mapped[0] = 1.0f;
      mapped[1] = mapped[2] = mapped[3] = 0.75f;
      vkUnmapMemory(dev, vertex_memory);
   }
   VkImageViewCreateInfo vi = {
      .sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
      .image = image,
      .viewType = VK_IMAGE_VIEW_TYPE_2D,
      .format = ii.format,
      .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
   VkImageView view;
   CHECK(vkCreateImageView(dev, &vi, NULL, &view));
   VkAttachmentDescription ad = {
      .format = ii.format,
      .samples = ii.samples,
      .loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR,
      .storeOp = VK_ATTACHMENT_STORE_OP_STORE,
      .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
      .finalLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL};
   VkAttachmentReference ar = {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
   VkSubpassDescription sub = {
      .pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS,
      .colorAttachmentCount = 1,
      .pColorAttachments = &ar};
   VkRenderPassCreateInfo ri = {
      .sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO,
      .attachmentCount = 1,
      .pAttachments = &ad,
      .subpassCount = 1,
      .pSubpasses = &sub};
   VkRenderPass render;
   CHECK(vkCreateRenderPass(dev, &ri, NULL, &render));

   VkShaderModule modules[2];
   for (unsigned i = 0; i < 2; ++i) {
      FILE *file = fopen(argv[i + 1], "rb");
      if (!file)
         return 1;
      fseek(file, 0, SEEK_END);
      size_t size = ftell(file);
      rewind(file);
      uint32_t *code = malloc(size);
      if (fread(code, 1, size, file) != size)
         return 1;
      fclose(file);
      VkShaderModuleCreateInfo mi = {
         .sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
         .codeSize = size,
         .pCode = code};
      CHECK(vkCreateShaderModule(dev, &mi, NULL, &modules[i]));
      free(code);
   }
   VkPipelineLayoutCreateInfo li = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
   VkPipelineLayout layout;
   CHECK(vkCreatePipelineLayout(dev, &li, NULL, &layout));
   VkPipelineShaderStageCreateInfo stages[2] = {
      {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
       .stage = VK_SHADER_STAGE_FRAGMENT_BIT,
       .module = modules[1],
       .pName = "main"},
      {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
       .stage = VK_SHADER_STAGE_VERTEX_BIT,
       .module = modules[0],
       .pName = "main"}};
   VkVertexInputBindingDescription binding = {0,
                                              far_vertex ? 2048
                                              : bounds   ? 0
                                                         : 8,
                                              VK_VERTEX_INPUT_RATE_VERTEX};
   VkVertexInputAttributeDescription attr = {
      0, 0, scalar_vertex ? VK_FORMAT_R32_SFLOAT
            : bounds ? VK_FORMAT_R32G32B32A32_SFLOAT : VK_FORMAT_R32G32_SFLOAT,
      0};
   VkPipelineVertexInputStateCreateInfo vertex_input = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
      .vertexBindingDescriptionCount = bounds,
      .pVertexBindingDescriptions = &binding,
      .vertexAttributeDescriptionCount = bounds,
      .pVertexAttributeDescriptions = &attr};
   VkPipelineInputAssemblyStateCreateInfo ia = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
      .topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST};
   VkViewport viewport = {0, 0, W, H, 0, 1};
   VkRect2D scissor = {{3, 2}, {W - 7, H - 5}};
   VkPipelineViewportStateCreateInfo vp = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
      .viewportCount = 1,
      .pViewports = &viewport,
      .scissorCount = 1,
      .pScissors = &scissor};
   VkPipelineRasterizationStateCreateInfo rs = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
      .polygonMode = VK_POLYGON_MODE_FILL,
      .cullMode = VK_CULL_MODE_NONE,
      .frontFace = VK_FRONT_FACE_COUNTER_CLOCKWISE,
      .lineWidth = 1};
   VkPipelineMultisampleStateCreateInfo ms = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
      .rasterizationSamples = VK_SAMPLE_COUNT_1_BIT};
   VkPipelineColorBlendAttachmentState ca = {
      .blendEnable = VK_FALSE,
      .srcColorBlendFactor = VK_BLEND_FACTOR_SRC_ALPHA,
      .dstColorBlendFactor = VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA,
      .colorBlendOp = VK_BLEND_OP_ADD,
      .srcAlphaBlendFactor = VK_BLEND_FACTOR_ONE,
      .dstAlphaBlendFactor = VK_BLEND_FACTOR_ZERO,
      .alphaBlendOp = VK_BLEND_OP_ADD,
      .colorWriteMask = 15};
   VkPipelineColorBlendStateCreateInfo cb = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
      .attachmentCount = 1,
      .pAttachments = &ca};
   VkPipelineDepthStencilStateCreateInfo ds = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO,
      .depthTestEnable = VK_FALSE,
      .depthWriteEnable = VK_FALSE,
      .depthCompareOp = VK_COMPARE_OP_LESS,
   };
   VkGraphicsPipelineCreateInfo graphics = {
      .sType = VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,
      .stageCount = 2,
      .pStages = stages,
      .pVertexInputState = &vertex_input,
      .pInputAssemblyState = &ia,
      .pViewportState = &vp,
      .pRasterizationState = &rs,
      .pMultisampleState = &ms,
      .pColorBlendState = &cb,
      .pDepthStencilState = &ds,
      .layout = layout,
      .renderPass = render};
   VkPipeline pipeline;
   CHECK(vkCreateGraphicsPipelines(dev, VK_NULL_HANDLE, 1, &graphics, NULL,
                                   &pipeline));

   VkFramebufferCreateInfo fi = {
      .sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO,
      .renderPass = render,
      .attachmentCount = 1,
      .pAttachments = &view,
      .width = W,
      .height = H,
      .layers = 1};
   VkFramebuffer fb;
   CHECK(vkCreateFramebuffer(dev, &fi, NULL, &fb));
   VkCommandPoolCreateInfo pi = {
      .sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
      .flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
      .queueFamilyIndex = qi};
   VkCommandPool pool;
   CHECK(vkCreateCommandPool(dev, &pi, NULL, &pool));
   VkCommandBufferAllocateInfo cai = {
      .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
      .commandPool = pool,
      .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
      .commandBufferCount = 1};
   VkCommandBuffer cmd;
   CHECK(vkAllocateCommandBuffers(dev, &cai, &cmd));
   VkFenceCreateInfo fence_info = {.sType =
                                      VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
   VkFence fence;
   CHECK(vkCreateFence(dev, &fence_info, NULL, &fence));
   PFN_vkGetMemoryFdKHR getfd = (PFN_vkGetMemoryFdKHR)vkGetDeviceProcAddr(dev,"vkGetMemoryFdKHR");
   if (!getfd) abort();
   VkMemoryGetFdInfoKHR fd_info = {
      .sType = VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR,
      .memory = memory[1], .handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT};
   int dma_fd;
   CHECK(getfd(dev, &fd_info, &dma_fd));
   send_dma_fd(client_socket, dma_fd); close(dma_fd);
   memset(pixels, 0xa5, BYTES + GUARD);
   for (unsigned pass = 0; pass < perf_frames; ++pass) {
      unsigned token; exact_read(client_socket,&token,sizeof(token));
      const uint64_t started = now_ns();
      unsigned generation = pass % 3;
      const uint8_t expected[4] = {32 + generation * 31, 128 - generation * 17,
                                   223 - generation * 23, 255};
      CHECK(vkResetCommandBuffer(cmd, 0));
      CHECK(vkResetFences(dev, 1, &fence));
      VkCommandBufferBeginInfo begin = {
         .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
      CHECK(vkBeginCommandBuffer(cmd, &begin));
      VkClearValue clear;
      for (unsigned c = 0; c < 4; ++c)
         clear.color.float32[c] = expected[c] / 255.0f;
      VkRenderPassBeginInfo rb = {
         .sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO,
         .renderPass = render,
         .framebuffer = fb,
         .renderArea = {{0, 0}, {W, H}},
         .clearValueCount = 1,
         .pClearValues = &clear};
      vkCmdBeginRenderPass(cmd, &rb, VK_SUBPASS_CONTENTS_INLINE);
      vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline);
      if (bounds) {
         VkDeviceSize zero = 0;
         vkCmdBindVertexBuffers(cmd, 0, 1, &vertex_buffer, &zero);
      }
      vkCmdDraw(cmd, 3, 1, far_vertex ? (1u << 21) : 0, 0);
      vkCmdEndRenderPass(cmd);
      VkImageMemoryBarrier barrier = {
         .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
         .srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
         .dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
         .oldLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
         .newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
         .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
         .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
         .image = image,
         .subresourceRange = vi.subresourceRange};
      vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
                           VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, NULL, 0, NULL,
                           1, &barrier);
      VkBufferImageCopy copy = {
         .imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1},
         .imageExtent = {W, H, 1}};
      VkBufferMemoryBarrier acquire = {
         .sType = VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER,
         .dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
         .srcQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL, .dstQueueFamilyIndex = qi,
         .buffer = buffer, .size = VK_WHOLE_SIZE};
      if (pass) vkCmdPipelineBarrier(cmd,VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
         VK_PIPELINE_STAGE_TRANSFER_BIT,0,0,NULL,1,&acquire,0,NULL);
      vkCmdCopyImageToBuffer(cmd, image, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                             buffer, 1, &copy);
      VkMemoryBarrier host = {.sType = VK_STRUCTURE_TYPE_MEMORY_BARRIER,
                              .srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
                              .dstAccessMask = VK_ACCESS_HOST_READ_BIT};
      vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TRANSFER_BIT,
                           VK_PIPELINE_STAGE_HOST_BIT, 0, 1, &host, 0, NULL, 0,
                           NULL);
      VkBufferMemoryBarrier release = {
         .sType = VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER,
         .srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
         .srcQueueFamilyIndex = qi, .dstQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL,
         .buffer = buffer, .size = VK_WHOLE_SIZE};
      vkCmdPipelineBarrier(cmd,VK_PIPELINE_STAGE_TRANSFER_BIT,
         VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,0,0,NULL,1,&release,0,NULL);
      CHECK(vkEndCommandBuffer(cmd));
      VkSubmitInfo submit = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                             .commandBufferCount = 1,
                             .pCommandBuffers = &cmd};
      CHECK(vkQueueSubmit(queue, 1, &submit, fence));
      CHECK(vkWaitForFences(dev, 1, &fence, VK_TRUE, 10000000000ull));
      const uint64_t gpu_ns = now_ns() - started;
      for (unsigned i = 0; i < BYTES; ++i) {
         // Full oracle on first/last frames; sparse fresh-color checks otherwise.
         if (pass && pass + 1 < perf_frames) {
            const unsigned center = ((H/2)*W+W/2)*4;
            if (i == 64) i = center;
            if (i == center + 4) i = BYTES - 64;
         }
         unsigned x = (i / 4) % W, y = (i / 4) / W;
         bool inside = !sample_mask && x >= 3 && x < W - 4 && y >= 2 && y < H - 3;
         const uint8_t draw_color[4] = {bounds && !scalar_vertex ? 0 : 255, bounds ? 0 : 64,
                                        bounds ? 0 : 128, 255};
         uint8_t wanted = inside ? draw_color[i % 4] : expected[i % 4];
         if (abs((int)pixels[i] - wanted) > 1) {
            fprintf(stderr, "pixel %u channel %u: got %u expected %u\n", i / 4,
                    i % 4, pixels[i], wanted);
            return 1;
         }
      }
      for (unsigned i = BYTES; i < BYTES + GUARD; ++i)
         if (pixels[i] != 0xa5)
            return 1;
      exact_write(client_socket, &gpu_ns, sizeof(gpu_ns));
   }
   vkDestroyFence(dev, fence, NULL);
   vkDestroyCommandPool(dev, pool, NULL);
   vkDestroyFramebuffer(dev, fb, NULL);
   vkDestroyPipeline(dev, pipeline, NULL);
   vkDestroyPipelineLayout(dev, layout, NULL);
   for (unsigned i = 0; i < 2; ++i)
      vkDestroyShaderModule(dev, modules[i], NULL);
   vkDestroyRenderPass(dev, render, NULL);
   vkDestroyImageView(dev, view, NULL);
   vkUnmapMemory(dev, memory[1]);
   vkDestroyBuffer(dev, buffer, NULL);
   vkDestroyImage(dev, image, NULL);
   vkFreeMemory(dev, memory[1], NULL);
   vkFreeMemory(dev, memory[0], NULL);
   vkDestroyBuffer(dev, vertex_buffer, NULL);
   vkFreeMemory(dev, vertex_memory, NULL);
   vkDestroyDevice(dev, NULL);
   vkDestroyInstance(instance, NULL);
   return 0;
}

static void flip_done(int fd,unsigned seq,unsigned sec,unsigned usec,void *data) {
 (void)fd;(void)seq;(void)sec;(void)usec;*(bool*)data=true;
}
int main(int argc,char **argv) {
 setbuf(stdout,NULL);
 if(argc!=3){fprintf(stderr,"usage: %s vertex.spv fragment.spv\n",argv[0]);return 1;}
 perf_frames=getenv("PERF_FRAMES")?strtoul(getenv("PERF_FRAMES"),NULL,0):30;
 if(perf_frames<4||perf_frames>1000)abort();
 bool offscreen=getenv("PERF_OFFSCREEN")!=NULL;
 unsigned clients=getenv("PERF_CLIENTS")?strtoul(getenv("PERF_CLIENTS"),NULL,0):2;
 if(clients!=1&&clients!=2)abort();
 int dcp=-1;drmModeRes *res=NULL;
 for(int i=0;i<8;i++) {char path[64];snprintf(path,sizeof(path),"/dev/dri/card%d",i);int fd=open(path,O_RDWR|O_CLOEXEC);if(fd<0)continue;
  drmVersionPtr ver=drmGetVersion(fd);bool apple=ver&&ver->name&&!strcmp(ver->name,"apple");if(ver)drmFreeVersion(ver);
  if(apple){dcp=fd;res=drmModeGetResources(fd);if(res&&res->count_connectors)break;if(res)drmModeFreeResources(res);res=NULL;}close(fd);dcp=-1;
 }
 if(dcp<0||!res){fprintf(stderr,"No DCP DRM device\n");return 1;}
 drmModeConnector *conn=NULL;
 for(int i=0;i<res->count_connectors;i++){drmModeConnector *c=drmModeGetConnector(dcp,res->connectors[i]);if(c&&c->connection==DRM_MODE_CONNECTED&&c->count_modes){conn=c;break;}if(c)drmModeFreeConnector(c);}
 if(!conn){fprintf(stderr,"No connected DCP connector\n");return 1;}
 drmModeModeInfo mode=conn->modes[0];
 for(int i=0;i<conn->count_modes;i++)if(conn->modes[i].type&DRM_MODE_TYPE_PREFERRED){mode=conn->modes[i];break;}
 perf_width=mode.hdisplay;perf_height=mode.vdisplay;
 if(perf_width%64){fprintf(stderr,"Benchmark requires 256-byte aligned rows\n");return 1;}
 uint32_t crtc=0;for(int i=0;i<conn->count_encoders&&!crtc;i++){drmModeEncoder *e=drmModeGetEncoder(dcp,conn->encoders[i]);if(!e)continue;for(int k=0;k<res->count_crtcs;k++)if(e->possible_crtcs&(1u<<k)){crtc=res->crtcs[k];break;}drmModeFreeEncoder(e);}
 if(!crtc)abort();
 if(drmSetMaster(dcp)){perror("drmSetMaster");return 1;}
 drmModeCrtc *old=drmModeGetCrtc(dcp,crtc);
 printf("DCP_BENCH mode=%s %ux%u clock=%u connector=%u crtc=%u clients=%u frames=%u offscreen=%d dma_buf=1 cpu_copy=0\n",mode.name,perf_width,perf_height,mode.clock,conn->connector_id,crtc,clients,perf_frames,offscreen);
 int channels[2][2];pid_t children[2];uint32_t fbs[2]={0},handles[2]={0};
 for(unsigned i=0;i<clients;i++) {
  if(socketpair(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0,channels[i]))abort();
  children[i]=fork();if(children[i]<0)abort();
  if(!children[i]) {close(dcp);for(unsigned j=0;j<i;j++)close(channels[j][0]);close(channels[i][0]);client_socket=channels[i][1];int rc=client_main(argc,argv);close(client_socket);_exit(rc);}
  close(channels[i][1]);
 }
 for(unsigned i=0;i<clients;i++) {
  int fd=recv_dma_fd(channels[i][0]);if(drmPrimeFDToHandle(dcp,fd,&handles[i])){perror("prime import");return 1;}close(fd);
  uint32_t hs[4]={handles[i],0},pitches[4]={perf_width*4,0},offsets[4]={0};
  if(drmModeAddFB2(dcp,perf_width,perf_height,DRM_FORMAT_ABGR8888,hs,pitches,offsets,&fbs[i],0)){perror("AddFB2");return 1;}
 }
 uint64_t *samples=calloc(perf_frames*clients,sizeof(*samples));if(!samples)abort();
 uint64_t start=now_ns(),gpu_total=0;unsigned measured=0,flips=0;bool installed=false;drmEventContext event={.version=2,.page_flip_handler=flip_done};
 for(unsigned pass=0;pass<perf_frames;pass++)for(unsigned i=0;i<clients;i++) {
  if(pass==3&&i==0){start=now_ns();gpu_total=0;measured=0;}
  exact_write(channels[i][0],&pass,sizeof(pass));uint64_t gpu_ns;exact_read(channels[i][0],&gpu_ns,sizeof(gpu_ns));
  if(!offscreen) {
   if(!installed){if(drmModeSetCrtc(dcp,crtc,fbs[i],0,0,&conn->connector_id,1,&mode)){perror("SetCrtc");return 1;}installed=true;}
   else {
    bool done=false;if(drmModePageFlip(dcp,crtc,fbs[i],DRM_MODE_PAGE_FLIP_EVENT,&done)){perror("PageFlip");return 1;}
    while(!done){struct pollfd p={.fd=dcp,.events=POLLIN};int rc=poll(&p,1,5000);if(rc<0&&errno==EINTR)continue;if(rc<=0||p.revents&(POLLERR|POLLHUP)){fprintf(stderr,"PageFlip event timeout/error\n");return 1;}if(drmHandleEvent(dcp,&event)){perror("HandleEvent");return 1;}}
    flips++;
   }
  }
  if(pass>=3){gpu_total+=gpu_ns;measured++;}
  samples[pass*clients+i]=gpu_ns;
 }
 uint64_t elapsed=now_ns()-start;
 for(unsigned pass=0;pass<perf_frames;pass++)for(unsigned i=0;i<clients;i++)
  printf("DCP_SAMPLE pass=%u client=%u gpu_us=%.3f\n",pass,i,samples[pass*clients+i]/1000.0);
 free(samples);
 for(unsigned i=0;i<clients;i++){int status;if(waitpid(children[i],&status,0)<0||!WIFEXITED(status)||WEXITSTATUS(status)){fprintf(stderr,"Vulkan child failed\n");return 1;}close(channels[i][0]);}
 if(installed&&old&&old->mode_valid){if(drmModeSetCrtc(dcp,old->crtc_id,old->buffer_id,old->x,old->y,&conn->connector_id,1,&old->mode)){perror("restore crtc");return 1;}}
 else if(installed){if(drmModeSetCrtc(dcp,crtc,0,0,0,NULL,0,NULL)){perror("disable crtc");return 1;}}
 for(unsigned i=0;i<clients;i++){drmModeRmFB(dcp,fbs[i]);struct drm_gem_close c={.handle=handles[i]};drmIoctl(dcp,DRM_IOCTL_GEM_CLOSE,&c);}
 printf("DCP_RESULT frames=%u flips=%u measured=%u elapsed_ms=%.3f fps=%.3f gpu_mean_us=%.3f clients=%u offscreen=%d PASS\n",perf_frames*clients,flips,measured,elapsed/1e6,measured*1e9/elapsed,gpu_total/1000.0/measured,clients,offscreen);
 if(old)drmModeFreeCrtc(old);
 drmModeFreeConnector(conn);drmModeFreeResources(res);drmDropMaster(dcp);close(dcp);return 0;
}
