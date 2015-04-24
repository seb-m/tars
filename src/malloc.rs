//! Protected memory allocation
//!
//! Commonly named functions `malloc`, `calloc`, `realloc` and `free`,
//! share the same logic and mostly the same interfaces than their posix
//! counterparts. They only add an `align` parameter for data alignment
//! mostly like `posix_memalign`. This is intended to adhere a bit more
//! to the interface provided by the `alloc::heap` module.
//!
//! `malloc_key` and `realloc_key` are a bit different. They share the
//! same interfaces than `malloc` and `realloc` but provide additional
//! guarantee expected for modifying their memory protections on their
//! allocated regions.
//!
//! All regions allocated through these functions must call `free` to
//! deallocate their memory.
//!
//! `realloc` and `realloc_key` currently always work by explictly
//! making a new copy after having allocated a new object.
//!
//! Mixing different functions together may lead to undetermined
//! behaviors. For instance if an attempt is made to reallocate memory
//! through `realloc_key` it is expected the memory must have originally
//! been allocated with `malloc_key`.
//!
//! Most errors such as bad pointers provided by callers, as well as
//! unexpected internal errors, integrity errors, are treated as
//! irrecoverable. Therefore unless otherwise specified these functions
//! will `panic!` on error and the heap will be cleaned-up on stack
//! unwinding as each allocator is instantiated and dedicated to a single
//! thread.
//!
//! Allocations of size zero are handled by returning a pointer to a
//! static page that can't be read nor written, emitting a termination
//! signal on any attempt.
//!
//! `protect_read`, `protect_write` and `protect_none` functions may be
//! used to change memory protections on regions allocated through
//! `malloc_key` and `realloc_key` exclusively.
//!
//! This malloc implementation is heavily inspired by [OpenBSD's malloc](
//! http://www.openbsd.org/cgi-bin/man.cgi?query=malloc&arch=default&
//! manpath=OpenBSD-current).
//!
use std::cell::RefCell;
use std::cmp;
use std::fmt::{self, Debug, Formatter};
use std::hash::{self, SipHasher};
use std::iter;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::rc::Rc;

use num::ToPrimitive;
use rand::Rng;

use mmap::{self, RangePos, Prot};
use utils;


// Chunks
// Minimal size of a slot in a chunk.
const MIN_CHUNK_SIZE: usize = 16;
// As the page size cannot be inferred at compile-time this value
// is provided only for use in the Dir struct. It is expected to
// be larger than the real value.
const MAX_CHUNK_SHIFT: usize = 16;
// It will only be possible to map up to MAX_CHUNK_MAPPING * 8
// slots in a chunk.
const MAX_CHUNK_MAPPING: usize = 16;

// Initial number of regions, must be a power of two.
const INITIAL_REGIONS: usize = 128;

// Junk bytes used to fill buffers after memory allocation and
// before deallocation. Even if disabled, memory will be zeroed-out
// on deallocation but will use byte of value zero.
const USE_JUNK: bool = true;
const ALLOC_JUNK: u8 = 0xd0;
const FREE_JUNK: u8 = 0xdf;

// Cache empty chunks.
const USE_CACHE: bool = true;
const MAX_CACHE_SIZE: usize = 64;

// Set `true` to this constant or pass `--cfg feature="malloc_stats"`
// to `rustc` or use `cargo build --features malloc_stats` with cargo
// to enable assembling statistics.
#[cfg(feature = "malloc_stats")]
const USE_STATS: bool = true;
#[cfg(not(feature = "malloc_stats"))]
const USE_STATS: bool = false;


#[inline]
fn page_shift() -> usize {
    mmap::page_size().trailing_zeros().to_usize().unwrap()
}

#[inline]
fn max_chunk_shift() -> usize {
    page_shift() - 1
}

#[inline]
fn max_chunk_size() -> usize {
    1_usize << max_chunk_shift()
}

#[inline]
fn min_chunk_size() -> usize {
    cmp::max(mmap::page_size() / (MAX_CHUNK_MAPPING << 3), MIN_CHUNK_SIZE)
}

#[inline]
fn max_slot_index(chunk_size: usize) -> usize {
    mmap::page_size() / chunk_size
}

#[inline]
fn chunk_size(size: usize) -> usize {
    match size {
        0 => 0,
        sz if sz <= min_chunk_size() => min_chunk_size(),
        sz if sz <= max_chunk_size() => sz.next_power_of_two(),
        _ => unreachable!()
    }
}

#[inline]
fn chunk_index(chunk_size: usize) -> usize {
    match chunk_size {
        0 => 0,
        cs => {
            assert!(cs.is_power_of_two());
            cs.trailing_zeros().to_usize().unwrap()
        }
    }
}

#[inline]
fn fill_byte_alloc(zero_fill: bool) -> Option<u8> {
    match (zero_fill, USE_JUNK) {
        (true, _) => Some(0),
        (false, true) => Some(ALLOC_JUNK),
        (_, _) => None
    }
}

#[inline]
fn fill_byte_dealloc() -> Option<u8> {
    if USE_JUNK {
        Some(FREE_JUNK)
    } else {
        Some(0)
    }
}


unsafe fn dir_alloc() -> *mut u8 {
    mmap::allocate(mem::size_of::<Dir>(),
                   mem::min_align_of::<Dir>(),
                   None,
                   Prot::ReadWrite,
                   RangePos::Rand)
}

unsafe fn dir_dealloc(ptr: *mut u8) {
    mmap::deallocate(ptr, mem::size_of::<Dir>(), Some(0));
}

unsafe fn regions_alloc(count: usize) -> *mut u8 {
    let size = count.checked_mul(mem::size_of::<Region>()).unwrap();
    mmap::allocate(size,
                   mem::min_align_of::<Region>(),
                   None,
                   Prot::ReadWrite,
                   RangePos::Start)
}

unsafe fn regions_dealloc(ptr: *mut u8, count: usize) {
    let size = count.checked_mul(mem::size_of::<Region>()).unwrap();
    mmap::deallocate(ptr, size, Some(0));
}


// Provide an interface to the pages directory.
#[doc(hidden)]
pub struct ThreadDir {
    dir: Rc<RefCell<LocalDir>>
}

impl ThreadDir {
    pub unsafe fn alloc(&mut self, size: usize, zero_fill: bool,
                        force_large: bool) -> *mut u8 {
        self.dir.borrow_mut().alloc(size, zero_fill, force_large)
    }

    pub unsafe fn realloc(&mut self, ptr: *mut u8, size: usize, zero_fill: bool,
                          force_large: bool) -> *mut u8 {
        self.dir.borrow_mut().realloc(ptr, size, zero_fill, force_large)
    }

    pub unsafe fn dealloc(&mut self, ptr: *mut u8) {
        self.dir.borrow_mut().dealloc(ptr)
    }

    pub unsafe fn protect(&mut self, ptr: *mut u8, prot: Prot) {
        self.dir.borrow_mut().protect(ptr, prot)
    }
}

impl Debug for ThreadDir {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.dir.borrow().fmt(f)
    }
}


// Structure used to manage a Dir and owned by a thread.
struct LocalDir {
    dir: *mut Dir
}

