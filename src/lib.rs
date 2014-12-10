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
#![feature(unsafe_destructor)]
#![feature(slicing_syntax)]
#![feature(phase)]

#[cfg(test)] extern crate test;
#[cfg(test)] #[phase(plugin, link)] extern crate log;

extern crate alloc;
extern crate libc;
extern crate serialize;

pub use allocator::BufAlloc;
pub use allocator::KeyAlloc;
pub use buf::{ProtBuf, ProtBuf8};
pub use key::{ProtKey, ProtKey8, ProtKeyRead, ProtKeyWrite};

mod utils;
mod mmap;
pub mod malloc;
pub mod allocator;
mod buf;
mod key;
