//! TARS
//!
//! Data structure containers with protected memory.
//!
//! Souce code [repository](https://github.com/seb-m/tars) on Github.
#![crate_name = "tars"]

#![doc(html_logo_url = "http://www.rust-lang.org/logos/rust-logo-128x128-blk-v2.png",
       html_favicon_url = "http://www.rust-lang.org/favicon.ico",
       html_root_url = "http://doc.rust-lang.org/")]

#![deny(stable_features)]
#![feature(unsafe_destructor)]
#![feature(optin_builtin_traits)]
#![feature(core)]
#![feature(hash)]
#![feature(alloc)]
#![feature(os)]
#![feature(env)]
#![feature(std_misc)]

#![cfg_attr(test, feature(test))]
#![cfg_attr(test, feature(old_path))]
#![cfg_attr(test, feature(std_misc))]

#[cfg(test)] extern crate test;
#[cfg(test)] #[macro_use] extern crate log;

extern crate alloc;

extern crate libc;
extern crate rand;

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
