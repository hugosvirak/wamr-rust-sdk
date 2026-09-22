use std::{
    alloc::{GlobalAlloc, Layout},
    ffi::c_void,
    ptr::{self, null_mut},
};

use embedded_alloc::LlffHeap;
use wamr_sys::{mem_alloc_usage_t, mem_alloc_usage_t_Alloc_For_LinearMemory};

const ALIGNMENT: usize = std::mem::align_of::<AllocationHeader>();

#[repr(C)]
struct AllocationHeader {
    size: usize,
}

pub struct CustomMemoryPoolData {
    pub linear_memory_allocator: LlffHeap,
    pub linear_memory_pool: Option<Box<[u8]>>,

    pub runtime_memory_allocator: LlffHeap,
    pub runtime_memory_pool: Option<Box<[u8]>>,
}

impl Default for CustomMemoryPoolData {
    fn default() -> Self {
        Self {
            linear_memory_allocator: LlffHeap::empty(),
            linear_memory_pool: None,

            runtime_memory_allocator: LlffHeap::empty(),
            runtime_memory_pool: None,
        }
    }
}

impl CustomMemoryPoolData {
    #[inline]
    fn allocator(&self, usage: mem_alloc_usage_t) -> &LlffHeap {
        if usage == mem_alloc_usage_t_Alloc_For_LinearMemory {
            &self.linear_memory_allocator
        } else {
            &self.runtime_memory_allocator
        }
    }

    pub fn init(&self) {
        if let Some(ref linear_memory_pool) = self.linear_memory_pool {
            unsafe {
                self.linear_memory_allocator.init(
                    linear_memory_pool.as_ptr() as usize,
                    linear_memory_pool.len(),
                );
            }
        }

        if let Some(ref runtime_memory_pool) = self.runtime_memory_pool {
            unsafe {
                self.runtime_memory_allocator.init(
                    runtime_memory_pool.as_ptr() as usize,
                    runtime_memory_pool.len(),
                );
            }
        }
    }
}

unsafe fn allocate(
    data: &CustomMemoryPoolData,
    usage: mem_alloc_usage_t,
    size: usize,
) -> *mut c_void {
    let header_size = std::mem::size_of::<AllocationHeader>();

    let total_size = match header_size.checked_add(size) {
        Some(size) => size,
        None => return null_mut(),
    };

    let layout = match Layout::from_size_align(total_size, ALIGNMENT) {
        Ok(layout) => layout,
        Err(_) => return null_mut(),
    };

    let allocator = data.allocator(usage);

    let raw = allocator.alloc(layout);

    if raw.is_null() {
        return null_mut();
    }

    let header = raw as *mut AllocationHeader;

    ptr::write(header, AllocationHeader { size });

    raw.add(header_size) as *mut c_void
}

#[inline]
unsafe fn get_header(ptr: *mut c_void) -> *mut AllocationHeader {
    let header_size = std::mem::size_of::<AllocationHeader>();

    (ptr as *mut u8).sub(header_size) as *mut AllocationHeader
}

pub unsafe extern "C" fn malloc_func(
    usage: mem_alloc_usage_t,
    user_data: *mut c_void,
    size: u32,
) -> *mut c_void {
    if user_data.is_null() {
        return null_mut();
    }

    let data = &*(user_data as *const CustomMemoryPoolData);

    allocate(data, usage, size as usize)
}

pub unsafe extern "C" fn free_func(
    usage: mem_alloc_usage_t,
    user_data: *mut c_void,
    ptr: *mut c_void,
) {
    if user_data.is_null() || ptr.is_null() {
        return;
    }

    let data = &*(user_data as *const CustomMemoryPoolData);

    let header_ptr = get_header(ptr);

    // Read the allocation size before freeing the memory.
    let header = ptr::read(header_ptr);

    let header_size = std::mem::size_of::<AllocationHeader>();

    let total_size = match header_size.checked_add(header.size) {
        Some(size) => size,
        None => return,
    };

    let layout = match Layout::from_size_align(total_size, ALIGNMENT) {
        Ok(layout) => layout,
        Err(_) => return,
    };

    let allocator = data.allocator(usage);

    allocator.dealloc(header_ptr as *mut u8, layout);
}

