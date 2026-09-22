/*
 * Copyright (C) 2019 Intel Corporation. All rights reserved.
 * SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
 */

//! This is the main entry point for executing WebAssembly modules.
//! Every process should have only one instance of this runtime by call
//! `Runtime::new()` or `Runtime::builder().build()` once.

use std::ffi::c_void;

use wamr_sys::{
    mem_alloc_type_t_Alloc_With_Allocator, mem_alloc_type_t_Alloc_With_System_Allocator,
    wasm_runtime_destroy, wasm_runtime_full_init, wasm_runtime_init, MemAllocOption__bindgen_ty_2,
    NativeSymbol, RunningMode_Mode_Interp, RunningMode_Mode_LLVM_JIT, RuntimeInitArgs,
};

use crate::{
    alloc::{free_func, malloc_func, realloc_func, CustomMemoryPoolData},
    host_function::HostFunctionList,
    RuntimeError,
};

#[allow(dead_code)]
pub struct Runtime {
    host_functions: HostFunctionList,
    custom_memory_pool_data: Box<CustomMemoryPoolData>,
}

impl Runtime {
    /// return a `RuntimeBuilder` instance
    ///
    /// has to
    /// - select a allocation mode
    /// - select a running mode
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::default()
    }

    /// create a new `Runtime` instance with the default configuration which includes:
    /// - system allocator mode
    /// - the default running mode
    ///
    /// # Errors
    ///
    /// if the runtime initialization failed, it will return `RuntimeError::InitializationFailure`
    pub fn new() -> Result<Self, RuntimeError> {
        match unsafe { wasm_runtime_init() } {
            true => Ok(Runtime {
                host_functions: HostFunctionList::new("empty"),
                custom_memory_pool_data: Box::new(CustomMemoryPoolData::default()),
            }),
            false => Err(RuntimeError::InitializationFailure),
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            wasm_runtime_destroy();
        }
    }
}

/// The builder of `Runtime`. It is used to configure the runtime.
/// Get one via `Runtime::builder()`
pub struct RuntimeBuilder {
    args: RuntimeInitArgs,
    host_functions: HostFunctionList,
    custom_memory_pool_data: Box<CustomMemoryPoolData>,
}

/// Can't build() until config allocator mode
impl Default for RuntimeBuilder {
    fn default() -> Self {
        let args = RuntimeInitArgs::default();
        RuntimeBuilder {
            args,
            host_functions: HostFunctionList::new("host"),
            custom_memory_pool_data: Box::new(CustomMemoryPoolData::default()),
        }
    }
}

impl RuntimeBuilder {
    /// system allocator mode
    /// allocate memory from system allocator for runtime consumed memory
    pub fn use_system_allocator(mut self) -> RuntimeBuilder {
        self.args.mem_alloc_type = mem_alloc_type_t_Alloc_With_System_Allocator;
        self
    }

    // Uses custom memory pools for the runtime and linear data
    pub fn use_memory_pool(
        mut self,
        runtime_memory_pool: Box<[u8]>,
        linear_memory_pool: Box<[u8]>,
    ) -> RuntimeBuilder {
        self.custom_memory_pool_data.runtime_memory_pool = Some(runtime_memory_pool);
        self.custom_memory_pool_data.linear_memory_pool = Some(linear_memory_pool);
        self.args.mem_alloc_type = mem_alloc_type_t_Alloc_With_Allocator;
        self.args.mem_alloc_option.allocator = MemAllocOption__bindgen_ty_2 {
            malloc_func: malloc_func as *mut c_void,
            realloc_func: realloc_func as *mut c_void,
            free_func: free_func as *mut c_void,
            user_data: self.custom_memory_pool_data.as_mut() as *mut CustomMemoryPoolData
                as *mut c_void,
        };
        self
    }

    /// use interpreter mode
    pub fn run_as_interpreter(mut self) -> RuntimeBuilder {
        self.args.running_mode = RunningMode_Mode_Interp;
        self
    }

    /// use llvm-jit mode
    pub fn run_as_llvm_jit(mut self, opt_level: u32, size_level: u32) -> RuntimeBuilder {
        self.args.running_mode = RunningMode_Mode_LLVM_JIT;
        self.args.llvm_jit_opt_level = opt_level;
        self.args.llvm_jit_size_level = size_level;
        self
    }