impl Drop for LocalDir {
    fn drop(&mut self) {
        unsafe {
            // Do not do anything if structure's integrity is broken.
            if self.dir.is_null() || !(*self.dir).check_integrity() {
                return;
            }

            // FIXME: temp debug
            // Dump statistics.
            // info!("{}", *self.dir);

            // Force dealloc regions metadata and objects to clean-up the heap.
            (*self.dir).scavenge();
            // Dealloc Dir.
            dir_dealloc(self.dir as *mut u8);
            self.dir = ptr::null_mut();
        }
    }
}

impl Deref for LocalDir {
    type Target = Dir;

    fn deref(&self) -> &Dir {
        unsafe {
            &*self.dir
        }
    }
}

impl DerefMut for LocalDir {
    fn deref_mut(&mut self) -> &mut Dir {
        unsafe {
            &mut *self.dir
        }
    }
}


#[doc(hidden)]
pub fn thread_dir() -> ThreadDir {
    thread_local!(static THREAD_DIR_KEY: Rc<RefCell<LocalDir>> = {
        let dir = LocalDir {
            dir: unsafe { Dir::init() }
        };
        Rc::new(RefCell::new(dir))
    });

    ThreadDir {
        dir: THREAD_DIR_KEY.with(|dir| dir.clone())
    }
}


// Central pages directory.
struct Dir {
    // Canary used for integrity checks.
    canary1: usize,
    // Pointer to the current pool of allocated regions.
    regions: *mut Region,
    // Total number of regions allocated in the pool.
    total: usize,
    // Number of unused regions.
    free: usize,
    // Cache list.
    cache1: *mut u8,
    // Cache list's back pointer.
    cache2: *mut u8,
    // Current number of cached free chunks.
    cache_len: usize,
    // Pointers to chunks with free slots where index i represents chunks
    // with slots of size-class 2^i (except for i=0 used to handle allocations
    // of size 0 and also for the first indexes which might remain unused
    // depending on the pagesize and the value of MAX_CHUNK_MAPPING).
    chunks1: [*mut u8; MAX_CHUNK_SHIFT],
    // Pointers to the last chunks of their respective chunk lists inserted
    // in chunks1.
    chunks2: [*mut u8; MAX_CHUNK_SHIFT],
    // Canary.
    canary2: usize,
    // Statistics.
    stats: Option<Stats>
}


// Aggregated Dir statistics.
struct Stats {
    // Number of allocated Large objects.
    larges: usize,
    // Cumulated size of allocated Large objects.
    larges_bytes: usize,
    // Number of allocated Chunks.
    chunks: usize,
    // Distribution of chunks in their respective size-classes.
    chunks_classes: [usize; MAX_CHUNK_SHIFT],
    // Number of deallocated chunks.
    chunks_dealloc: usize,
    // Number of cached chunks.
    cached: usize,
    // Number of chunks reused from cache.
    reused: usize,
    // Number of allocated keys.
    keys: usize,
    // Number of modifications of memory protections for
    // read, write, none.
    prot_reads: usize,
    prot_writes: usize,
    prot_nones: usize
}

#[derive(Copy, Clone)]
enum RegionType {
    // Region is free, no object attached.
    Free = 0,
    // Region holds a chunk as object.
    Chunk,
    // Region holds a "large" (> max_chunk_size()) object.
    Large,
    // Region holds an empty chunk that is currently cached.
    Cache
}


// Region hold metadata informations and point to the allocated object
// eventually returned by the allocator.
struct Region {
    // Pointer to the allocated object, i.e. either a chunk for small sizes
    // under pagesize / 2, or a standalone mmap'ed area for larger ojects
    // and keys.
    object: *mut u8,
    // Canary used for integrity checks.
    canary: usize,
    // Size-class of the chunk or full size of the mapped memory object for
    // large objects.
    size: usize,
    // Region type.
    kind: RegionType,

    // Next fields are only relevant for chunks.

    // Bits mapping for tracking free slots in chunks of a given size-class.
    mapping: [u8; MAX_CHUNK_MAPPING],
    // Next referenced chunk of the same size-class also with free slots; or
    // next cached chunk when inserted in cache.
    next: *mut u8,
    // Previous referenced chunk of the same size-class also with free slots;
    // or previous cached chunk when inserted in cache.
    prev: *mut u8,
}

// A bit countertuitive but it happens that regions are shallowly copied.
impl Copy for Region {}

impl Clone for Region {
    fn clone(&self) -> Region {
        *self
    }
}


impl Dir {
    // Singleton initialization.
    pub unsafe fn init() -> *mut Dir {
        // Various checks at runtime.
        assert!(mmap::page_size() > 1 && mmap::page_size().is_power_of_two());
        assert!(MAX_CHUNK_MAPPING * 8 < mmap::page_size() &&
                mmap::page_size() % (MAX_CHUNK_MAPPING * 8) == 0);
        assert!(INITIAL_REGIONS.is_power_of_two());

        // Need this check because MAX_CHUNK_SHIFT's value must be known at
        // compile-time as it is used statically in Dir's struct.
        assert!(max_chunk_shift() <= MAX_CHUNK_SHIFT);

        let dir = dir_alloc() as *mut Dir;
        (*dir).canary1 = utils::os_rng().gen();
        (*dir).canary2 = (*dir).canary1 ^ dir as usize;
        (*dir).total = INITIAL_REGIONS;
        (*dir).free = INITIAL_REGIONS;
        (*dir).regions = regions_alloc((*dir).total) as *mut Region;
        if USE_STATS {
            (*dir).stats = Some(Stats::new());
        } else {
            (*dir).stats = None;
        }

        dir
    }

    // This method try to force deallocate all allocated structures
    // belonging to this Dir. After calling this method, its corresponding
    // instance is left in an undetermined state and must not be used
    // anymore.
    pub unsafe fn scavenge(&mut self) {
        // Only proceed with deallocations if current state is sound.
        if self.regions.is_null() || !self.check_integrity() {
            return;
        }

        if self.total != self.free {
            let canary_dir = self.canary2;

            for i in 0_usize..self.total {
                let region = self.region_at_index_mut(i);

                // Do not do anything if region's integrity is broken.
                if region.is_free() || !region.check_integrity(canary_dir) {
                    continue;
                }

                region.dealloc_data(true);
            }
        }

        regions_dealloc(self.regions as *mut u8, self.total);
        self.regions = ptr::null_mut();
    }

    #[inline]
    pub fn check_integrity(&self) -> bool {
        self.canary2 == self.canary1 ^ (self as *const Dir as usize)
    }

    #[inline]
    fn regions_used(&self) -> usize {
        self.total.checked_sub(self.free).unwrap()
    }

    unsafe fn regions_realloc(&mut self, count: usize) {
        let prev_total = self.total;
        let prev_alloc = self.regions;
        let mut prev_regions = self.regions;

        self.total = count;
        self.free = count;
        self.regions = regions_alloc(count) as *mut Region;

        // Move regions.
        for _ in 0_usize..prev_total {
            if !(*prev_regions).is_free() {
                let new_index = self.region_pick((*prev_regions).object);
                let new_region = self.region_at_index_mut(new_index) as
                    *mut Region;
                self.free = self.free.checked_sub(1).unwrap();
                *new_region = *prev_regions;
            }
            prev_regions = prev_regions.offset(1);
        }

        // Finally deallocate old regions.
        regions_dealloc(prev_alloc as *mut u8, prev_total);
    }