pub unsafe extern "C" fn realloc_func(
    usage: mem_alloc_usage_t,
    _full_size_mmaped: bool,
    user_data: *mut c_void,
    ptr: *mut c_void,
    size: u32,
) -> *mut c_void {
    if user_data.is_null() {
        return null_mut();
    }

    // realloc(NULL, size) behaves like malloc(size).
    if ptr.is_null() {
        return malloc_func(usage, user_data, size);
    }

    // realloc(ptr, 0) behaves like free(ptr).
    if size == 0 {
        free_func(usage, user_data, ptr);
        return null_mut();
    }

    let data = &*(user_data as *const CustomMemoryPoolData);

    let header_ptr = get_header(ptr);
    let old_header = &*header_ptr;
    let old_size = old_header.size;

    let header_size = std::mem::size_of::<AllocationHeader>();

    let old_total_size = match header_size.checked_add(old_size) {
        Some(size) => size,
        None => return null_mut(),
    };

    let new_size = size as usize;

    let new_total_size = match header_size.checked_add(new_size) {
        Some(size) => size,
        None => return null_mut(),
    };

    let old_layout = match Layout::from_size_align(old_total_size, ALIGNMENT) {
        Ok(layout) => layout,
        Err(_) => return null_mut(),
    };

    let allocator = data.allocator(usage);

    let new_raw =
        allocator.realloc(header_ptr as *mut u8, old_layout, new_total_size);

    if new_raw.is_null() {
        // Original allocation remains valid.
        return null_mut();
    }

    // realloc may move the allocation, so update the header.
    let new_header = new_raw as *mut AllocationHeader;

    ptr::write(new_header, AllocationHeader { size: new_size });

    new_raw.add(header_size) as *mut c_void
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    use wamr_sys::mem_alloc_usage_t_Alloc_For_Runtime;

    const LINEAR_POOL_SIZE: usize = 64 * 1024;
    const RUNTIME_POOL_SIZE: usize = 64 * 1024;

    fn create_test_data() -> Box<CustomMemoryPoolData> {
        let data = Box::new(CustomMemoryPoolData {
            linear_memory_allocator: LlffHeap::empty(),
            linear_memory_pool: Some(
                vec![0u8; LINEAR_POOL_SIZE].into_boxed_slice(),
            ),

            runtime_memory_allocator: LlffHeap::empty(),
            runtime_memory_pool: Some(
                vec![0u8; RUNTIME_POOL_SIZE].into_boxed_slice(),
            ),
        });

        data.init();

        data
    }

    fn user_data(data: &CustomMemoryPoolData) -> *mut c_void {
        data as *const CustomMemoryPoolData as *mut c_void
    }

    #[test]
    fn alloc_func_allocates_memory() {
        let data = create_test_data();

        let ptr = unsafe {
            malloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data(&data),
                128,
            )
        };

        assert!(!ptr.is_null());

        unsafe {
            ptr::write_bytes(ptr, 0xAB, 128);

            free_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data(&data),
                ptr,
            );
        }
    }

    #[test]
    fn alloc_and_free_reuses_memory() {
        let data = create_test_data();
        let user_data = user_data(&data);

        let ptr = unsafe {
            malloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                256,
            )
        };

        assert!(!ptr.is_null());

        unsafe {
            ptr::write_bytes(ptr, 0xCD, 256);

            free_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                ptr,
            );
        }

        let ptr2 = unsafe {
            malloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                256,
            )
        };

        assert!(!ptr2.is_null());

        unsafe {
            free_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                ptr2,
            );
        }
    }

    #[test]
    fn realloc_func_preserves_data_when_growing() {
        let data = create_test_data();
        let user_data = user_data(&data);

        let ptr = unsafe {
            malloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                128,
            )
        };

        assert!(!ptr.is_null());

        unsafe {
            for i in 0..128 {
                *(ptr as *mut u8).add(i) = i as u8;
            }
        }

        let new_ptr = unsafe {
            realloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                false,
                user_data,
                ptr,
                256,
            )
        };

        assert!(!new_ptr.is_null());

        unsafe {
            for i in 0..128 {
                assert_eq!(
                    *(new_ptr as *const u8).add(i),
                    i as u8
                );
            }

            free_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                new_ptr,
            );
        }
    }

    #[test]
    fn realloc_func_preserves_data_when_shrinking() {
        let data = create_test_data();
        let user_data = user_data(&data);

        let ptr = unsafe {
            malloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                256,
            )
        };

        assert!(!ptr.is_null());

        unsafe {
            for i in 0..256 {
                *(ptr as *mut u8).add(i) = (i & 0xFF) as u8;
            }
        }

        let new_ptr = unsafe {
            realloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                false,
                user_data,
                ptr,
                128,
            )
        };

        assert!(!new_ptr.is_null());

        unsafe {
            for i in 0..128 {
                assert_eq!(
                    *(new_ptr as *const u8).add(i),
                    (i & 0xFF) as u8
                );
            }

            free_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                new_ptr,
            );
        }
    }

    #[test]
    fn realloc_null_behaves_like_malloc() {
        let data = create_test_data();
        let user_data = user_data(&data);

        let ptr = unsafe {
            realloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                false,
                user_data,
                ptr::null_mut(),
                128,
            )
        };

        assert!(!ptr.is_null());

        unsafe {
            free_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                ptr,
            );
        }
    }

    #[test]
    fn realloc_zero_frees_memory() {
        let data = create_test_data();
        let user_data = user_data(&data);

        let ptr = unsafe {
            malloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                128,
            )
        };

        assert!(!ptr.is_null());

        let result = unsafe {
            realloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                false,
                user_data,
                ptr,
                0,
            )
        };

        assert!(result.is_null());

        let ptr2 = unsafe {
            malloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                128,
            )
        };

        assert!(!ptr2.is_null());

        unsafe {
            free_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                ptr2,
            );
        }
    }

    #[test]
    fn runtime_memory_uses_runtime_allocator() {
        let data = create_test_data();
        let user_data = user_data(&data);

        let ptr = unsafe {
            malloc_func(
                mem_alloc_usage_t_Alloc_For_Runtime,
                user_data,
                128,
            )
        };

        assert!(!ptr.is_null());

        unsafe {
            free_func(
                mem_alloc_usage_t_Alloc_For_Runtime,
                user_data,
                ptr,
            );
        }
    }

    #[test]
    fn multiple_allocations_and_frees() {
        let data = create_test_data();
        let user_data = user_data(&data);

        let mut allocations = Vec::new();

        for size in [16, 32, 64, 128, 256, 512, 1024] {
            let ptr = unsafe {
                malloc_func(
                    mem_alloc_usage_t_Alloc_For_LinearMemory,
                    user_data,
                    size,
                )
            };

            assert!(
                !ptr.is_null(),
                "allocation of {} bytes failed",
                size
            );

            unsafe {
                ptr::write_bytes(ptr, 0x5A, size as usize);
            }

            allocations.push(ptr);
        }

        for ptr in allocations {
            unsafe {
                free_func(
                    mem_alloc_usage_t_Alloc_For_LinearMemory,
                    user_data,
                    ptr,
                );
            }
        }

        let ptr = unsafe {
            malloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                1024,
            )
        };

        assert!(!ptr.is_null());

        unsafe {
            free_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                ptr,
            );
        }
    }

    #[test]
    fn large_non_power_of_two_allocation() {
        let data = Box::new(CustomMemoryPoolData {
            linear_memory_allocator: LlffHeap::empty(),
            linear_memory_pool: Some(
                vec![0u8; 3 * 1024 * 1024].into_boxed_slice(),
            ),

            runtime_memory_allocator: LlffHeap::empty(),
            runtime_memory_pool: Some(
                vec![0u8; RUNTIME_POOL_SIZE].into_boxed_slice(),
            ),
        });

        data.init();

        let user_data = user_data(&data);

        // This is the kind of allocation that was problematic
        // with the buddy allocator because it was rounded to 2 MiB.
        let size = 1_114_112;

        let ptr = unsafe {
            malloc_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                size,
            )
        };

        assert!(!ptr.is_null());

        unsafe {
            ptr::write_bytes(ptr, 0xA5, size as usize);

            free_func(
                mem_alloc_usage_t_Alloc_For_LinearMemory,
                user_data,
                ptr,
            );
        }
    }
}
