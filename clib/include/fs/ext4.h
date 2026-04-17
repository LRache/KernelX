#ifndef __KERNELX_FS_EXT4_H__
#define __KERNELX_FS_EXT4_H__

/*
 * Unified entry point for KernelX ext4 helper APIs.
 *
 * Keep the per-feature headers for internal organization and incremental
 * includes, but prefer this header for callers that need the full helper
 * surface.
 */

#include <fs/ext4/inode.h>
#include <fs/ext4/link.h>
#include <fs/ext4/read.h>
#include <fs/ext4/truncate.h>
#include <fs/ext4/write.h>

#endif
