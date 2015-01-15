//! Protected buffer
//!
use alloc::heap;
use std::fmt;
use std::intrinsics;
use std::iter::AdditiveIterator;
use std::marker;
use std::mem;
use std::num::{Int, FromPrimitive};
use std::ops::{Deref, DerefMut, Index, IndexMut,
               Range, RangeTo, RangeFrom, FullRange};
use std::ptr;
use std::rand::Rng;
use std::raw::Slice;

use allocator::{Allocator, KeyAllocator, DefaultBufferAllocator};
use key::ProtKey;
use utils;


/// Buffer of bytes
pub type ProtBuf8<A = DefaultBufferAllocator> = ProtBuf<u8, A>;


unsafe fn alloc<A: Allocator, T>(count: usize) -> *mut T {
    let size = count.checked_mul(mem::size_of::<T>()).unwrap();

    // allocate
    let ptr = <A as Allocator>::allocate(size,
                                         mem::min_align_of::<T>()) as *mut T;
    assert!(!ptr.is_null());
    ptr
}

unsafe fn dealloc<A: Allocator, T>(ptr: *mut T, count: usize) {
    let size = count.checked_mul(mem::size_of::<T>()).unwrap();

    // deallocate
    <A as Allocator>::deallocate(ptr as *mut u8, size,
                                 mem::min_align_of::<T>());
}


/// A protected Buffer
///
/// Fixed-length buffer used to handle sensible data. Try to minimize
/// data copy and zero-out memory on deallocation.
///
/// Such a protected buffer must be associated with a protected memory
/// allocator responsible for allocating and deallocating its memory.
///
/// A `ProtBuf` instance always provides a read/write access to its memory
/// elements. See `ProtKey` for more controlled access.
///
/// ```rust
/// # extern crate tars;
/// # use tars::allocator::{ProtectedBufferAllocator, ProtectedKeyAllocator};
/// # use tars::{ProtKey, ProtBuf, ProtBuf8};
/// # fn my_function(_: &mut [u8]) {}
/// # fn main() {
/// // Create a new buffer of bytes
/// let mut buf: ProtBuf<u8, ProtectedBufferAllocator> = ProtBuf::new_zero(42);
///
/// // Or more simply, like this with exactly the same result
/// let mut buf: ProtBuf8 = ProtBuf::new_zero(42);
///
/// assert_eq!(buf[21], 0);
///
/// // Use a slice to access and manipulate the underlying memory buffer
/// my_function(buf.as_mut_slice());
///
/// // Create a new random key
/// let key: ProtKey<u8, ProtectedKeyAllocator> =
///      ProtBuf::new_rand_os(42).into_key();
/// # }
/// ```
pub struct ProtBuf<T, A = DefaultBufferAllocator> {
    len: usize,
    ptr: *mut T,
    nosync: marker::NoSync
}

// fixme
//impl<T, A> !Sync for ProtBuf<T, A> {
//}

impl<T: Copy, A: Allocator> ProtBuf<T, A> {
    fn from_raw_parts(length: usize, ptr: *mut T) -> ProtBuf<T, A> {
        ProtBuf {
            len: length,
            ptr: ptr,
            nosync: marker::NoSync
        }
    }

    /// Return the length i.e. the number of elements `T` of this buffer.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Return the size in bytes represented by the elements of this buffer.
    #[doc(hidden)]
    pub fn len_bytes(&self) -> usize {
        self.len.checked_mul(mem::size_of::<T>()).unwrap()
    }

    fn with_length(length: usize) -> ProtBuf<T, A> {
        if mem::size_of::<T>() == 0 || length == 0 {
            return ProtBuf::from_raw_parts(0, heap::EMPTY as *mut T);
        }

        let ptr = unsafe {
            alloc::<A, T>(length)
        };
        ProtBuf::from_raw_parts(length, ptr)
    }

    /// New allocated buffer with unitilialized memory.
    pub fn new(length: usize) -> ProtBuf<T, A> {
        ProtBuf::with_length(length)
    }

