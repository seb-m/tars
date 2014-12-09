//! Protected key
//!
use std::cell::{mod, Cell, Ref, RefCell, RefMut};
use std::fmt;
use std::num::Int;
use std::rc::Rc;

use allocator::{Allocator, KeyAllocator};
use buf::ProtBuf;


/// Key of bytes
pub type ProtKey8<A> = ProtKey<A, u8>;

const NOREAD: uint = 0;


/// A protected key
///
/// Transform a `ProtBuf` instance into a protected key `ProtKey` and provide
/// tigher access control on its memory.
///
/// By default a `ProtKey` cannot be read nor written to and will only
/// provide separated accesses with limited scopes. Thus, RAII accessor
/// methods must be used to read and write to a `ProtKey`. Accessing the
/// underlying key is a bit similar to the way of manipulating an object
/// wrapped in `RefCell`.
///
/// ```rust
/// # extern crate tars;
/// # use tars::allocator::ProtectedKeyAllocator;
/// # use tars::ProtBuf;
/// # use tars::ProtKey;
/// # fn encrypt(_: &[u8], _: &[u8]) {}
/// # fn main() {
/// // Instanciate a new buffer initialized with random bytes.
/// // Same as an usual ProtBuf instance but with a different allocator.
/// let buf_rnd = ProtBuf::<ProtectedKeyAllocator, u8>::new_rand_os(32);
///
/// // Until here memory buffer is read/write. Turns-it into a key:
/// let key = ProtKey::new(buf_rnd);
///
/// {
///     // Request access in read-mode
///     let key_read = key.read();
///     let byte = key_read[16];
///     // ...
/// }   // Relinquish its read-access
///
/// // Alternative way to read its content
/// key.read_with(|k| encrypt(k.as_slice(), b"abc"));
///
/// // Access it in write-mode
/// if let Some(mut key_write) = key.try_write() {
///     key_write[16] = 42;
/// }
/// # }
/// ```
pub struct ProtKey<A, T> {
    key: RefCell<ProtBuf<A, T>>,
    read_ctr: Rc<Cell<uint>>
}

impl<A: KeyAllocator, T: Copy> ProtKey<A, T> {
    /// Take ownership of `prot_buf` and transform it into a `ProtKey`. By
    /// default prevent any access.
    pub fn new(prot_buf: ProtBuf<A, T>) -> ProtKey<A, T> {
        unsafe {
            KeyAllocator::protect_none(None::<A>, prot_buf.as_ptr() as *mut u8,
                                       prot_buf.len_bytes());
        }

        ProtKey {
            key: RefCell::new(prot_buf),
            read_ctr: Rc::new(Cell::new(NOREAD))
        }
    }

    /// Consume and copy `prot_buf` to force using `ProtKey`'s allocator.
    /// If `prot_buf` already uses a `KeyAllocator` there is no need to make
    /// a copy so directly call the default cstor `new` instead.
    pub fn from_buf<B: Allocator>(prot_buf: ProtBuf<B, T>) -> ProtKey<A, T> {
        let buf = ProtBuf::from_slice(prot_buf.as_slice());
        ProtKey::new(buf)
    }

    /// Return a wrapper to the key in read mode. This method `panic!` if
    /// this key is already accessed in write mode.
    // FIXME: Not sure if it's the best interface to provide a `try_read`
    // variant to this `fail`ing method. It would maybe be better to
    // implement a single method returning a `Result`. See this RFC
    // https://github.com/rust-lang/rfcs/blob/master/text/0236-error-conventions.md
    pub fn read(&self) -> ProtKeyRead<A, T> {
        ProtKeyRead::new(self.key.borrow(), self.read_ctr.clone())
    }

    /// Return a wrapper to the key in read mode. Return `None`
    /// if the key is already accessed in write mode.
    pub fn try_read(&self) -> Option<ProtKeyRead<A, T>> {
        match self.key.try_borrow() {
            Some(borrowed_key) => Some(ProtKeyRead::new(borrowed_key,
                                                        self.read_ctr.clone())),
            None => None
        }
    }

    /// Access the key in read mode and pass a reference to closure `f`.
    /// The key can only be read during this call. This method will `panic!`
    /// if a read access cannot be acquired on this key.
    pub fn read_with(&self, f: |&ProtKeyRead<A, T>|) {
        f(&self.read())
    }