    fn regions_grow(&mut self) {
        // Keep a sparse list of regions to minimize collisions on
        // insertions.
        if self.free.checked_mul(4).unwrap() >= self.total {
            return;
        }

        let count = self.total.checked_mul(2).unwrap();
        unsafe {
            self.regions_realloc(count)
        }
    }

    fn regions_shrink(&mut self) {
        if self.total == INITIAL_REGIONS ||
           self.regions_used().checked_mul(16).unwrap() >= self.total {
            return;
        }

        let count = cmp::max(self.total.checked_div(2).unwrap(),
                             INITIAL_REGIONS);
        unsafe {
            self.regions_realloc(count)
        }
    }

    #[inline]
    fn region_at_index(&self, index: usize) -> &Region {
        assert!(index < self.total);
        unsafe {
            &*self.regions.offset(index.to_isize().unwrap())
        }
    }

    #[inline]
    fn region_at_index_mut(&mut self, index: usize) -> &mut Region {
        assert!(index < self.total);
        unsafe {
            &mut *self.regions.offset(index.to_isize().unwrap())
        }
    }

    #[inline]
    fn region_mask(&self) -> usize {
        self.total - 1
    }

    #[inline]
    fn object_to_region_index(&self, object: *mut u8) -> usize {
        hash::hash::<_, SipHasher>(&(mmap::mask_pointer(object) as usize))
            as usize & self.region_mask()
    }

    fn region_pick(&self, object: *mut u8) -> usize {
        // Algorithm L Knuth volume 3, 6.4.
        let mut index = self.object_to_region_index(object);
        loop {
            if self.region_at_index(index).is_free() {
                break;
            }
            index = index.wrapping_sub(1) & self.region_mask();
        }
        index
    }

    fn region_insert(&mut self, object: *mut u8, size: usize,
                     chunk: bool) -> usize {
        // Grow regions if needed.
        self.regions_grow();

        let canary_dir = self.canary2;
        let region_index = self.region_pick(object);

        {
            let region = self.region_at_index_mut(region_index);
            region.init(object, size, chunk, canary_dir);
        }

        self.free = self.free.checked_sub(1).unwrap();
        region_index
    }

    fn region_find(&self, object: *mut u8) -> Option<usize> {
        assert!(!object.is_null());

        let start = mmap::mask_pointer(object);
        let mut index = self.object_to_region_index(object);

        loop {
            let region = self.region_at_index(index);
            if region.is_free() {
                return None;
            }
            if mmap::mask_pointer(region.object) == start {
                if !region.is_chunk() && region.object != object {
                    return None;
                } else {
                    return Some(index);
                }
            }
            index = index.wrapping_sub(1) & self.region_mask();
        }
    }

    fn region_delete(&mut self, index: usize) {
        // Algorithm R Knuth volume 3, 6.4.
        self.free += 1;
        let mut i = index;

        'a: loop {
            self.region_at_index_mut(i).set_as_free();
            let j = i;

            loop {
                i = i.wrapping_sub(1) & self.region_mask();
                let region = self.region_at_index(i);
                if region.is_free() {
                    break 'a;
                }
                let ri = self.object_to_region_index(region.object);
	        if (i <= ri && ri < j) || (ri < j && j < i) ||
                   (j < i && i <= ri) {
		    continue;
                }
                unsafe {
                    *self.regions.offset(j.to_isize().unwrap()) =
                        *(region as *const Region);
                }
                break;
            }
        }