    /// New allocated buffer with its memory initialized with bytes of
    /// value zero.
    pub fn new_zero(length: usize) -> ProtBuf<T, A> {
        let n = ProtBuf::with_length(length);
        unsafe {
            ptr::zero_memory(n.ptr, length);
        }
        n
    }

    /// Allocate a new buffer of size `length` and fill it with randomly
    /// generated bytes. Use `rng` as random number generator.
    pub fn new_rand<R: Rng>(length: usize, rng: &mut R) -> ProtBuf<T, A> {
        let mut n = ProtBuf::with_length(length);
        rng.fill_bytes(unsafe {
            mem::transmute(Slice {
                data: n.as_mut_ptr() as *const u8,
                len: n.len_bytes()
            })
        });
        n
    }

    /// Allocate a new buffer of size `length` and fill it with randomly
    /// generated bytes. Use an instance of `OsRng` as random number
    /// generator.
    pub fn new_rand_os(length: usize) -> ProtBuf<T, A> {
        ProtBuf::new_rand(length, &mut utils::os_rng())
    }

    /// New buffer with elements copied from slice `values`.
    pub fn from_slice(values: &[T]) -> ProtBuf<T, A> {
        unsafe {
            ProtBuf::from_raw_buf(values.as_ptr(), values.len())
        }
    }

    /// New buffer from unsafe buffer.
    pub unsafe fn from_raw_buf(buf: *const T, length: usize) -> ProtBuf<T, A> {
        assert!(!buf.is_null());
        let n = ProtBuf::with_length(length);
        ptr::copy_nonoverlapping_memory(n.ptr, buf, n.len);
        n
    }

    /// Build a new instance by concatenating slice `items` together.
    pub fn from_slices(items: &[&[T]]) -> ProtBuf<T, A> {
        let length = items.iter().map(|x| x.len()).sum();
        let n = ProtBuf::with_length(length);
        let mut idx: isize = 0;

        unsafe {
            for it in items.iter() {
                ptr::copy_nonoverlapping_memory(n.ptr.offset(idx),
                                                it.as_ptr(), it.len());
                idx += it.len() as isize;
            }
        }
        n
    }

    /// Build a new instance by concatenating `ProtBuf` `items` together.
    pub fn from_bufs(items: &[&ProtBuf<T, A>]) -> ProtBuf<T, A> {
        let v: Vec<&[T]> = items.iter().map(|x| (*x).as_slice()).collect();
        ProtBuf::from_slices(v.as_slice())
    }

    /// Return a mutable slice into `self`.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe {
            mem::transmute(Slice {
                data: self.ptr as *const T,
                len: self.len
            })
        }
    }

    /// Cast `self` to another compatible type and return a slice on it.
    pub fn cast_to_slice<U>(&self) -> &[U] {
        let bytes_size = self.len_bytes();
        let dst_type_size = mem::size_of::<U>();
        assert_eq!(mem::min_align_of::<T>(), mem::min_align_of::<U>());
        assert!(bytes_size > 0 && dst_type_size > 0 &&
                bytes_size >= dst_type_size && bytes_size % dst_type_size == 0);
        unsafe {
            mem::transmute(Slice {
                data: self.ptr as *const U,
                len: bytes_size.checked_div(dst_type_size).unwrap()
            })
        }
    }

    /// Cast `self` to another compatible type and return a mut slice on it.
    pub fn cast_to_slice_mut<U>(&mut self) -> &mut [U] {
        let bytes_size = self.len_bytes();
        let dst_type_size = mem::size_of::<U>();
        assert_eq!(mem::min_align_of::<T>(), mem::min_align_of::<U>());
        assert!(bytes_size > 0 && dst_type_size > 0 &&
                bytes_size >= dst_type_size && bytes_size % dst_type_size == 0);
        unsafe {
            mem::transmute(Slice {
                data: self.ptr as *const U,
                len: bytes_size.checked_div(dst_type_size).unwrap()
            })
        }
    }
}

impl<T: Copy, A: Allocator> AsSlice<T> for ProtBuf<T, A> {
    /// Return a slice into `self`.
    fn as_slice(&self) -> &[T] {
        unsafe {
            mem::transmute(Slice {
                data: self.ptr,
                len: self.len
            })
        }
    }
}

