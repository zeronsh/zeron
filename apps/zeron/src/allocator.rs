//! The global allocator on macOS: mimalloc, with its memory labelled as heap.
//!
//! mimalloc v2 labels every region it maps with the VM tag in its `os_tag`
//! option, 100 by default. On macOS tag 100 is `VM_MEMORY_IOACCELERATOR`
//! (`mach/vm_statistics.h`), so `footprint`, `vmmap` and Activity Monitor
//! showed the whole Rust heap as GPU memory ("IOAccelerator"). Tag 254 is in
//! the range Apple leaves to applications (`VM_MEMORY_APPLICATION_SPECIFIC_1`
//! ..`_16`, 240–255) and is mimalloc's own fallback, so the heap reads as
//! "Memory Tag 254". It is set before the first allocation: regions keep the
//! tag they were mapped with.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicBool, Ordering};

/// The VM tag mimalloc maps the heap with.
pub const HEAP_VM_TAG: std::ffi::c_long = 254;

static TAGGED: AtomicBool = AtomicBool::new(false);

/// [`mimalloc::MiMalloc`], tagging its regions [`HEAP_VM_TAG`] from the first
/// allocation on.
pub struct Allocator;

#[inline]
fn tag_heap() {
    if !TAGGED.load(Ordering::Relaxed) {
        // Only stores the option (no allocation); racing threads store the
        // same value.
        unsafe { libmimalloc_sys::mi_option_set(libmimalloc_sys::mi_option_os_tag, HEAP_VM_TAG) };
        TAGGED.store(true, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for Allocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        tag_heap();
        unsafe { mimalloc::MiMalloc.alloc(layout) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        tag_heap();
        unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { mimalloc::MiMalloc.dealloc(ptr, layout) }
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Only memory already allocated (and so already tagged) gets here.
        unsafe { mimalloc::MiMalloc.realloc(ptr, layout, new_size) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `struct vm_region_extended_info` (`mach/vm_region.h`).
    #[repr(C)]
    #[derive(Default)]
    struct RegionInfo {
        protection: i32,
        user_tag: u32,
        pages_resident: u32,
        pages_shared_now_private: u32,
        pages_swapped_out: u32,
        pages_dirtied: u32,
        ref_count: u32,
        shadow_depth: u16,
        external_pager: u8,
        share_mode: u8,
        pages_reusable: u32,
    }

    const VM_REGION_EXTENDED_INFO: i32 = 13;

    unsafe extern "C" {
        /// What `mach_task_self()` reads (`mach/mach_init.h`).
        static mach_task_self_: libc::mach_port_t;
        fn mach_vm_region(
            task: libc::mach_port_t,
            address: *mut u64,
            size: *mut u64,
            flavor: i32,
            info: *mut RegionInfo,
            count: *mut u32,
            object_name: *mut libc::mach_port_t,
        ) -> i32;
    }

    /// The VM tag of the region holding `ptr`, as `vmmap` reads it.
    fn vm_tag(ptr: *const u8) -> u32 {
        let mut address = ptr as u64;
        let mut size = 0u64;
        let mut info = RegionInfo::default();
        let mut count = (std::mem::size_of::<RegionInfo>() / 4) as u32;
        let mut object = 0;
        let kr = unsafe {
            mach_vm_region(
                mach_task_self_,
                &mut address,
                &mut size,
                VM_REGION_EXTENDED_INFO,
                &mut info,
                &mut count,
                &mut object,
            )
        };
        assert_eq!(kr, 0, "mach_vm_region failed");
        assert!(
            address <= ptr as u64 && (ptr as u64) < address + size,
            "the region found holds the allocation"
        );
        info.user_tag
    }

    /// The heap never reads as GPU memory: small and large allocations alike
    /// sit in regions tagged as heap, not `VM_MEMORY_IOACCELERATOR` (100).
    #[test]
    fn the_heap_is_tagged_as_heap_not_gpu_memory() {
        let small = Box::new([1u8; 64]);
        let large = vec![1u8; 64 << 20];
        for (what, ptr) in [("small", small.as_ptr()), ("large", large.as_ptr())] {
            assert_eq!(
                vm_tag(ptr),
                HEAP_VM_TAG as u32,
                "{what} allocation's region tag"
            );
        }
    }
}