        // Shrink regions if needed.
        self.regions_shrink();
    }

    unsafe fn list_insert(&mut self, start: &mut *mut u8, end: &mut *mut u8,
                          region: &mut Region) {
        if start.is_null() {
            assert!(end.is_null());
            region.next = ptr::null_mut();
            *end = region.object;
        } else {
            assert!(!end.is_null());
            region.next = *start;
            let first_region = self.regions.offset(self.region_find(
                *start).unwrap().to_isize().unwrap());
            (*first_region).prev = region.object;
        }

        region.prev = ptr::null_mut();
        *start = region.object;
    }

    unsafe fn list_remove(&mut self, start: &mut *mut u8, end: &mut *mut u8,
                          region: &mut Region) {
        let prev_region = if !region.prev.is_null() {
            self.regions.offset(self.region_find(region.prev).unwrap()
                                .to_isize().unwrap())
        } else {
            ptr::null_mut()
        };

        let next_region = if !region.next.is_null() {
            self.regions.offset(self.region_find(region.next).unwrap()
                                .to_isize().unwrap())
        } else {
            ptr::null_mut()
        };

        if prev_region.is_null() {
            if next_region.is_null() {
                *start = ptr::null_mut();
                *end = ptr::null_mut();
            } else {
                *start = (*next_region).object;
                (*next_region).prev = ptr::null_mut();
            }
        } else {
            if next_region.is_null() {
                (*prev_region).next = ptr::null_mut();
                *end = (*prev_region).object;
            } else {
                (*prev_region).next = (*next_region).object;
                (*next_region).prev = (*prev_region).object;
            }
        }
    }

    unsafe fn free_chunk_insert(&mut self, region_index: usize) {
        let dir: *mut Dir = mem::transmute(self);

        let region = (*dir).regions.offset(region_index.to_isize().unwrap());
        assert!((*region).is_chunk());

        let index = chunk_index((*region).size);

        if (*region).size == 0 {
            assert!((*dir).chunks1[index].is_null());
        }

        (*dir).list_insert(&mut (*dir).chunks1[index],
                           &mut (*dir).chunks2[index], &mut *region);
    }

    unsafe fn free_chunk_remove(&mut self, region_index: usize) {
        let dir: *mut Dir = mem::transmute(self);

        let region = (*dir).regions.offset(region_index.to_isize().unwrap());
        assert!((*region).is_chunk() && (*region).size != 0);

        let index = chunk_index((*region).size);
        (*dir).list_remove(&mut (*dir).chunks1[index],
                           &mut (*dir).chunks2[index], &mut *region);
    }

    #[inline]
    fn can_cache_chunk(&self) -> bool {
        USE_CACHE && self.cache_len < MAX_CACHE_SIZE
    }

    #[inline]
    fn has_cached_chunk(&self) -> bool {
        USE_CACHE && self.cache_len > 0
    }

    unsafe fn cache_chunk_insert(&mut self, region_index: usize) {
        let dir: *mut Dir = mem::transmute(self);

        let region = (*dir).regions.offset(region_index.to_isize().unwrap());
        assert!((*region).is_chunk() && (*region).size != 0);

        (*dir).list_insert(&mut (*dir).cache1, &mut (*dir).cache2,
                           &mut *region);

        (*dir).cache_len += 1;
        (*region).set_as_cache();
    }

    unsafe fn cache_chunk_take(&mut self,
                               chunk_size: usize) -> (usize, *mut u8) {
        let dir: *mut Dir = mem::transmute(self);

        let chunk = if utils::rng().gen_range(0_usize, 2_usize) == 1 {
            (*dir).cache1
        } else {
            (*dir).cache2
        };
        assert!(!chunk.is_null());

        let region_index = (*dir).region_find(chunk).unwrap();
        let region = (*dir).regions.offset(region_index.to_isize().unwrap());
        assert!((*region).check_integrity((*dir).canary2));

        (*dir).list_remove(&mut (*dir).cache1, &mut (*dir).cache2,
                           &mut *region);

        (*dir).cache_len -= 1;
        (*region).set_as_chunk(chunk_size);

        (region_index, chunk)
    }

    #[inline]
    fn has_free_chunk(&self, chunk_size: usize) -> bool {
        !self.chunks1[chunk_index(chunk_size)].is_null()
    }

    unsafe fn create_chunk(&mut self, chunk_size: usize) {
        let (region_index, chunk) = if self.has_cached_chunk() {
            if USE_STATS {
                self.stats.as_mut().unwrap().reused += 1;
            }
            self.cache_chunk_take(chunk_size)
        } else {
            let chunk = mmap::allocate(mmap::page_size(),
                                       0,
                                       fill_byte_alloc(false),
                                       Prot::ReadWrite,
                                       RangePos::Start);
            let region_index = self.region_insert(chunk, chunk_size, true);
            (region_index, chunk)
        };

        self.free_chunk_insert(region_index);

        // A static chunk is used for allocations of size 0 and is pointing
        // to a non-readable-writable memory area.
        if chunk_size == 0 {
            mmap::protect(chunk, mmap::page_size(), Prot::None);
        }
    }

    unsafe fn take_chunk_slot(&mut self, chunk_size: usize, real_size: usize,
                              zero_fill: bool) -> *mut u8 {
        debug_assert!(self.has_free_chunk(chunk_size) &&
                      chunk_size >= real_size);

        let index = chunk_index(chunk_size);
        // Either take the first chunk or the last one.
        let chunk = if utils::rng().gen_range(0_usize, 2_usize) == 1 {
            self.chunks1[index]
        } else {
            self.chunks2[index]
        };
        assert!(!chunk.is_null());

        if index == 0 {
            return chunk;
        }

        if USE_STATS {
            self.stats.as_mut().unwrap().chunks_classes[index] += 1;
        }

        let region_index = self.region_find(chunk).unwrap();

        let (slot_index, chunk_now_full) = {
            let canary_dir = self.canary2;
            let region = self.region_at_index_mut(region_index);
            assert!(region.check_integrity(canary_dir));

            let slot_index = region.take_chunk_slot();
            (slot_index, region.is_full_chunk())
        };

        if chunk_now_full {
            self.free_chunk_remove(region_index);
        }

        let slot = chunk.offset((slot_index * chunk_size) as isize);

        if let Some(fill_byte) = fill_byte_alloc(zero_fill) {
            ptr::write_bytes(slot, fill_byte, chunk_size);
        }

        slot
    }

    pub unsafe fn alloc(&mut self, size: usize, zero_fill: bool,
                        force_large: bool) -> *mut u8 {
        assert!(self.check_integrity());

        if force_large || size > max_chunk_size() {
            if USE_STATS {
                self.stats.as_mut().unwrap().larges += 1;
                self.stats.as_mut().unwrap().larges_bytes += size;
            }

            let (prot, pos) = if force_large {
                if USE_STATS {
                    self.stats.as_mut().unwrap().keys += 1;
                }
                (Prot::Write, RangePos::End)
            } else {
                (Prot::ReadWrite, RangePos::Start)
            };

            let object = mmap::allocate(size, 0, fill_byte_alloc(zero_fill),
                                        prot, pos);
            self.region_insert(object, size, false);
            object as *mut u8
        } else {
            if USE_STATS {
                self.stats.as_mut().unwrap().chunks += 1;
            }

            let chunk_size = chunk_size(size);

            if !self.has_free_chunk(chunk_size) {
                self.create_chunk(chunk_size);
            }

            self.take_chunk_slot(chunk_size, size, zero_fill)
        }
    }

    pub unsafe fn realloc(&mut self, ptr: *mut u8, size: usize, zero_fill: bool,
                          force_large: bool) -> *mut u8 {
        assert!(self.check_integrity());

        let nptr = self.alloc(size, zero_fill, force_large);
        assert!(!nptr.is_null());

        if ptr.is_null() {
            return nptr;
        }

        let prev_size = {
            let region_index = self.region_find(ptr).unwrap();
            let region = self.region_at_index(region_index);

            assert!(region.check_integrity(self.canary2));

            region.size
        };

        ptr::copy_nonoverlapping(ptr as *const u8, nptr,
                                 cmp::min(size, prev_size));

        self.dealloc(ptr);
        nptr
    }

    unsafe fn free_chunk_slot(&mut self, region_index: usize, offset: usize) {
        let (chunk_was_full, chunk_is_empty) = {
            let region = self.region_at_index_mut(region_index);
            assert!(region.is_chunk() && region.size != 0);

            let was_full = region.is_full_chunk();

            region.free_chunk_slot(offset);

            let is_empty = region.is_empty_chunk();
            (was_full, is_empty)
        };

        if chunk_was_full {
            self.free_chunk_insert(region_index);
        }

        if chunk_is_empty {
            self.free_chunk_remove(region_index);
        }
    }

    pub unsafe fn dealloc(&mut self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }

        assert!(self.check_integrity());

        // Potentially signals a double free in case a region is not found.
        let region_index = self.region_find(ptr).unwrap();

        let region = &mut *self.regions.offset(region_index.to_isize().unwrap());
        assert!(region.check_integrity(self.canary2));

        match region.kind {
            RegionType::Chunk => {
                if USE_STATS {
                    self.stats.as_mut().unwrap().chunks_dealloc += 1;
                }

                // Kind of static chunk, never deallocated.
                if region.size == 0 {
                    return;
                }

                let chunk_offset = (ptr as usize).checked_sub(
                    region.object as usize).unwrap();

                // Free chunk slot.
                self.free_chunk_slot(region_index, chunk_offset);

                if region.is_empty_chunk() {
                    if self.can_cache_chunk() {
                        // Cache region and its chunk object.
                        self.cache_chunk_insert(region_index);
                        if USE_STATS {
                            self.stats.as_mut().unwrap().cached += 1;
                        }
                    } else {
                        // Delete object and regions's metadata.
                        region.dealloc_data(false);
                        self.region_delete(region_index);
                    }
                }
            },
            RegionType::Large => {
                assert_eq!(region.object, ptr);
                region.dealloc_data(false);
                self.region_delete(region_index);
            },
            _ => unreachable!()
        }
    }

    pub unsafe fn protect(&mut self, ptr: *mut u8, prot: Prot) {
        if ptr.is_null() {
            return;
        }

        assert!(self.check_integrity());

        if USE_STATS {
            match prot {
                Prot::Read => self.stats.as_mut().unwrap().prot_reads += 1,
                Prot::Write => self.stats.as_mut().unwrap().prot_writes +=1,
                Prot::None => self.stats.as_mut().unwrap().prot_nones += 1,
                _ => ()
            }
        }


        let canary_dir = self.canary2;
        let region_index = self.region_find(ptr).unwrap();
        let region = self.region_at_index_mut(region_index);

        assert!(region.check_integrity(canary_dir));
        assert_eq!(region.kind as usize, RegionType::Large as usize);

        assert_eq!(region.object, ptr);
        mmap::protect(region.object, region.size, prot);
    }
}

