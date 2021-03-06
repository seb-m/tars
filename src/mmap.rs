//! mmap wrapper
use libc::consts::os::posix88::{MAP_ANON, MAP_PRIVATE, MAP_FAILED,
                                PROT_READ, PROT_WRITE, PROT_NONE};
use libc::funcs::posix88::mman;
use libc::types::common::c95::c_void;
use libc::types::os::arch::c95::{c_int, size_t};
use std::cmp;
use std::io;
use std::ptr;
use std::sync::{Once, ONCE_INIT};

use num::ToPrimitive;

use utils;


/// Used to prevent `mlock` calls on allocated regions. This may be
/// needed on systems with heavy restrictions on `RLIMIT_MEMLOCK`
/// (see `man mprotect` for more details).
///
/// Set `false` to this constant or pass `--cfg feature="no_mlock"` to
/// `rustc` or use `cargo build --features no_mlock` with cargo to disable
/// `mlock` calls (it might be needed in virtual environments).
#[cfg(feature = "no_mlock")]
const USE_MLOCK: bool = false;
#[cfg(not(feature = "no_mlock"))]
const USE_MLOCK: bool = true;

pub const MIN_ALIGN: usize = 16;


pub fn page_size() -> usize {
    static ONCE: Once = ONCE_INIT;
    static mut pagesize: usize = 0;

    unsafe {
        ONCE.call_once(|| {
            pagesize = utils::page_size();
        });

        pagesize
    }
}

#[inline]
pub fn page_mask() -> usize {
    page_size() - 1
}

#[inline]
pub fn mask_pointer<T>(ptr: *mut T) -> *mut T {
    (ptr as usize & !page_mask()) as *mut T
}

#[inline]
fn page_round(size: usize) -> usize {
    size.checked_add(page_mask()).unwrap() & !page_mask()
}


/// Hint at how the buffer should be positionned in the allocated
/// region.
#[derive(Copy, Clone)]
pub enum RangePos {
    Start,
    End,
    Rand
}


/// Memory protection flags. `None` means no `Read` and no `Write`
/// allowed.
#[derive(Copy, Clone)]
pub enum Prot {
    None,
    Read,
    Write,
    ReadWrite
}

impl Prot {
    fn to_mprot(prot: Prot) -> c_int {
        match prot {
            Prot::None => PROT_NONE,
            Prot::Read => PROT_READ,
            // Note that due to hardware limitations concerning page
            // protection on most architectures (such as x86, x86_64),
            // PROT_WRITE implies PROT_READ.
            Prot::Write => PROT_WRITE,
            Prot::ReadWrite => PROT_READ | PROT_WRITE
        }
    }
}


/// Allocate memory
///
/// `size` is the size of memory to be allocated, it is rounded to the next
/// page size multiple. `align` is equal to 0 if no alignment hint is provided,
/// otherwise it must be a power of two smaller than the current page size.
/// `fill` indicates if allocated pages must be filled with a specified byte
/// value. `prot` set the initial pages protections. `pos` hints how the
/// buffer should be positionned inside the allocated region. This function
/// `panic!` on error, only valid non-null pointers are returned.
pub unsafe fn allocate(size: usize, align: usize, fill: Option<u8>,
                       prot: Prot, pos: RangePos) -> *mut u8 {
    let region_sz = page_round(size);
    let full_sz = region_sz.checked_add((2 * page_size())).unwrap();

     // Check align is compatible.
    if align > 0 {
        assert!(align < page_size() && align.is_power_of_two());
    }

    let align_sz = match (align, pos) {
        (0, RangePos::Rand) => MIN_ALIGN,
        (0, RangePos::End) => MIN_ALIGN,
        (_, RangePos::Start) => 1, // Aligned on page's size
        (_, _) => cmp::max(align, MIN_ALIGN)
    };

    let null_addr: *const u8 = ptr::null();
    // On FreeBSD if prot is PROT_WRITE any immmediate read attempt will
    // lead to a segfault. This is not bad because it is not expected
    // to make a read on a write protection but it is counter to the
    // practical behavior where PROT_WRITE usually implies PROT_READ.
    let object = mman::mmap(null_addr as *mut c_void,
                            full_sz as size_t,
                            Prot::to_mprot(prot),
                            MAP_ANON | MAP_PRIVATE |
                            map_imp::additional_map_flags(),
                            -1,
                            0);
    if object == MAP_FAILED {
        panic!("mmap failed: {}", io::Error::last_os_error());
    }

    // Use first and last pages as guarded pages.
    let mut rv = mman::mprotect(object, page_size() as size_t, PROT_NONE);
    if rv != 0 {
        panic!("mprotect failed: {}", io::Error::last_os_error());
    }

    let start = object as *mut u8;

    let lp_offset = full_sz.to_isize().unwrap().checked_sub(
        page_size().to_isize().unwrap()).unwrap();
    rv = mman::mprotect(start.offset(lp_offset) as *mut c_void,
                        page_size() as size_t, PROT_NONE);
    if rv != 0 {
        panic!("mprotect failed: {}", io::Error::last_os_error());
    }

    let mut region = start.offset(page_size() as isize);

    // mlock
    if USE_MLOCK {
        // Do not lock guarded pages.
        rv = mman::mlock(region as *const c_void, region_sz as size_t);
        if rv != 0 {
            panic!("mlock failed: {}", io::Error::last_os_error());
        }
    }

    // madvise and minherit
    self::adv_imp::madvise(region, region_sz);
    self::inh_imp::minherit(start, full_sz);

    match pos {
        _ if size == region_sz => (),
        RangePos::End => {
            let offset = (region_sz - size) & !(align_sz - 1);
            region = region.offset(offset as isize);
        },
        RangePos::Rand => {
            let r = (region_sz - size).checked_div(
                align_sz).unwrap().to_isize().unwrap();
            let offset = utils::gen_range(&mut utils::rng(), 0, r) *
                align_sz as isize;
            region = region.offset(offset);
        },
        _ => ()
    }

    if let Some(fill_byte) = fill {
        ptr::write_bytes(start.offset(page_size() as isize),
                         fill_byte, region_sz);
    }

    region
}