    /// Return a wrapper to the key in write mode. This method `panic!` if
    /// the key is already currently accessed in read or write mode.
    pub fn write(&self) -> ProtKeyWrite<A, T> {
        let key_write = ProtKeyWrite::new(self.key.borrow_mut());
        assert_eq!(self.read_ctr.get(), NOREAD);
        key_write
    }

    /// Return a wrapper to the key in write mode. Return `None`
    /// if the key is already accessed in read or write mode.
    pub fn try_write(&self) -> Option<ProtKeyWrite<A, T>> {
        let key_write = match self.key.try_borrow_mut() {
            Some(borrowed_key) => Some(ProtKeyWrite::new(borrowed_key)),
            None => None
        };

        if key_write.is_some() {
            assert_eq!(self.read_ctr.get(), NOREAD);
        }

        key_write
    }

    /// Access the key in write mode and pass a reference to closure `f`.
    /// The key can only be writtent during this call. This method will
    /// `panic!` if a write access cannot be acquired on this key.
    pub fn write_with(&self, f: |&mut ProtKeyWrite<A, T>|) {
        f(&mut self.write())
    }
}

#[unsafe_destructor]
impl<A: KeyAllocator, T: Copy> Drop for ProtKey<A, T> {
    fn drop(&mut self) {
        // FIXME: without this assert this drop is useless.
        assert_eq!(self.read_ctr.get(), NOREAD);
    }
}

impl<A: KeyAllocator, T: Copy> Clone for ProtKey<A, T> {
    fn clone(&self) -> ProtKey<A, T> {
        ProtKey::new(self.read().clone())
    }
}

impl<A: KeyAllocator, T: Copy> PartialEq for ProtKey<A, T> {
    fn eq(&self, other: &ProtKey<A, T>) -> bool {
        match (self.try_read(), other.try_read()) {
            (Some(ref s), Some(ref o)) => *s == *o,
            (_, _) => false
        }
    }
}

impl<A: KeyAllocator, T: fmt::Show + Copy> fmt::Show for ProtKey<A, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.try_read() {
            Some(r) => r.fmt(f),
            None => Err(fmt::Error)
        }
    }
}


/// An RAII protected key with read access
///
/// This instance is the result of a `read` request on a `ProtKey`. If no
/// other similar instance on the same `ProtKey` exists, raw memory access
/// will be revoked when this instance is destructed.
pub struct ProtKeyRead<'a, A, T: 'a> {
    ref_key: Ref<'a, ProtBuf<A, T>>,
    read_ctr: Rc<Cell<uint>>
}

impl<'a, A: KeyAllocator, T: Copy> ProtKeyRead<'a, A, T> {
    fn new(ref_key: Ref<'a, ProtBuf<A, T>>,
           read_ctr: Rc<Cell<uint>>) -> ProtKeyRead<'a, A, T> {
        if read_ctr.get() == NOREAD {
            unsafe {
                KeyAllocator::protect_read(None::<A>,
                                           ref_key.as_ptr() as *mut u8,
                                           ref_key.len_bytes());
            }
        }
        read_ctr.set(read_ctr.get().checked_add(1).unwrap());
        ProtKeyRead {
            ref_key: ref_key,
            read_ctr: read_ctr
        }
    }

    /// Clone this instance.
    // FIXME: Currently does not implement `clone()` as it would interfere
    //        with `ProtKey::clone()`. (see comment in `cell::clone_ref()`).
    pub fn clone_it(&self) -> ProtKeyRead<A, T> {
        ProtKeyRead::new(cell::clone_ref(&self.ref_key), self.read_ctr.clone())
    }
}

#[unsafe_destructor]
impl<'a, A: KeyAllocator, T: Copy> Drop for ProtKeyRead<'a, A, T> {
    fn drop(&mut self) {
        self.read_ctr.set(self.read_ctr.get().checked_sub(1).unwrap());
        if self.read_ctr.get() == NOREAD {
            unsafe {
                KeyAllocator::protect_none(None::<A>,
                                           self.ref_key.as_ptr() as *mut u8,
                                           self.ref_key.len_bytes());
            }
        }
    }
}

