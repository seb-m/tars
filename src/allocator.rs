//! Memory allocators
//!
//! Provide a common interface for memory allocation in protected
//! containers `ProtBuf` and `ProtKey`.
use alloc::heap;
use std::marker::PhantomFn;

use malloc;


/// Base trait for memory allocators
pub trait Allocator : PhantomFn<Self> {
    /// Allocate `size` bytes of memory whose base address is a multiple
    /// of `align`.
    unsafe fn allocate(size: usize, align: usize) -> *mut u8;

    /// Deallocate `size` bytes memory at `ptr`. `size` and `align` must
    /// be the same values used when `allocate` was called.
    unsafe fn deallocate(ptr: *mut u8, size: usize, align: usize);
}

/// Trait for keys allocators
pub trait KeyAllocator : Allocator {
    /// Set memory protection of pages allocated at `ptr` to read-only.
    unsafe fn protect_read(ptr: *mut u8, size: usize);

    /// Set memory protection of pages allocated at `ptr` to write-only.
    /// Note that on most architectures write-only cannot be enforced
    /// and also allows read access.
    unsafe fn protect_write(ptr: *mut u8, size: usize);

    /// Set memory protection of pages allocated at `ptr` to prevent
    /// any access.
    unsafe fn protect_none(ptr: *mut u8, size: usize);
}


/// Default buffer allocator
///
/// Default allocator used to allocate and deallocate memory in protected
/// buffers.
pub type DefaultBufferAllocator = ProtectedBufferAllocator;

/// Default key allocator
///
/// Default allocator used to allocate and deallocate memory in protected
/// buffers.
pub type DefaultKeyAllocator = ProtectedKeyAllocator;


/// Null heap allocator
///
/// Heap allocator using Rust's allocator (usually `jemalloc`). Beware, this
/// allocator **does not implement any protection at all**. Provided only for
/// testing and compatibilty purposes.
#[doc(hidden)]
#[derive(Copy)]
pub struct NullHeapAllocator;

impl Allocator for NullHeapAllocator {
    unsafe fn allocate(size: usize, align: usize) -> *mut u8 {
        heap::allocate(size, align)
    }

    unsafe fn deallocate(ptr: *mut u8, size: usize, align: usize) {
        heap::deallocate(ptr, size, align);
    }
}

impl KeyAllocator for NullHeapAllocator {
    unsafe fn protect_read(_ptr: *mut u8, _size: usize) {
    }

    unsafe fn protect_write(_ptr: *mut u8, _size: usize) {
    }

    unsafe fn protect_none(_ptr: *mut u8, _size: usize) {
    }
}


/// Protected buffer allocator
///
/// Use a custom allocator to provide various memory protections in
/// order to help protect data buffers. Its typical use is as allocator
/// of `ProtBuf` buffers.
#[derive(Copy)]
pub struct ProtectedBufferAllocator;

impl Allocator for ProtectedBufferAllocator {
    unsafe fn allocate(size: usize, align: usize) -> *mut u8 {
        malloc::malloc(size, align)
    }

    unsafe fn deallocate(ptr: *mut u8, _size: usize, _align: usize) {
        malloc::free(ptr);
    }
}


/// Protected key allocator
///
/// Use a custom allocator, similar to `ProtectedBufferAllocator` but allow
/// for more granularity in the control of its memory. Its typical use
/// is as allocator of `ProtKey` keys.
#[derive(Copy)]
pub struct ProtectedKeyAllocator;

impl Allocator for ProtectedKeyAllocator {
    unsafe fn allocate(size: usize, align: usize) -> *mut u8 {
        malloc::malloc_key(size, align)
    }

    unsafe fn deallocate(ptr: *mut u8, _size: usize, _align: usize) {
        malloc::free(ptr);
    }
}

impl KeyAllocator for ProtectedKeyAllocator {
    unsafe fn protect_read(ptr: *mut u8, _size: usize) {
        malloc::protect_read(ptr);
    }

    unsafe fn protect_write(ptr: *mut u8, _size: usize) {
        malloc::protect_write(ptr);
    }

    unsafe fn protect_none(ptr: *mut u8, _size: usize) {
        malloc::protect_none(ptr);
    }
}
