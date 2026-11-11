/* SPDX-License-Identifier: MIT */
#ifndef APPLE9_TEST_DEVICE_H
#define APPLE9_TEST_DEVICE_H

#include <stdbool.h>
#include <string.h>
#include <vulkan/vulkan.h>

/* Test the native Apple9 implementations without depending on a marketing
 * name. Keep software implementations from satisfying a hardware test. */
static inline bool
apple9_test_native_device(const VkPhysicalDeviceProperties *props)
{
   return props->deviceType == VK_PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU &&
          (strstr(props->deviceName, "(G16G ") ||
           strstr(props->deviceName, "(G16C ") ||
           strstr(props->deviceName, "(G17P "));
}

#endif