impl Debug for Dir {
    fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
        try!(write!(fmt, "Current state:\n"));

        assert!(!self.regions.is_null());
        let mut num_free = 0_usize;
        let mut num_chunk = 0_usize;
        let mut num_large = 0_usize;
        let mut num_cache = 0_usize;
        for i in 0_usize..self.total {
            let region = self.region_at_index(i);

            match region.kind {
                RegionType::Free => num_free += 1,
                RegionType::Chunk => num_chunk += 1,
                RegionType::Large => num_large += 1,
                RegionType::Cache => num_cache += 1
            }
        }
        try!(write!(fmt, "regions:\n"));
        try!(write!(fmt, "total:         {}\n", self.total));
        try!(write!(fmt, "free:          {}\n",  num_free));
        try!(write!(fmt, "chunks:        {}\n", num_chunk));
        try!(write!(fmt, "large objects: {}\n", num_large));
        try!(write!(fmt, "cached chunks: {}\n", num_cache));

        try!(write!(fmt, "chunks:\n"));
        for i in iter::range_inclusive(0_usize, max_chunk_shift()) {
            let size = 1_usize << i;

            if i != 0 && size < min_chunk_size() {
                continue;
            }

            if self.chunks1[i].is_null() {
                try!(write!(fmt, "chunk size: {:<5} -> empty\n", size));
            } else {
                let mut l: usize = 0;
                let mut object: *mut u8 = self.chunks1[i];
                loop {
                    l += 1;
                    let region = self.region_at_index(
                        self.region_find(object).unwrap());
                    if !region.next.is_null() {
                        object = region.next;
                    } else {
                        break;
                    }
                }
                try!(write!(fmt, "chunk size: {:<5} -> free chunks: {}\n",
                            size, l));
            }
        }

        if USE_STATS {
            try!(write!(fmt, "\n{:?}", self.stats.as_ref().unwrap()));
        }

        Ok(())
    }
}


impl Stats {
    fn new() -> Stats {
        Stats {
            larges: 0,
            larges_bytes: 0,
            chunks: 0,
            chunks_classes: [0; MAX_CHUNK_SHIFT],
            chunks_dealloc: 0,
            cached: 0,
            reused: 0,
            keys: 0,
            prot_reads: 0,
            prot_writes: 0,
            prot_nones: 0
        }
    }
}

impl Debug for Stats {
    fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
        try!(write!(fmt, "Stats:\n"));
        try!(write!(fmt, "larges:       {}\n", self.larges));
        try!(write!(fmt, "larges_bytes: {}\n", self.larges_bytes));
        try!(write!(fmt, "chunks:       {}\n", self.chunks));
        try!(write!(fmt, "chunks sizes:\n"));
        for i in iter::range_inclusive(0_usize, max_chunk_shift()) {
            let size = 1_usize << i;
            if i != 0 && size < min_chunk_size() {
                continue;
            }
            try!(write!(fmt, " {:5} bytes: {}\n", size,
                        self.chunks_classes[i]));
        }
        try!(write!(fmt, "chks_dealloc: {}\n", self.chunks_dealloc));
        try!(write!(fmt, "cached:       {}\n", self.cached));
        try!(write!(fmt, "reused:       {}\n", self.reused));
        try!(write!(fmt, "keys:         {}\n", self.keys));
        try!(write!(fmt, "prot_reads:   {}\n", self.prot_reads));
        try!(write!(fmt, "prot_writes:  {}\n", self.prot_writes));
        try!(write!(fmt, "prot_nones:   {}\n", self.prot_nones));
        Ok(())
    }
}


impl Region {
    fn init(&mut self, object: *mut u8, size: usize, chunk: bool,
            canary_dir: usize) {
        assert!(!object.is_null());
        self.object = object;
        self.canary = canary_dir ^ object as usize;
        self.kind = if chunk {
            RegionType::Chunk
        } else {
            RegionType::Large
        };
        self.size = size;

        if chunk {
            self.init_chunk();
        }
    }

    fn init_chunk(&mut self) {
        if self.size == 0 {
            return;
        }

        let max_index = max_slot_index(self.size);

        for i in 0_usize..max_index >> 3 {
            self.mapping[i] = 0xff;
        }

        if max_index % 8 != 0 {
            self.mapping[max_index >> 3] = 0;
        }

        for i in 0_usize..max_index % 8 {
            self.mapping[max_index >> 3] |= 1 << i;
        }

        self.next = ptr::null_mut();
        self.prev = ptr::null_mut();
    }

    #[inline]
    fn check_integrity(&self, canary_dir: usize) -> bool {
        !self.is_free() && self.canary == canary_dir ^ self.object as usize
    }

    unsafe fn set_as_cache(&mut self) {
        assert_eq!(self.kind as usize, RegionType::Chunk as usize);

        self.kind = RegionType::Cache;
        self.size = 0;

        assert!(!self.object.is_null());
        mmap::protect(self.object, mmap::page_size(), Prot::None);
    }

    unsafe fn set_as_chunk(&mut self, chunk_size: usize) {
        assert_eq!(self.kind as usize, RegionType::Cache as usize);

        self.kind = RegionType::Chunk;
        self.size = chunk_size;
        self.init_chunk();

        assert!(!self.object.is_null());
        mmap::protect(self.object, mmap::page_size(), Prot::ReadWrite);
    }

    fn set_as_free(&mut self) {
        self.object = ptr::null_mut();
        self.kind = RegionType::Free;
    }

    #[inline]
    fn is_free(&self) -> bool {
        debug_assert_eq!(self.object.is_null(),
                         (self.kind as usize == RegionType::Free as usize));
        self.kind as usize == RegionType::Free as usize
    }

    #[inline]
    fn is_chunk(&self) -> bool {
        self.kind as usize == RegionType::Chunk as usize
    }

    fn chunk_state(&self, full: bool) -> bool {
        // Check this region represents a valid chunk.
        assert!(self.is_chunk() && self.size != 0);

        let byte_val = if full {
            0
        } else {
            255
        };

        let max_slot_index = max_slot_index(self.size);

        for i in 0_usize..max_slot_index >> 3 {
            if self.mapping[i] != byte_val {
                return false;
            }
        }

        for i in 0_usize..max_slot_index % 8 {
            let expected_val = if full {
                0
            } else {
                1 << i
            };
            if self.mapping[max_slot_index >> 3] & (1 << i) != expected_val {
                return false;
            }
        }

        true
    }

    fn is_full_chunk(&self) -> bool {
        self.chunk_state(true)
    }

