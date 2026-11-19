// SPDX-License-Identifier: GPL-2.0-only
// Independent Vulkan processes alternate GPU copies through a token ring.
// Setup/warmup and complete CPU output checks are outside submit/fence timings.
#define _POSIX_C_SOURCE 200809L
#include <vulkan/vulkan.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>

#define MAX_CLIENTS 8
#define BYTES 65536
#define VK(call) do { VkResult result = (call); if (result != VK_SUCCESS) { \
    fprintf(stderr, "%s: VkResult %d\n", #call, result); exit(1); } } while (0)
struct buffer { VkBuffer buffer; VkDeviceMemory memory; uint32_t *map; };
struct result { uint32_t client, iterations; uint64_t sum, p50, p95, p99, max; };
static uint64_t now(void)
{
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC, &t)) exit(1);
    return (uint64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}
static void transfer(int fd, void *data, size_t size, int writing)
{
    unsigned char *p = data;
    while (size) {
        ssize_t n = writing ? write(fd, p, size) : read(fd, p, size);
        if (n <= 0) { perror(writing ? "write" : "read"); exit(1); }
        p += n; size -= n;
    }
}
static int compare(const void *a, const void *b)
{
    uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
    return (x > y) - (x < y);
}
static struct buffer make_buffer(VkDevice device, VkPhysicalDevice gpu)
{
    struct buffer b = {0};
    VkBufferCreateInfo info = { .sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
        .size = BYTES, .usage = VK_BUFFER_USAGE_TRANSFER_SRC_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT };
    VK(vkCreateBuffer(device, &info, NULL, &b.buffer));
    VkMemoryRequirements requirements;
    VkPhysicalDeviceMemoryProperties properties;
    vkGetBufferMemoryRequirements(device, b.buffer, &requirements);
    vkGetPhysicalDeviceMemoryProperties(gpu, &properties);
    uint32_t type;
    for (type = 0; type < properties.memoryTypeCount; type++) {
        VkMemoryPropertyFlags flags = VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT;
        if ((requirements.memoryTypeBits & (1u << type)) &&
            (properties.memoryTypes[type].propertyFlags & flags) == flags) break;
    }
    if (type == properties.memoryTypeCount) { fprintf(stderr, "No coherent memory type\n"); exit(1); }
    VkMemoryAllocateInfo allocation = { .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
        .allocationSize = requirements.size, .memoryTypeIndex = type };
    VK(vkAllocateMemory(device, &allocation, NULL, &b.memory));
    VK(vkBindBufferMemory(device, b.buffer, b.memory, 0));
    VK(vkMapMemory(device, b.memory, 0, BYTES, 0, (void **)&b.map));
    return b;
}
static void child(unsigned index, unsigned clients, unsigned iterations,
                  int tokens[MAX_CLIENTS][2], int ready, int done, int results)
{
    VkApplicationInfo app = { .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
        .pApplicationName = "Asahi client switch", .apiVersion = VK_API_VERSION_1_0 };
    VkInstanceCreateInfo ii = { .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &app };
    VkInstance instance; VK(vkCreateInstance(&ii, NULL, &instance));
    uint32_t count = 0; VK(vkEnumeratePhysicalDevices(instance, &count, NULL));
    if (!count || count > 16) exit(1);
    VkPhysicalDevice devices[16], gpu = VK_NULL_HANDLE;
    VK(vkEnumeratePhysicalDevices(instance, &count, devices));
    for (unsigned i = 0; i < count; i++) {
        VkPhysicalDeviceProperties p; vkGetPhysicalDeviceProperties(devices[i], &p);
        if (p.deviceType == VK_PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU &&
            !strncmp(p.deviceName, "Apple ", 6)) {
            gpu = devices[i];
            if (!index) fprintf(stderr, "GPU=%s vendor=%04x device=%04x\n", p.deviceName, p.vendorID, p.deviceID);
            break;
        }
    }
    if (!gpu) { fprintf(stderr, "FAIL: no hardware Apple Vulkan device\n"); exit(1); }
    uint32_t families = 0; vkGetPhysicalDeviceQueueFamilyProperties(gpu, &families, NULL);
    if (!families || families > 32) exit(1);
    VkQueueFamilyProperties properties[32];
    vkGetPhysicalDeviceQueueFamilyProperties(gpu, &families, properties);
    uint32_t family;
    for (family = 0; family < families; family++)
        if (properties[family].queueCount && (properties[family].queueFlags & VK_QUEUE_COMPUTE_BIT)) break;
    if (family == families) exit(1);
    float priority = 1.0f;
    VkDeviceQueueCreateInfo qi = { .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
        .queueFamilyIndex = family, .queueCount = 1, .pQueuePriorities = &priority };
    VkDeviceCreateInfo di = { .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
        .queueCreateInfoCount = 1, .pQueueCreateInfos = &qi };
    VkDevice device; VK(vkCreateDevice(gpu, &di, NULL, &device));
    VkQueue queue; vkGetDeviceQueue(device, family, 0, &queue);
    struct buffer src = make_buffer(device, gpu), dst = make_buffer(device, gpu);
    VkCommandPoolCreateInfo pi = { .sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
        .queueFamilyIndex = family };
    VkCommandPool pool; VK(vkCreateCommandPool(device, &pi, NULL, &pool));
    VkCommandBufferAllocateInfo ai = { .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
        .commandPool = pool, .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY, .commandBufferCount = 1 };
    VkCommandBuffer command; VK(vkAllocateCommandBuffers(device, &ai, &command));
    VkCommandBufferBeginInfo bi = { .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO };
    VK(vkBeginCommandBuffer(command, &bi));
    VkBufferMemoryBarrier barrier = { .sType = VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER,
        .srcAccessMask = VK_ACCESS_HOST_WRITE_BIT, .dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
        .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED, .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .buffer = src.buffer, .size = BYTES };
    vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_HOST_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
        0, 0, NULL, 1, &barrier, 0, NULL);
    VkBufferCopy copy = { .size = BYTES };
    vkCmdCopyBuffer(command, src.buffer, dst.buffer, 1, &copy);
    barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT; barrier.dstAccessMask = VK_ACCESS_HOST_READ_BIT;
    barrier.buffer = dst.buffer;
    vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_HOST_BIT,
        0, 0, NULL, 1, &barrier, 0, NULL);
    VK(vkEndCommandBuffer(command));
    VkFenceCreateInfo fi = { .sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO };
    VkFence fence; VK(vkCreateFence(device, &fi, NULL, &fence));
    VkSubmitInfo si = { .sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
        .commandBufferCount = 1, .pCommandBuffers = &command };
    uint64_t *samples = calloc(iterations, sizeof(*samples)); if (!samples) exit(1);
    unsigned char token = 1; transfer(ready, &token, 1, 1);
    for (unsigned phase = 0; phase < 2; phase++) {
        unsigned rounds = phase ? iterations : 8;
        for (unsigned frame = 0; frame < rounds; frame++) {
            transfer(tokens[index][0], &token, 1, 0);
            for (unsigned word = 0; word < BYTES / 4; word++) {
                src.map[word] = (index + 1) * 0x1234567u ^ (frame + phase * 65536) * 0x9e3779b9u ^ word;
                dst.map[word] = ~src.map[word];
            }
            uint64_t begin = now();
            VK(vkResetFences(device, 1, &fence));
            VK(vkQueueSubmit(queue, 1, &si, fence));
            VK(vkWaitForFences(device, 1, &fence, VK_TRUE, 5000000000ull));
            uint64_t elapsed = now() - begin;
            if (phase) samples[frame] = elapsed;
            if (memcmp(src.map, dst.map, BYTES)) {
                fprintf(stderr, "FAIL: client %u frame %u output mismatch\n", index, frame); exit(1);
            }
            int out = frame + 1 == rounds && index + 1 == clients ? done : tokens[(index + 1) % clients][1];
            transfer(out, &token, 1, 1);
        }
    }
    struct result result = { .client = index, .iterations = iterations };
    for (unsigned i = 0; i < iterations; i++) result.sum += samples[i];
    qsort(samples, iterations, sizeof(*samples), compare);
    result.p50 = samples[(iterations - 1) * 50 / 100]; result.p95 = samples[(iterations - 1) * 95 / 100];
    result.p99 = samples[(iterations - 1) * 99 / 100]; result.max = samples[iterations - 1];
    VK(vkDeviceWaitIdle(device));
    vkDestroyFence(device, fence, NULL); vkDestroyCommandPool(device, pool, NULL);
    vkUnmapMemory(device, src.memory); vkUnmapMemory(device, dst.memory);
    vkDestroyBuffer(device, src.buffer, NULL); vkDestroyBuffer(device, dst.buffer, NULL);
    vkFreeMemory(device, src.memory, NULL); vkFreeMemory(device, dst.memory, NULL);
    vkDestroyDevice(device, NULL); vkDestroyInstance(instance, NULL); free(samples);
    transfer(results, &result, sizeof(result), 1);
    exit(0);
}
int main(int argc, char **argv)
{
    unsigned clients = argc > 1 ? strtoul(argv[1], NULL, 0) : 2;
    unsigned iterations = argc > 2 ? strtoul(argv[2], NULL, 0) : 64;
    if (!clients || clients > MAX_CLIENTS || !iterations || iterations > 4096) return 2;
    int tokens[MAX_CLIENTS][2], ready[2], done[2], results[2]; pid_t children[MAX_CLIENTS];
    for (unsigned i = 0; i < clients; i++) if (pipe(tokens[i])) return 1;
    if (pipe(ready) || pipe(done) || pipe(results)) return 1;
    for (unsigned i = 0; i < clients; i++) {
        children[i] = fork(); if (children[i] < 0) return 1;
        if (!children[i]) child(i, clients, iterations, tokens, ready[1], done[1], results[1]);
    }
    close(ready[1]); close(done[1]); close(results[1]);
    unsigned char token = 1;
    for (unsigned i = 0; i < clients; i++) transfer(ready[0], &token, 1, 0);
    uint64_t elapsed = 0;
    for (unsigned phase = 0; phase < 2; phase++) {
        uint64_t begin = now(); transfer(tokens[0][1], &token, 1, 1); transfer(done[0], &token, 1, 0);
        if (phase) elapsed = now() - begin;
    }
    for (unsigned i = 0; i < clients; i++) {
        struct result result; transfer(results[0], &result, sizeof(result), 0);
        printf("{\"client\":%u,\"iterations\":%u,\"mean_ns\":%llu,\"p50_ns\":%llu,\"p95_ns\":%llu,\"p99_ns\":%llu,\"max_ns\":%llu}\n",
            result.client, result.iterations, (unsigned long long)(result.sum / result.iterations),
            (unsigned long long)result.p50, (unsigned long long)result.p95,
            (unsigned long long)result.p99, (unsigned long long)result.max);
    }
    int failed = 0;
    for (unsigned i = 0; i < clients; i++) {
        int status; if (waitpid(children[i], &status, 0) < 0 || !WIFEXITED(status) || WEXITSTATUS(status)) failed = 1;
    }
    if (failed) return 1;
    printf("{\"result\":\"PASS\",\"clients\":%u,\"jobs\":%u,\"elapsed_ns_including_oracles_and_ipc\":%llu,\"bytes_per_copy\":%u}\n",
        clients, clients * iterations, (unsigned long long)elapsed, BYTES);
    return 0;
}
