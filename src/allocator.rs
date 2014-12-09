//! Memory allocators
//!
//! Provide a common interface for memory allocation in protected
//! containers `ProtBuf` and `ProtKey`.
use alloc::heap;

use malloc;


/// Base trait for memory allocators
pub trait Allocator {
    // FIXME: (#16293) waiting UFCS to remove this unused first argument.
    /// Allocate `size` bytes of memory whose base address is a multiple
    /// of `align`.
    unsafe fn allocate(_: Option<Self>, size: uint, align: uint) -> *mut u8;

    /// Deallocate `size` bytes memory at `ptr`. `size` and `align` must
    /// be the same values used when `allocate` was called.
    unsafe fn deallocate(_: Option<Self>, ptr: *mut u8, size: uint,
                         align: uint);
}

/// Trait for keys allocators
pub trait KeyAllocator : Allocator {
    /// Set memory protection of pages allocated at `ptr` to read-only.
    unsafe fn protect_read(_: Option<Self>, ptr: *mut u8, size: uint);

    /// Set memory protection of pages allocated at `ptr` to write-only.
    /// Note that on most architectures write-only cannot be enforced
    /// and also allows read access.
    unsafe fn protect_write(_: Option<Self>, ptr: *mut u8, size: uint);

    /// Set memory protection of pages allocated at `ptr` to prevent
    /// any access.
    unsafe fn protect_none(_: Option<Self>, ptr: *mut u8, size: uint);
}


/// Default buffer allocator
///
/// Default allocator used to allocate and deallocate memory in protected
/// buffers.
pub type BufAlloc = ProtectedBufferAllocator;

/// Default key allocator
///
/// Default allocator used to allocate and deallocate memory in protected
/// buffers.
pub type KeyAlloc = ProtectedKeyAllocator;


/// Null heap allocator
///
/// Heap allocator using Rust's allocator (usually `jemalloc`). Beware, this
/// allocator **does not implement any protection at all**. Provided only for
/// testing and compatibilty purposes.
#[doc(hidden)]
pub struct NullHeapAllocator;

impl Allocator for NullHeapAllocator {
    unsafe fn allocate(_: Option<NullHeapAllocator>, size: uint,
                       align: uint) -> *mut u8 {
        heap::allocate(size, align)
    }

    unsafe fn deallocate(_: Option<NullHeapAllocator>, ptr: *mut u8,
                         size: uint, align: uint) {
        heap::deallocate(ptr, size, align);
    }
}

impl KeyAllocator for NullHeapAllocator {
    unsafe fn protect_read(_: Option<NullHeapAllocator>, _ptr: *mut u8,
                           _size: uint) {
    }

    unsafe fn protect_write(_: Option<NullHeapAllocator>, _ptr: *mut u8,
                            _size: uint) {
    }

    unsafe fn protect_none(_: Option<NullHeapAllocator>, _ptr: *mut u8,
                           _size: uint) {
    }
}


/// Protected buffer allocator
///
/// Use a custom allocator to provide various memory protections in
/// order to help protect data buffers. Its typical use is as allocator
/// of `ProtBuf` buffers.
pub struct ProtectedBufferAllocator;

impl Allocator for ProtectedBufferAllocator {
    unsafe fn allocate(_: Option<ProtectedBufferAllocator>, size: uint,
                       align: uint) -> *mut u8 {
        malloc::malloc(size, align)
    }

    unsafe fn deallocate(_: Option<ProtectedBufferAllocator>, ptr: *mut u8,
                         _size: uint, _align: uint) {
        malloc::free(ptr);
    }
}


/// Protected key allocator
///
/// Use a custom allocator, similar to `ProtectedBufferAllocator` but allow
/// for more granularity in the control of its memory. Its typical use
/// is as allocator of `ProtKey` keys.
pub struct ProtectedKeyAllocator;

impl Allocator for ProtectedKeyAllocator {
    unsafe fn allocate(_: Option<ProtectedKeyAllocator>, size: uint,
                       align: uint) -> *mut u8 {
        malloc::malloc_key(size, align)
    }

    unsafe fn deallocate(_: Option<ProtectedKeyAllocator>, ptr: *mut u8,
                         _size: uint, _align: uint) {
        malloc::free(ptr);
    }
}

impl KeyAllocator for ProtectedKeyAllocator {
    unsafe fn protect_read(_: Option<ProtectedKeyAllocator>, ptr: *mut u8,
                           _size: uint) {
        malloc::protect_read(ptr);
    }

    unsafe fn protect_write(_: Option<ProtectedKeyAllocator>,
                            ptr: *mut u8, _size: uint) {
        malloc::protect_write(ptr);
    }

    unsafe fn protect_none(_: Option<ProtectedKeyAllocator>, ptr: *mut u8,
                           _size: uint) {
        malloc::protect_none(ptr);
    }
}