impl<'a, A: KeyAllocator, T: Copy> Deref<ProtBuf<A, T>> for ProtKeyRead<'a, A, T> {
    fn deref(&self) -> &ProtBuf<A, T> {
        &*self.ref_key
    }
}

impl<'a, A: KeyAllocator, T: Copy> AsSlice<T> for ProtKeyRead<'a, A, T> {
    fn as_slice(&self) -> &[T] {
        (**self).as_slice()
    }
}

impl<'a, A: KeyAllocator, T: Copy> PartialEq for ProtKeyRead<'a, A, T> {
    fn eq(&self, other: &ProtKeyRead<A, T>) -> bool {
        **self == **other
    }
}

impl<'a, A: KeyAllocator, T: fmt::Show + Copy> fmt::Show for ProtKeyRead<'a, A, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.ref_key.fmt(f)
    }
}


/// An RAII protected key with write access
///
/// This instance is the result of a `write` request on a `ProtKey`. Its
/// raw memory may only be written during the lifetime of this object.
pub struct ProtKeyWrite<'a, A, T: 'a> {
    ref_key: RefMut<'a, ProtBuf<A, T>>,
}

impl<'a, A: KeyAllocator, T: Copy> ProtKeyWrite<'a, A, T> {
    fn new(ref_key: RefMut<'a, ProtBuf<A, T>>) -> ProtKeyWrite<'a, A, T> {
        unsafe {
            KeyAllocator::protect_write(None::<A>,
                                        ref_key.as_ptr() as *mut u8,
                                        ref_key.len_bytes());
        }
        ProtKeyWrite {
            ref_key: ref_key,
        }
    }
}

#[unsafe_destructor]
impl<'a, A: KeyAllocator, T: Copy> Drop for ProtKeyWrite<'a, A, T> {
    fn drop(&mut self) {
        unsafe {
            KeyAllocator::protect_none(None::<A>,
                                       self.ref_key.as_ptr() as *mut u8,
                                       self.ref_key.len_bytes());
        }
    }
}

/// This method is mandatory,  but it should not be used for reading the
/// content of the underlying key...
#[allow(unreachable_code)]
impl<'a, A: KeyAllocator, T: Copy> Deref<ProtBuf<A, T>> for ProtKeyWrite<'a, A, T> {
    fn deref(&self) -> &ProtBuf<A, T> {
        panic!("key must only be written");
        &*self.ref_key
    }
}

impl<'a, A: KeyAllocator,
      T: Copy> DerefMut<ProtBuf<A, T>> for ProtKeyWrite<'a, A, T> {
    fn deref_mut(&mut self) -> &mut ProtBuf<A, T> {
        &mut *self.ref_key
    }
}


#[cfg(test)]
mod test {
    use allocator::ProtectedKeyAllocator;
    use buf::ProtBuf;
    use key::ProtKey;


    #[test]
    fn test_read() {
        let s1 = ProtBuf::<ProtectedKeyAllocator, u8>::new_rand_os(256);
        let s2 = s1.clone();

        let key = ProtKey::new(s1);

        assert!(key.read().as_slice() == s2.as_slice());
        assert!(key.read()[] == s2[]);
        assert!(*key.read() == s2);

        {
            let r1 = key.read();
            let r2 = key.try_read().unwrap();
            assert!(r1 == r2);

            assert!(key.try_write().is_none());

            let r3 = r1.clone_it();
            assert!(r3 == r2);
        }

        key.read_with(|k| assert!(k[] == s2.as_slice()));

        assert!(key.try_write().is_some());
    }

    #[test]
    fn test_write() {
        let zero = ProtBuf::<ProtectedKeyAllocator, u8>::new_zero(256);
        let key =
            ProtBuf::<ProtectedKeyAllocator, u8>::new_rand_os(256).into_key();

        for i in key.write().as_mut_slice().iter_mut() {
            *i = 0;
        }
        assert!(*key.read() == zero);

        {
            let _w = key.write();
            assert!(key.try_write().is_none());
            assert!(key.try_read().is_none());
        }

        key.write_with(|k| k[42] = 42);

        assert!(key.try_write().is_some());
        assert!(key.try_read().is_some());
    }
}
