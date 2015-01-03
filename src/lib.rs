//! TARS
//!
//! Data structure containers with protected memory.
//!
//! Souce code [repository](https://github.com/seb-m/tars) on Github.
#![crate_name = "tars"]
#![experimental]
#![doc(html_logo_url = "http://www.rust-lang.org/logos/rust-logo-128x128-blk.png",
       html_favicon_url = "http://www.rust-lang.org/favicon.ico",
       html_root_url = "http://doc.rust-lang.org/")]

#![feature(macro_rules)]
#![feature(default_type_params)]
#![feature(unsafe_destructor)]
#![feature(slicing_syntax)]
#![feature(associated_types)]
#![feature(phase)]

#[cfg(test)] extern crate test;
#[cfg(test)] #[phase(plugin, link)] extern crate log;

extern crate alloc;
extern crate libc;
// FIXME: maybe we should remove this dependancy.
extern crate "rustc-serialize" as rustc_serialize;

pub use allocator::DefaultBufferAllocator;
pub use allocator::DefaultKeyAllocator;
pub use buf::{ProtBuf, ProtBuf8};
pub use key::{ProtKey, ProtKey8, ProtKeyRead, ProtKeyWrite};

mod utils;
mod mmap;
pub mod malloc;
pub mod allocator;
mod buf;
mod key;