    fn is_empty_chunk(&self) -> bool {
        self.chunk_state(false)
    }

    #[inline]
    fn chunk_slot_is_free(&self, index: usize) -> bool {
        debug_assert!(index < max_slot_index(self.size));
        self.mapping[index >> 3] & (1 << (index % 8)) != 0
    }

    fn take_chunk_slot(&mut self) -> usize {
        debug_assert!(self.is_chunk() && self.size != 0 &&
                      !self.is_full_chunk());

        let max_slot_index = max_slot_index(self.size);
        assert!(max_slot_index > 0);
        let mut slot_index = utils::rng().gen_range(0_usize, max_slot_index);

        let mut found = false;
        for _ in 0_usize..max_slot_index {
            if self.chunk_slot_is_free(slot_index) {
                found = true;
                break;
            }
            slot_index = slot_index.wrapping_sub(1) % max_slot_index;
        }
        assert!(found);

        // Mark slot as taken.
        self.mapping[slot_index >> 3] ^= 1 << (slot_index % 8);

        slot_index
    }

    unsafe fn free_chunk_slot(&mut self, offset: usize) {
        debug_assert!(self.is_chunk() && self.size != 0);

        assert!(offset < mmap::page_size() && offset % self.size == 0);
        let slot_index = offset.checked_div(self.size).unwrap();
        assert!(slot_index < max_slot_index(self.size));

        // Potentially detected a double free.
        assert!(!self.chunk_slot_is_free(slot_index));

        // Mark slot as free.
        self.mapping[slot_index >> 3] |= 1 << (slot_index % 8);

        utils::set_memory(self.object.offset(offset as isize),
                          fill_byte_dealloc().unwrap(), self.size);
    }

    unsafe fn dealloc_data(&mut self, forced: bool) {
        match self.kind {
            RegionType::Chunk => {
                let fill = if forced {
                    fill_byte_dealloc()
                } else {
                    None
                };
                mmap::deallocate(self.object, mmap::page_size(), fill);
            },
            RegionType::Large => {
                mmap::deallocate(self.object, self.size, fill_byte_dealloc());
            },
            RegionType::Cache => {
                mmap::deallocate(self.object, mmap::page_size(), None);
            },
            _ => unreachable!()
        }

        self.set_as_free();
    }
}

impl Debug for Region {
    fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
        if self.is_free() {
            return write!(fmt, "Region: free\n");
        }

        try!(write!(fmt, "Region: used, type: {}, size: {}\n",
                    self.kind as usize, self.size));

        if self.is_chunk() && self.size != 0 {
            try!(write!(fmt, "mapping:"));
            for i in 0_usize..max_slot_index(self.size) {
                if i % 8 == 0 {
                    try!(write!(fmt, " "));
                }
                if i > 0 && i % 32 == 0  {
                    try!(write!(fmt, "\n         "));
                }
                try!(write!(fmt, "{}", self.chunk_slot_is_free(i) as usize));
            }
            try!(write!(fmt, "\n"));
        }

        Ok(())
    }
}


fn align_to_size(align: usize, size: usize) -> Option<usize> {
    match align {
        0 => Some(size),
        algn if !algn.is_power_of_two() || algn >= mmap::page_size() => None,
        algn if algn <= mmap::MIN_ALIGN => Some(size),
        algn => Some(cmp::max(size.next_power_of_two(), algn))
    }
}

unsafe fn xmalloc(size: usize, align: usize, zero_fill: bool,
                  force_large: bool) -> *mut u8 {
    let sz = match align_to_size(align, size) {
        Some(sz) => sz,
        None => return ptr::null_mut()
    };

    thread_dir().alloc(sz, zero_fill, force_large)
}

/// Allocate memory
///
/// Provides the same interface than the usual `malloc` function along
/// with the following specifities: the requested alignment `align` must
/// be a power of two smaller than pagesize. Also, `align` is expected
/// to be equal to zero if no specific alignment needs to be requested.
/// This function returns `NULL` if `align` is invalid and otherwise
/// `panic!` on error.
pub unsafe fn malloc(size: usize, align: usize) -> *mut u8 {
    xmalloc(size, align, false, false)
}

/// Allocate memory
///
/// Provides the same interface than the usual `calloc` function, see
/// `malloc` for this implementation's specifities.
pub unsafe fn calloc(count: usize, size: usize, align: usize) -> *mut u8 {
    let full_sz = count.checked_mul(size).unwrap();
    xmalloc(full_sz, align, true, false)
}

/// Allocate memory and allow changes to memory protections
///
/// Similar to `malloc` but with the additional guarantee that its
/// returned pointer can be used with `protect_read`, `protect_write`
/// or `protect_none` to change memory protections on its allocated
/// region.
pub unsafe fn malloc_key(size: usize, align: usize) -> *mut u8 {
    xmalloc(size, align, false, true)
}


unsafe fn xrealloc(ptr: *mut u8, size: usize, align: usize,
                   force_large: bool) -> *mut u8 {
    let sz = match align_to_size(align, size) {
        Some(sz) => sz,
        None => return ptr::null_mut()
    };

    thread_dir().realloc(ptr, sz, false, force_large)
}

/// Reallocate memory
///
/// Provides the same interface than the usual `realloc` function, see
/// `malloc` for this implementation's specifities.
pub unsafe fn realloc(ptr: *mut u8, size: usize, align: usize) -> *mut u8 {
    xrealloc(ptr, size, align, false)
}

/// Reallocate memory and allow changes to memory protections
///
/// Must only be called after memory allocation with `malloc_key`.
pub unsafe fn realloc_key(ptr: *mut u8, size: usize, align: usize) -> *mut u8 {
    xrealloc(ptr, size, align, true)
}


/// Free memory
///
/// Provides the same interface than the usual `free` function. This function
/// has no effect if `ptr` is a `NULL` pointer or a pointer to a zero-sized
/// area. But will `panic!` for any other kind of error.
pub unsafe fn free(ptr: *mut u8) {
    thread_dir().dealloc(ptr);
}


/// Set memory protection to read-only
///
/// `ptr` must have been allocated through `malloc_key` exclusively.
/// This function has no effect if `ptr` is `NULL` but `panic!` for
/// any other kind of error.
pub unsafe fn protect_read(ptr: *mut u8) {
    thread_dir().protect(ptr, Prot::Read);
}

/// Set memory protection to write-only
///
/// See `protect_read` for its usage.
///
/// Note that due to hardware limitations on most architectures (such
/// as x86, x86_64), setting a `write` page protection will also implicitly
/// imply granting `read` access too (see `man mprotect`).
pub unsafe fn protect_write(ptr: *mut u8) {
    thread_dir().protect(ptr, Prot::Write);
}

/// Set memory protection to prevent any access
///
/// See `protect_read` for its usage.
pub unsafe fn protect_none(ptr: *mut u8) {
    thread_dir().protect(ptr, Prot::None);
}


#[cfg(test)]
mod test {
    use alloc::heap;
    use libc;
    use std::cmp;
    use std::collections::HashSet;
    use std::env;
    use std::ptr;
    use std::sync::Future;
    use std::usize;
    use test::Bencher;