/// Deallocate memory
///
/// `ptr` must be a pointer returned by `allocate` where `size` was
/// used as argument. `fill` indicates if the memory must be filled with
/// a specified byte value before deallocation. This function returns
/// immediately without any effect if `ptr` is `NULL` and `panic!` on
/// error.
pub unsafe fn deallocate(ptr: *mut u8, size: usize, fill: Option<u8>) {
    if ptr.is_null() {
        return;
    }

    let region_sz = page_round(size);
    let full_sz = region_sz.checked_add((2 * page_size())).unwrap();

    // Assuming the pointer is rightly located (as it should) in the first
    // page after the initial page guard.
    let region = mask_pointer(ptr);

    if let Some(fill_byte) = fill {
        // Make sure the region can be written.
        protect(region, region_sz, Prot::Write);

        utils::set_memory(region, fill_byte, region_sz);
    }

    // munlock
    if USE_MLOCK {
        let rv = mman::munlock(region as *const c_void, region_sz as size_t);
        if rv != 0 {
            panic!("munlock failed: {}", io::Error::last_os_error());
        }
    }

    let start = region.offset(-(page_size() as isize));
    let rv = mman::munmap(start as *mut c_void, full_sz as size_t);
    if rv != 0 {
        panic!("munmap failed: {}", io::Error::last_os_error());
    }
}

/// Change memory protections
///
/// `ptr` must be a pointer returned by `allocate` where `size` was
/// used as argument. This function returns immediately if `ptr` is
/// `NULL` and `panic!` on error.
pub unsafe fn protect(ptr: *mut u8, size: usize, prot: Prot) {
    if ptr.is_null() {
        return;
    }

    let rv = mman::mprotect(mask_pointer(ptr) as *mut c_void,
                            page_round(size) as size_t,
                            Prot::to_mprot(prot));
    if rv != 0 {
        panic!("mprotect failed: {}", io::Error::last_os_error());
    }
}


#[cfg(target_os = "freebsd")]
mod map_imp {
    use libc::consts::os::extra::MAP_NOCORE;
    use libc::types::os::arch::c95::c_int;

    pub fn additional_map_flags() -> c_int {
        MAP_NOCORE
    }
}

#[cfg(not(target_os = "freebsd"))]
mod map_imp {
    use libc::types::os::arch::c95::c_int;

    pub fn additional_map_flags() -> c_int {
        0
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod adv_imp {
    use libc::consts::os::bsd44::MADV_DONTFORK;
    use libc::EINVAL;
    use libc::funcs::bsd44;
    use libc::types::common::c95::c_void;
    use libc::types::os::arch::c95::{c_int, size_t};
    use std::io;


    pub unsafe fn madvise(ptr: *mut u8, size: usize) {
        let dont_dump: c_int = 16;
        let rv = bsd44::madvise(ptr as *mut c_void, size as size_t,
                                dont_dump | MADV_DONTFORK);
        if rv != 0 {
            let err = io::Error::last_os_error();
            // FIXME: EINVAL errors are currently ignored because
            // MADV_DONTDUMP and MADV_DONTFORK are not valid advices on
            // old kernels respectively Linux < 3.4 and Linux < 2.6.16.
            // There should be an explicit way - other than relying on
            // kernel's version - to check for the availability of these
            // flags in the kernel.
            if err.raw_os_error().unwrap() != EINVAL {
                panic!("madvise failed: {}", err);
            }
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod adv_imp {
    use libc::funcs::bsd44;
    use libc::types::common::c95::c_void;
    use libc::consts::os::bsd44::MADV_ZERO_WIRED_PAGES;
    use libc::types::os::arch::c95::size_t;
    use std::io;


    pub unsafe fn madvise(ptr: *mut u8, size: usize) {
        let rv = bsd44::madvise(ptr as *mut c_void, size as size_t,
                                MADV_ZERO_WIRED_PAGES);
        if rv != 0 {
            panic!("madvise failed: {}", io::Error::last_os_error());
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android",
              target_os = "macos", target_os = "ios")))]
mod adv_imp {
    pub unsafe fn madvise(_: *mut u8, _: usize) {
    }
}


#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
mod inh_imp {
    pub use libc::types::common::c95::c_void;
    pub use libc::types::os::arch::c95::{c_int, size_t};
    use std::io;


    mod bsdext {
        extern {
            pub fn minherit(addr: *mut super::c_void, len: super::size_t,
                            inherit: super::c_int) -> super::c_int;
        }
    }

    pub unsafe fn minherit(ptr: *mut u8, size: usize) {
        // Value named INHERIT_NONE on freebsd and VM_INHERIT_NONE on
        // macos/ios.
        let inherit_none: c_int = 2;
        let rv = bsdext::minherit(ptr as *mut c_void, size as size_t,
                                  inherit_none);
        if rv != 0 {
            panic!("minherit failed: {}", io::Error::last_os_error());
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios",
              target_os = "freebsd")))]
mod inh_imp {
    pub unsafe fn minherit(_: *mut u8, _: usize) {
    }
}
