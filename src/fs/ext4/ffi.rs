use core::ffi::{c_char, c_void};
use alloc::ffi::CString;

use crate::kernel::errno::Errno;
use super::superblock::Ext4SuperBlock;

unsafe extern "C" {
    /*
    int kernelx_ext4_register_block_device(
        uint32_t block_size,
        uint64_t block_count,
        uintptr_t f_open,
        uintptr_t f_bread,
        uintptr_t f_bwrite,
        uintptr_t f_close,
        void *user,
        struct ext4_fs **return_fs
    )
     */
    fn kernelx_ext4_create_filesystem(
        block_size : u32,
        block_count: u64,
        f_open:   usize,
        f_bread:  usize,
        f_bwrite: usize,
        f_close:  usize,
        user: *mut c_void,
        return_fs: *mut usize
    ) -> i32;

    /*
    int kernelx_ext4_destroy_filesystem(
        struct ext4_fs *fs
    )
     */
    fn kernelx_ext4_destroy_filesystem(
        fs: *mut c_void
    ) -> i32;

    /*
    int kernelx_ext4_get_inode(
        struct ext4_fs *fs,
        uint32_t ino,
        struct ext4_inode_ref **ret_inode
    )
     */
    fn kernelx_ext4_get_inode(
        fs: *mut c_void,
        ino: u32,
        ret_inode: *mut usize
    ) -> i32;

    /*
    int kernelx_ext4_put_inode(
        struct ext4_inode_ref *inode_ref
    )
     */
    fn kernelx_ext4_put_inode(
        inode_ref: *mut c_void
    ) -> i32;

    /*
    int kernelx_ext4_inode_lookup(
        struct ext4_inode_ref *inode,
        const char *name,
        uint32_t *ret_ino
    )
     */
    fn kernelx_ext4_inode_lookup(
        inode: *mut c_void,
        name: *const c_char,
        ret_ino: *mut u32
    ) -> i32;

    /*
    ssize_t kernelx_ext4_inode_readat(
        struct ext4_inode_ref *inode,
        void *buf,
        size_t size,
        size_t offset
    )
    */
    fn kernelx_ext4_inode_readat(
        inode: *mut c_void,
        buf: *mut c_void,
        size: usize,
        offset: usize
    ) -> isize;

    /*
    ssize_t kernelx_ext4_inode_writeat(
        struct ext4_inode_ref *inode_ref,
        const void *buf,
        size_t size,
        size_t fpos
    )
     */
    fn kernelx_ext4_inode_writeat(
        inode_ref: *mut c_void,
        buf: *const c_void,
        size: usize,
        fpos: usize
    ) -> isize;

    /*
    ssize_t kernelx_ext4_get_inode_size(
    struct ext4_inode_ref *inode_ref
    )
     */
    fn kernelx_ext4_get_inode_size(
        inode_ref: *mut c_void
    ) -> isize;
}

pub enum Ext4Errno {
    EOK = 0,
}

#[inline(always)]
pub fn create_filesystem(
    block_size : u32,
    block_count: u64,
    f_open:   usize,
    f_bread:  usize,
    f_bwrite: usize,
    f_close:  usize,
    superblock: *mut Ext4SuperBlock,
) -> Result<usize, Errno> {
    let mut fs = 0usize;
    
    let rc = unsafe {
        kernelx_ext4_create_filesystem(
            block_size,
            block_count,
            f_open,
            f_bread,
            f_bwrite,
            f_close,
            superblock as *mut c_void,
            &mut fs
        )
    };

    if rc != Ext4Errno::EOK as i32 {
        Err(Errno::from(-rc))
    } else {
        Ok(fs)
    }
}

#[inline(always)]
pub fn destroy_filesystem(fs: usize) -> Result<(), Errno> {
    let rc = unsafe { kernelx_ext4_destroy_filesystem(fs as *mut c_void) };
    if rc != Ext4Errno::EOK as i32 {
        Err(Errno::from(-rc))
    } else {
        Ok(())
    }
}

#[inline(always)]
pub fn get_inode_handler(
    fs_handler: usize,
    ino: u32,
) -> Result<usize, Errno> {
    let mut inode_handler = 0;
    let rc = unsafe { 
        kernelx_ext4_get_inode(fs_handler as *mut c_void, ino, &mut inode_handler) 
    };

    if rc != Ext4Errno::EOK as i32 {
        Err(Errno::from(-rc))
    } else {
        Ok(inode_handler)
    }
}

#[inline(always)]
pub fn put_inode_handler(
    inode_handler: usize
) -> Result<(), Errno> {
    let rc = unsafe { 
        kernelx_ext4_put_inode(inode_handler as *mut c_void) 
    };

    if rc != Ext4Errno::EOK as i32 {
        Err(Errno::from(-rc))
    } else {
        Ok(())
    }
}

#[inline(always)]
pub fn inode_lookup(
    inode_handler: usize,
    name: &str,
) -> Result<u32, Errno> {
    let mut result = 0u32;
        
    let name = CString::new(name).unwrap();
    let r = unsafe {
        kernelx_ext4_inode_lookup(
            inode_handler as *mut c_void,
            name.as_ptr(),
            &mut result
        )
    };

    if r != Ext4Errno::EOK as i32 {
        return Err(Errno::from(-r));
    }
    
    Ok(result)
}

#[inline(always)]
pub fn inode_readat(
    inode_handler: usize,
    buf: &mut [u8],
    offset: usize
) -> Result<usize, Errno> {
    let r = unsafe {
        kernelx_ext4_inode_readat(
            inode_handler as *mut c_void,
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
            offset
        )
    };

    if r < 0 {
        Err(Errno::from(-r as i32))
    } else {
        Ok(r as usize)
    }
}

#[inline(always)]
pub fn inode_writeat(
    inode_handler: usize,
    buf: &[u8],
    offset: usize
) -> Result<usize, Errno> {
    let r = unsafe {
        kernelx_ext4_inode_writeat(
            inode_handler as *mut c_void,
            buf.as_ptr() as *const c_void,
            buf.len(),
            offset
        )
    };
    if r < 0 {
        Err(Errno::from(-r as i32))
    } else {
        Ok(r as usize)
    }
}

#[inline(always)]
pub fn inode_get_size(
    inode_handler: usize
) -> Result<usize, Errno> {
    let r = unsafe {
        kernelx_ext4_get_inode_size(inode_handler as *mut c_void)
    };
    
    if r < 0 {
        Err(Errno::from(-r as i32))
    } else {
        Ok(r as usize)
    }
}