    use num::ToPrimitive;
    use rand::{thread_rng, Rng};

    use mmap;


    fn print_dir_state() {
        info!("{:?}", super::thread_dir())
    }

    fn write_byte(ptr: *mut u8, index: usize) {
        unsafe {
            *ptr.offset(index.to_isize().unwrap()) = (index % 256) as u8;
        }
    }

    fn read_byte(ptr: *const u8, index: usize) {
        unsafe {
            assert_eq!(*ptr.offset(index.to_isize().unwrap()),
                       (index % 256) as u8);
        }
    }


    #[test]
    fn test_malloc_chunks() {
        const NA: usize = 2048;
        let mut p: [*mut u8; NA] = [ptr::null_mut(); NA];
        let mut s: [usize; NA] = [0_usize; NA];

        for i in 0_usize..NA {
            p[i] = unsafe {
                let size = thread_rng().gen_range(0_usize, env::page_size() >> 1);
                s[i] = size;
                super::malloc(size, 0)
            };
            assert!(!p[i].is_null());

            for j in 0_usize..s[i] {
                write_byte(p[i], j);
            }
        }

        for i in (0_usize..NA).step_by(16) {
            for j in 0_usize..s[i] {
                read_byte(p[i] as *const u8, j);
            }

            unsafe {
                super::free(p[i]);
            }
        }

        for i in (0_usize..NA).step_by(16) {
            p[i] = unsafe {
                let size = thread_rng().gen_range(0_usize, env::page_size() >> 1);
                s[i] = size;
                super::malloc(size, 0)
            };
            assert!(!p[i].is_null());

            for j in 0_usize..s[i] {
                write_byte(p[i], j);
            }
        }

        for i in 0_usize..NA {
            for j in 0_usize..s[i] {
                read_byte(p[i] as *const u8, j);
            }

            unsafe {
                super::free(p[i]);
            }
        }

        let d = super::thread_dir();
        assert!((d.dir.borrow().total - d.dir.borrow().cache_len) <=
                d.dir.borrow().free + 1);
    }

    #[test]
    fn test_malloc_large() {
        const NA: usize = 2048;
        let mut p: [*mut u8; NA] = [ptr::null_mut(); NA];
        let mut s: [usize; NA] = [0_usize; NA];

        for i in 0_usize..NA {
            p[i] = unsafe {
                let size = thread_rng().gen_range((env::page_size() >> 1) + 1,
                                                  env::page_size() << 3);
                s[i] = size;
                super::malloc(size, 0)
            };
            assert!(!p[i].is_null());

            for j in 0_usize..s[i] {
                write_byte(p[i], j);
            }
        }

        for i in 0_usize..NA {
            for j in 0_usize..s[i] {
                read_byte(p[i] as *const u8, j);
            }

            unsafe {
                super::free(p[i]);
            }
        }

        let d = super::thread_dir();
        assert_eq!(d.dir.borrow().total, d.dir.borrow().free);
    }

    #[test]
    fn test_malloc_keys() {
        const NA: usize = 2048;
        let mut p: [*mut u8; NA] = [ptr::null_mut(); NA];
        let mut s: [usize; NA] = [0_usize; NA];

        for i in 0_usize..NA {
            p[i] = unsafe {
                let size = thread_rng().gen_range(0, env::page_size() << 3);
                s[i] = size;
                super::malloc_key(size, 0)
            };
            assert!(!p[i].is_null());

            for j in 0_usize..s[i] {
                write_byte(p[i], j);
            }
        }

        for i in 0_usize..NA {
            for j in 0_usize..s[i] {
                read_byte(p[i] as *const u8, j);
            }

            unsafe {
                super::free(p[i]);
            }
        }

        let d = super::thread_dir();
        assert_eq!(d.dir.borrow().total, d.dir.borrow().free);
    }

    #[test]
    fn test_malloc_mixed() {
        const NA: usize = 8192;
        let mut p: [*mut u8; NA] = [ptr::null_mut(); NA];
        let mut s: [usize; NA] = [0_usize; NA];

        for i in 0_usize..NA {
            p[i] = unsafe {
                let size = thread_rng().gen_range(0_usize, env::page_size() << 2);
                s[i] = size;
                super::malloc(size, 0)
            };
            assert!(!p[i].is_null());

            for j in 0_usize..s[i] {
                write_byte(p[i], j);
            }
        }

        print_dir_state();

        for i in (0_usize..NA).step_by(16) {
            for j in 0_usize..s[i] {
                read_byte(p[i] as *const u8, j);
            }

            unsafe {
                super::free(p[i]);
            }
        }

        print_dir_state();

        for i in (0_usize..NA).step_by(16) {
            p[i] = unsafe {
                let size = thread_rng().gen_range(0_usize, env::page_size() << 4);
                s[i] = size;
                super::malloc(size, 0)
            };
            assert!(!p[i].is_null());

            for j in 0_usize..s[i] {
                write_byte(p[i], j);
            }
        }

        print_dir_state();

        for i in 0_usize..NA {
            for j in 0_usize..s[i] {
                read_byte(p[i] as *const u8, j);
            }

            unsafe {
                super::free(p[i]);
            }
        }

        print_dir_state();

        let d = super::thread_dir();
        assert!((d.dir.borrow().total - d.dir.borrow().cache_len) <=
                d.dir.borrow().free + 1);
    }

    #[test]
    fn test_malloc_align() {
        let mut sptr: *mut u8;
        let mut kptr: *mut u8;
        let mut align = 1;
        let mut size;

        while align < env::page_size() {
            size = thread_rng().gen_range(0_usize, env::page_size() << 2);

            sptr = unsafe {
                super::malloc(size, align)
            };
            kptr = unsafe {
                super::malloc_key(size, align)
            };
            assert!(!sptr.is_null() && !kptr.is_null());
            assert!(sptr as usize % align == 0 && kptr as usize % align == 0);

            for i in 0_usize..size {
                write_byte(sptr, i);
                write_byte(kptr, i);
            }

            for i in 0_usize..size {
                read_byte(sptr as *const u8, i);
                read_byte(kptr as *const u8, i);
            }

            unsafe {
                super::free(sptr);
                super::free(kptr);
            }

            align *= 2;
        }

        unsafe {
            assert!(super::malloc(42, align).is_null());
            assert!(super::malloc_key(42, align).is_null());
        }
    }

    #[test]
    #[should_panic(message = "double free")]
    fn test_double_free_chunk1() {
        unsafe {
            let p1 = super::malloc(42, 0);
            let p2 = super::malloc(42, 0);
            assert!(!p1.is_null() && !p2.is_null());

            super::free(p1);
            super::free(p1);
        }
    }

    #[test]
    #[should_panic(message = "double free")]
    fn test_double_free_chunk2() {
        unsafe {
            let p = super::malloc(42, 0);
            assert!(!p.is_null());
            super::free(p);
            super::free(p);
        }
    }

    #[test]
    #[should_panic(message = "invalid free")]
    fn test_free_invalid1() {
        let p: *mut u8 = 42 as *mut u8;
        unsafe {
            super::free(p);
        }
    }

