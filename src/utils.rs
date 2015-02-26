//! Utilities
use std::intrinsics;
use std::mem;
use std::num::Int;

use rand::{self, Rng, ThreadRng};
use rand::distributions::range::SampleRange;
use rand::os::OsRng;


#[allow(dead_code)]
pub unsafe fn zero_memory(ptr: *mut u8, size: usize) {
    intrinsics::volatile_set_memory(ptr, 0, size);
}

// Use llvm intrinsic to avoid being optmized-out, I don't know
// how effective it is. Maybe it would be worth calling `memset_s`
// on platforms where it is available.
pub unsafe fn set_memory(ptr: *mut u8, byte: u8, size: usize) {
    intrinsics::volatile_set_memory(ptr, byte, size);
}


// Return `1` iff `x == y`; `0` otherwise.
fn byte_eq(x: u8, y: u8) -> u8 {
    let mut z: u8 = !(x ^ y);
    z &= z >> 4;
    z &= z >> 2;
    z &= z >> 1;
    z
}

// Compare bytes buffers.
//
// Return `true` iff `x == y`; `false` otherwise.
pub fn bytes_eq<T>(x: &[T], y: &[T]) -> bool {
    if x.len() != y.len() {
        return false;
    }

    let size = x.len().checked_mul(mem::size_of::<T>()).unwrap();
    let mut px = x.as_ptr() as *const u8;
    let mut py = y.as_ptr() as *const u8;

    let mut d: u8 = 0;
    unsafe {
        for _ in 0_usize..size {
            d |= *px ^ *py;
            px = px.offset(1);
            py = py.offset(1);
        }
    }

    // Would prefer to return the result of byte_eq() instead of making
    // this last comparison, but this function is called from contexts where
    // boolean values are explicitly expected and this comparison seems
    // the only way to convert to a bool in rust.
    byte_eq(d, 0) == 1
}


// Instantiate a PRNG faster than `os_rng()`.
pub fn rng() -> ThreadRng {
    rand::thread_rng()
}

// Instantiate a PRNG based on `urandom`.
pub fn os_rng() -> OsRng {
    OsRng::new().unwrap()
}

// Same as `Rng::gen_range` but also tolerates when `low == high`.
pub fn gen_range<R: Rng, T: PartialOrd + SampleRange>(rng: &mut R,
                                                      low: T, high: T) -> T {
    if low == high {
        return low;
    }

    rng.gen_range(low, high)
}


#[cfg(test)]
mod tests {
    use std::old_path::BytesContainer;

    use rand::random;


    #[test]
    fn test_byte_eq() {
        for _ in 0_usize..256 {
            let a: u8 = random();
            let b: u8 = random();
            assert_eq!(super::byte_eq(a, b) == 1, a == b);
        }
    }

    #[test]
    fn test_bytes_eq() {
        let a: [u8; 64] = [0u8; 64];
        let b: [u8; 64] = [0u8; 64];
        assert!(super::bytes_eq(&a, &b));

        for _ in 0_usize..256 {
            let va: Vec<u8> = (0_usize..64).map(|_| random::<u8>()).collect();
            let a = va.container_as_bytes();
            let vb: Vec<u8> = (0_usize..64).map(|_| random::<u8>()).collect();
            let b = vb.container_as_bytes();
            assert_eq!(super::bytes_eq(a, b), a == b);
        }
    }
}