    /// register a host function
    pub fn register_host_function(
        mut self,
        function_name: &str,
        function_ptr: *mut c_void,
    ) -> RuntimeBuilder {
        self.host_functions
            .register_host_function(function_name, function_ptr);
        self
    }

    /// create a `Runtime` instance with the configuration
    ///
    /// # Errors
    ///
    /// if the runtime initialization failed, it will return `RuntimeError::InitializationFailure`
    pub fn build(mut self) -> Result<Runtime, RuntimeError> {
        match unsafe {
            let module_name = &(self.host_functions).get_module_name();
            self.args.native_module_name = module_name.as_ptr();

            let native_symbols = &(self.host_functions).get_native_symbols();
            self.args.n_native_symbols = native_symbols.len() as u32;
            self.args.native_symbols = native_symbols.as_ptr() as *mut NativeSymbol;

            self.custom_memory_pool_data.init();

            wasm_runtime_full_init(&mut self.args)
        } {
            true => Ok(Runtime {
                host_functions: self.host_functions,
                custom_memory_pool_data: self.custom_memory_pool_data,
            }),
            false => Err(RuntimeError::InitializationFailure),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wamr_sys::{wasm_runtime_free, wasm_runtime_malloc, wasm_runtime_realloc};

    #[test]
    #[ignore]
    fn test_runtime_new() {
        let runtime = Runtime::new();
        assert!(runtime.is_ok());

        /* use malloc to confirm */
        let small_buf = unsafe { wasm_runtime_malloc(16) };
        assert!(!small_buf.is_null());
        unsafe { wasm_runtime_free(small_buf) };

        drop(runtime);

        /* runtime has been destroyed. malloc should be failed */
        let small_buf = unsafe { wasm_runtime_malloc(16) };
        assert!(small_buf.is_null());

        {
            let runtime = Runtime::new();
            assert!(runtime.is_ok());

            let runtime = Runtime::new();
            assert!(runtime.is_ok());

            let runtime = Runtime::new();
            assert!(runtime.is_ok());
        }

        /* runtime has been destroyed. malloc should be failed */
        let small_buf = unsafe { wasm_runtime_malloc(16) };
        assert!(small_buf.is_null());
    }

    #[test]
    fn test_runtime_builder_default() {
        // use Mode_Default
        let runtime = Runtime::builder().use_system_allocator().build();
        assert!(runtime.is_ok());

        let small_buf = unsafe { wasm_runtime_malloc(16) };
        assert!(!small_buf.is_null());
        unsafe { wasm_runtime_free(small_buf) };
    }

    #[test]
    fn test_runtime_builder_interpreter() {
        let runtime = Runtime::builder()
            .run_as_interpreter()
            .use_system_allocator()
            .build();
        assert!(runtime.is_ok());

        let small_buf = unsafe { wasm_runtime_malloc(16) };
        assert!(!small_buf.is_null());
        unsafe { wasm_runtime_free(small_buf) };
    }

    #[test]
    #[cfg(feature = "llvmjit")]
    #[ignore]
    fn test_runtime_builder_llvm_jit() {
        let runtime = Runtime::builder()
            .run_as_llvm_jit(3, 3)
            .use_system_allocator()
            .build();
        assert!(runtime.is_ok());

        let small_buf = unsafe { wasm_runtime_malloc(16) };
        assert!(!small_buf.is_null());
        unsafe { wasm_runtime_free(small_buf) };
    }

    #[test]
    fn test_runtime_builder_memory_pool() {
        let runtime_pool = vec![0u8; 256 * 1024].into_boxed_slice();
        let linear_pool = vec![0u8; 64 * 1024].into_boxed_slice();

        let runtime = Runtime::builder()
            .use_memory_pool(runtime_pool, linear_pool)
            .build();

        assert!(runtime.is_ok());

        let runtime = runtime.unwrap();

        // WAMR runtime allocation should work through the custom allocator.
        let ptr = unsafe { wasm_runtime_malloc(16) };
        assert!(!ptr.is_null());

        unsafe {
            wasm_runtime_free(ptr);
        }

        drop(runtime);
    }

    #[test]
    fn test_runtime_builder_memory_pool_repeated_alloc_free() {
        let runtime_pool = vec![0u8; 256 * 1024].into_boxed_slice();
        let linear_pool = vec![0u8; 64 * 1024].into_boxed_slice();

        let runtime = Runtime::builder()
            .use_memory_pool(runtime_pool, linear_pool)
            .build();

        assert!(runtime.is_ok());

        let _runtime = runtime.unwrap();

        // If free/reuse is working correctly, this should not
        // progressively consume the runtime pool.
        for _ in 0..10_000 {
            let ptr = unsafe { wasm_runtime_malloc(1024) };

            assert!(!ptr.is_null(), "runtime allocation failed during iteration");

            unsafe {
                wasm_runtime_free(ptr);
            }
        }
    }

    #[test]
    fn test_runtime_builder_memory_pool_multiple_allocations() {
        let runtime_pool = vec![0u8; 256 * 1024].into_boxed_slice();
        let linear_pool = vec![0u8; 64 * 1024].into_boxed_slice();

        let runtime = Runtime::builder()
            .use_memory_pool(runtime_pool, linear_pool)
            .build();

        assert!(runtime.is_ok());

        let _runtime = runtime.unwrap();

        let sizes = [16, 32, 64, 128, 256, 512, 1024, 4096];

        let mut allocations = Vec::new();

        for size in sizes {
            let ptr = unsafe { wasm_runtime_malloc(size) };

            assert!(!ptr.is_null(), "failed to allocate {} bytes", size);

            // Verify the returned memory is writable.
            unsafe {
                std::ptr::write_bytes(ptr, 0xAB, size as usize);
            }

            allocations.push(ptr);
        }

        for ptr in allocations {
            unsafe {
                wasm_runtime_free(ptr);
            }
        }

        // Verify the allocator is still usable after freeing
        // all previous allocations.
        let ptr = unsafe { wasm_runtime_malloc(4096) };

        assert!(!ptr.is_null());

        unsafe {
            wasm_runtime_free(ptr);
        }
    }

    #[test]
    fn test_runtime_builder_memory_pool_realloc() {
        let runtime_pool = vec![0u8; 256 * 1024].into_boxed_slice();
        let linear_pool = vec![0u8; 64 * 1024].into_boxed_slice();

        let runtime = Runtime::builder()
            .use_memory_pool(runtime_pool, linear_pool)
            .build();

        assert!(runtime.is_ok());

        let _runtime = runtime.unwrap();

        let ptr = unsafe { wasm_runtime_malloc(128) };

        assert!(!ptr.is_null());

        // Put known data into the allocation.
        unsafe {
            for i in 0..128 {
                *(ptr as *mut u8).add(i) = i as u8;
            }
        }

        let ptr = unsafe { wasm_runtime_realloc(ptr, 256) };

        assert!(!ptr.is_null());

        // realloc must preserve the original contents.
        unsafe {
            for i in 0..128 {
                assert_eq!(*(ptr as *const u8).add(i), i as u8);
            }
        }

        unsafe {
            wasm_runtime_free(ptr);
        }
    }

    #[test]
    fn test_runtime_builder_memory_pool_realloc_shrink() {
        let runtime_pool = vec![0u8; 256 * 1024].into_boxed_slice();
        let linear_pool = vec![0u8; 64 * 1024].into_boxed_slice();

        let runtime = Runtime::builder()
            .use_memory_pool(runtime_pool, linear_pool)
            .build();

        assert!(runtime.is_ok());

        let _runtime = runtime.unwrap();

        let ptr = unsafe { wasm_runtime_malloc(4096) };

        assert!(!ptr.is_null());

        unsafe {
            for i in 0..4096 {
                *(ptr as *mut u8).add(i) = (i & 0xff) as u8;
            }
        }

        let ptr = unsafe { wasm_runtime_realloc(ptr, 512) };

        assert!(!ptr.is_null());

        unsafe {
            for i in 0..512 {
                assert_eq!(*(ptr as *const u8).add(i), (i & 0xff) as u8);
            }

            wasm_runtime_free(ptr);
        }
    }

    #[test]
    fn test_runtime_builder_memory_pool_is_reusable_after_runtime_drop() {
        let runtime_pool = vec![0u8; 256 * 1024].into_boxed_slice();
        let linear_pool = vec![0u8; 64 * 1024].into_boxed_slice();

        {
            let runtime = Runtime::builder()
                .use_memory_pool(runtime_pool, linear_pool)
                .build();

            assert!(runtime.is_ok());

            let runtime = runtime.unwrap();

            let ptr = unsafe { wasm_runtime_malloc(4096) };

            assert!(!ptr.is_null());

            unsafe {
                wasm_runtime_free(ptr);
            }

            drop(runtime);
        }
    }
}