    #[test]
    #[should_panic(message = "invalid pointer")]
    fn test_free_invalid2() {
        unsafe {
            // Stored in a chunk of size 64.
            let p1 = super::malloc(42, 0);
            let p2 = if mmap::mask_pointer(p1) == p1 {
                p1.offset(64)
            } else {
                mmap::mask_pointer(p1)
            };
            super::free(p2);
        }
    }

    #[test]
    #[should_panic(message = "invalid pointer")]
    fn test_free_invalid3() {
        unsafe {
            let p = super::malloc(env::page_size(), 0);
            super::free(p.offset(64));
        }
    }

    #[test]
    #[should_panic(message = "invalid pointer")]
    fn test_protect_chunk() {
        unsafe {
            let p = super::malloc(42, 0);
            assert!(!p.is_null());
            super::protect_read(p);
        }
    }

    #[test]
    #[should_panic(message = "invalid pointer")]
    fn test_protect_missing() {
        let p: *mut u8 = 42 as *mut u8;

        unsafe {
            super::protect_read(p);
        }
    }

    #[test]
    fn test_protect() {
        // Actually can't test access violations with #[should_panic]
        // without killing the test runner.

        unsafe {
            let p = super::malloc_key(42, 0);
            assert!(!p.is_null());

            super::protect_write(p);
            write_byte(p, 42);

            super::protect_none(p);

            super::protect_read(p);
            read_byte(p as *const u8, 42);

            super::protect_none(p);
            super::free(p);
        }
    }

    #[test]
    fn test_null() {
        unsafe {
            super::free(ptr::null_mut());
            super::protect_none(ptr::null_mut());
            super::protect_read(ptr::null_mut());
            super::protect_write(ptr::null_mut());
        }
    }

    #[test]
    fn test_zero() {
        // Actually can't test dereferencing this pointer while expecting
        // #[should_panic] without killing the test runner.

        unsafe {
            let p = super::malloc(0, 0);
            assert!(!p.is_null());
            super::free(p);
        }
    }

    #[test]
    fn test_realloc() {
        let size1 = thread_rng().gen_range(0_usize, env::page_size() << 2);
        let size2 = thread_rng().gen_range(0_usize, env::page_size() << 2);

        unsafe {
            let mut p1 = super::malloc(size1, 0);
            let mut p2 = super::malloc_key(size1, 0);
            assert!(!p1.is_null());
            assert!(!p2.is_null());

            for i in 0_usize..size1 {
                write_byte(p1, i);
                write_byte(p2, i);
            }

            p1 = super::realloc(p1, size2, 0);
            p2 = super::realloc_key(p2, size2, 0);
            assert!(!p1.is_null());
            assert!(!p2.is_null());

            for i in 0_usize..cmp::min(size1, size2) {
                read_byte(p1 as *const u8, i);
                read_byte(p2 as *const u8, i);
            }

            if size2 > size1 {
                for i in size1..size2 {
                    write_byte(p1, i);
                    write_byte(p2, i);
                }

                for i in 0_usize..size2 {
                    read_byte(p1 as *const u8, i);
                    read_byte(p2 as *const u8, i);
                }
            }

            super::free(p1);
            super::free(p2);
        }
    }

    #[test]
    fn test_realloc_zero() {
        let size = thread_rng().gen_range(0_usize, env::page_size() << 2);

        unsafe {
            let mut p1 = super::malloc(size, 0);
            assert!(!p1.is_null());

            for i in 0_usize..size {
                write_byte(p1, i);
            }

            p1 = super::realloc(p1, 0, 0);
            assert!(!p1.is_null());

            let p2 = super::realloc(p1, 0, 0);
            assert!(!p2.is_null());

            assert_eq!(p1, p2);

            super::free(p1);
            super::free(p2);
        }
    }

    #[test]
    fn test_calloc() {
        unsafe {
            // Large
            let mut p = super::calloc(1, env::page_size(), 0);
            assert!(!p.is_null());

            for i in 0_usize..env::page_size() {
                assert_eq!(*p.offset(i as isize), 0);
            }
            super::free(p);

            // Chunk
            p = super::calloc(1, 42, 0);
            assert!(!p.is_null());

            for i in 0_usize..42 {
                assert_eq!(*p.offset(i as isize), 0);
            }
            super::free(p);
        }
    }

    #[test]
    #[should_panic(message = "integer overflow")]
    fn test_calloc_overflow() {
        unsafe {
            super::calloc(usize::MAX, 2, 0);
        }
    }

    #[test]
    fn test_dir_addr() {
        let size = 10;

        let mut futures: Vec<_> =
            (0_usize..size).map(|_| Future::spawn(move || {
                let d1 = super::thread_dir();
                let d2 = d1.dir.borrow();
                d2.dir as usize
            })).collect();

        let addrs: HashSet<usize> =
            futures.iter_mut().map(|ref mut ft| ft.get()).collect();

        assert_eq!(addrs.len(), size);
        assert!(!addrs.contains(&0));
    }

    #[test]
    #[should_panic(message = "buffer overrun")]
    fn test_overrun1() {
        // Not enabled by default because test runner is killed.
        let enabled = false;

        if !enabled {
            assert!(false);
            return;
        }

        unsafe {
            let s = super::malloc(256, 0);
            assert!(!s.is_null());

            let d = heap::allocate(2 * env::page_size(), 0);
            assert!(!d.is_null());

            ptr::copy_nonoverlapping(s as *const u8, d, 2 * env::page_size());
        }
    }

    #[test]
    #[should_panic(message = "buffer overrun")]
    fn test_overrun2() {
        // Not enabled by default because test runner is killed.
        let enabled = false;

        if !enabled {
            assert!(false);
            return;
        }

        unsafe {
            let s = super::malloc(4096, 0);
            assert!(!s.is_null());

            let d = heap::allocate(2 * env::page_size(), 0);
            assert!(!d.is_null());

            ptr::copy_nonoverlapping(s as *const u8, d, 2 * env::page_size());
        }
    }

    #[bench]
    fn bench_init(b: &mut Bencher) {
        b.iter(|| {
            super::thread_dir();
        })
    }

    #[bench]
    fn bench_chunk_alloc(b: &mut Bencher) {
        b.iter(|| {
            unsafe {
                let p = super::malloc(42, 0);
                super::free(p);
            }
        })
    }

    #[bench]
    fn bench_page_alloc(b: &mut Bencher) {
        let pagesize = env::page_size();
        b.iter(|| {
            unsafe {
                let p = super::malloc(pagesize, 0);
                super::free(p);
            }
        })
    }

    #[bench]
    fn benck_libc_42_alloc(b: &mut Bencher) {
         b.iter(|| {
            unsafe {
                let p = libc::malloc(42) as *mut u8;
                libc::free(p as *mut libc::c_void);
            }
        })
    }

    #[bench]
    fn benck_libc_page_alloc(b: &mut Bencher) {
        let pagesize = env::page_size();
        b.iter(|| {
            unsafe {
                let p = libc::malloc(pagesize as libc::size_t) as *mut u8;
                libc::free(p as *mut libc::c_void);
            }
        })
    }
}