impl<T: Copy, A: KeyAllocator> ProtBuf<T, A> {
    /// Transform `self` into a protected key `ProtKey`.
    pub fn into_key(self) -> ProtKey<T, A> {
        ProtKey::new(self)
    }
}

impl<T: FromPrimitive + Copy, A: Allocator> ProtBuf<T, A> {
    /// New buffer from bytes.
    pub fn from_bytes(bytes: &[u8]) -> ProtBuf<T, A> {
        let len = bytes.len();
        let mut n: ProtBuf<T, A> = ProtBuf::with_length(len);

        for i in range(0us, len) {
            n[i] = FromPrimitive::from_u8(bytes[i]).unwrap();
        }
        n
    }
}

#[unsafe_destructor]
impl<T: Copy, A: Allocator> Drop for ProtBuf<T, A> {
    fn drop(&mut self) {
        if self.len != 0 && !self.ptr.is_null() && mem::size_of::<T>() != 0 {
            unsafe {
                // There is no explicit drop on each T elements, as T
                // is contrained to Copy it should not be an issue as they
                // do not implement destructors. However there would be an
                // issue to use ptr::read() as it could copy memory slots
                // to temporary objects.
                assert!(!intrinsics::needs_drop::<T>());
                dealloc::<A, T>(self.ptr, self.len)
            }
        }
    }
}

impl<T: Copy, A: Allocator> Clone for ProtBuf<T, A> {
    fn clone(&self) -> ProtBuf<T, A> {
        ProtBuf::from_slice(self.as_slice())
    }
}

impl<T: Copy, A: Allocator> Index<usize> for ProtBuf<T, A> {
    type Output = T;

    fn index(&self, index: &usize) -> &T {
        &self.as_slice()[*index]
    }
}

impl<T: Copy, A: Allocator> IndexMut<usize> for ProtBuf<T, A> {
    type Output = T;

    fn index_mut(&mut self, index: &usize) -> &mut T {
        &mut self.as_mut_slice()[*index]
    }
}

impl<T: Copy, A: Allocator> Index<Range<usize>> for ProtBuf<T, A> {
    type Output = [T];

    fn index(&self, index: &Range<usize>) -> &[T] {
        self.as_slice().index(index)
    }
}

impl<T: Copy, A: Allocator> Index<RangeTo<usize>> for ProtBuf<T, A> {
    type Output = [T];

    fn index(&self, index: &RangeTo<usize>) -> &[T] {
        self.as_slice().index(index)
    }
}

impl<T: Copy, A: Allocator> Index<RangeFrom<usize>> for ProtBuf<T, A> {
    type Output = [T];

    fn index(&self, index: &RangeFrom<usize>) -> &[T] {
        self.as_slice().index(index)
    }
}

impl<T: Copy, A: Allocator> Index<FullRange> for ProtBuf<T, A> {
    type Output = [T];

    fn index(&self, _index: &FullRange) -> &[T] {
        self.as_slice()
    }
}

impl<T: Copy, A: Allocator> IndexMut<Range<usize>> for ProtBuf<T, A> {
    type Output = [T];

    fn index_mut(&mut self, index: &Range<usize>) -> &mut [T] {
        self.as_mut_slice().index_mut(index)
    }
}

impl<T: Copy, A: Allocator> IndexMut<RangeTo<usize>> for ProtBuf<T, A> {
    type Output = [T];

    fn index_mut(&mut self, index: &RangeTo<usize>) -> &mut [T] {
        self.as_mut_slice().index_mut(index)
    }
}

impl<T: Copy, A: Allocator> IndexMut<RangeFrom<usize>> for ProtBuf<T, A> {
    type Output = [T];

    fn index_mut(&mut self, index: &RangeFrom<usize>) -> &mut [T] {
        self.as_mut_slice().index_mut(index)
    }
}

impl<T: Copy, A: Allocator> IndexMut<FullRange> for ProtBuf<T, A> {
    type Output = [T];

    fn index_mut(&mut self, _index: &FullRange) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: Copy, A: Allocator> Deref for ProtBuf<T, A> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T: Copy, A: Allocator> DerefMut for ProtBuf<T, A> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: Copy, A: Allocator> PartialEq for ProtBuf<T, A> {
    fn eq(&self, other: &ProtBuf<T, A>) -> bool {
        utils::bytes_eq(self.as_slice(), other.as_slice())
    }
}

impl<T: Copy, A: Allocator> Eq for ProtBuf<T, A> {
}

impl<T: fmt::Show + Copy, A: Allocator> fmt::Show for ProtBuf<T, A> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}

macro_rules! hex_fmt {
    ($T:ty, $U:ty) => {
        impl<A: Allocator> $T for ProtBuf<$U, A> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                for i in self.as_slice().iter() {
                    try!(i.fmt(f));
                }
                Ok(())
            }
        }
    }
}

hex_fmt!(fmt::UpperHex, usize);
hex_fmt!(fmt::UpperHex, u8);
hex_fmt!(fmt::UpperHex, u16);
hex_fmt!(fmt::UpperHex, u32);
hex_fmt!(fmt::UpperHex, u64);
hex_fmt!(fmt::LowerHex, usize);
hex_fmt!(fmt::LowerHex, u8);
hex_fmt!(fmt::LowerHex, u16);
hex_fmt!(fmt::LowerHex, u32);
hex_fmt!(fmt::LowerHex, u64);


#[cfg(test)]
mod test {
    use allocator::{NullHeapAllocator, ProtectedBufferAllocator};
    use buf::{ProtBuf, ProtBuf8};


    #[test]
    fn test_basic_dummy() {
        let mut r: [i64; 256] = [0; 256];
        let mut s: [u8; 256] = [0; 256];

        let a: ProtBuf<i64, NullHeapAllocator> = ProtBuf::new_zero(256);
        assert_eq!(a.as_slice(), r.as_slice());

        for i in range(0us, 256) {
            r[i] = i as i64;
            s[i] = i as u8;
        }

        let b: ProtBuf<i64, NullHeapAllocator> =
            ProtBuf::from_bytes(s.as_slice());
        assert_eq!(b.as_slice(), r.as_slice());

        let c: ProtBuf<i64, NullHeapAllocator> =
            ProtBuf::from_slice(r.as_slice());
        assert_eq!(c.as_slice(), r.as_slice());

        let d: ProtBuf<i64, NullHeapAllocator> = unsafe {
            ProtBuf::from_raw_buf(c.as_ptr(), c.len())
        };
        assert_eq!(d.as_slice(), c.as_slice());

        let e: ProtBuf<i64, NullHeapAllocator> =
            ProtBuf::from_slice(r.as_slice());
        assert_eq!(d, e);
    }

    #[test]
    fn test_basic_guarded() {
        let mut r: [i64; 256] = [0; 256];
        let mut s: [u8; 256] = [0; 256];

        let a: ProtBuf<i64, ProtectedBufferAllocator> = ProtBuf::new_zero(256);
        assert_eq!(a.as_slice(), r.as_slice());

        for i in range(0us, 256) {
            r[i] = i as i64;
            s[i] = i as u8;
        }

        let b: ProtBuf<i64, ProtectedBufferAllocator> =
            ProtBuf::from_bytes(s.as_slice());
        assert_eq!(b.as_slice(), r.as_slice());

        let c: ProtBuf<i64, NullHeapAllocator> =
            ProtBuf::from_slice(r.as_slice());
        assert_eq!(c.as_slice(), r.as_slice());

        let d: ProtBuf<i64, ProtectedBufferAllocator> = unsafe {
            ProtBuf::from_raw_buf(c.as_ptr(), c.len())
        };
        assert_eq!(d.as_slice(), c.as_slice());

        let e: ProtBuf<i64, ProtectedBufferAllocator> =
            ProtBuf::from_slice(r.as_slice());
        assert_eq!(d, e);
    }

    #[test]
    fn test_default_params() {
        let _: ProtBuf8 = ProtBuf::new_zero(42);
        let _: ProtBuf<u8> = ProtBuf::new_zero(42);
    }
}
