# TARS [![Build Status](https://travis-ci.org/seb-m/tars.svg?branch=master)](https://travis-ci.org/seb-m/tars)


## Description

[Rust](http://www.rust-lang.org/) library implementing data structure containers with protected memory.

At a low level this project implements a [memory allocator](http://seb.dbzteam.org/rs/tars/tars/malloc/index.html) mainly inspired by [OpenBSD's malloc](http://www.openbsd.org/cgi-bin/man.cgi?query=malloc&arch=default&manpath=OpenBSD-current). This allocator is used to allocate heap memory and provide memory protections.

Two data containers are currently implemented on top of this allocator. They follow two common use cases where the first container [ProtBuf](http://seb.dbzteam.org/rs/tars/tars/struct.ProtBuf.html) a fixed-length array can be used as buffer to handle data used in sensitive operations like for instance internal buffers in crypto operations. The second container [ProtKey](http://seb.dbzteam.org/rs/tars/tars/struct.ProtKey.html) extending `ProtBuf` is more adapted for storing and handling more persistent data like secret keys or more generally all types of data requiring more fine-grained access control. When used with [its default allocator](http://seb.dbzteam.org/rs/tars/tars/allocator/struct.ProtectedBufferAllocator.html) `ProtBuf` is particularly well suited for handling small data buffers by possibly [grouping them](https://github.com/seb-m/tars/blob/master/rust-meetup-122014/malloc.png) together on a same memory page for more space efficiency and by caching empty pages when all its slots are deallocated for more performances.


### Limitations

* It's not currently possible to be sure if the compiler/LLVM won't do something unexpected such as optimizing-out instructions, or generate intermediate variables with copy of protected data on the stack. There's actually a lot of moving parts: language, compiler, code generation, target architectures.
* Experimental code, interfaces may change.
* Only tested on OS X and Linux (`x86`, `x86_64`, `arm`). Not compatible with Windows.
* Slow allocations compared to general purpose allocators albeit in some cases more optimized than just plain `mmap` pages allocations.


## Documentation

* This code is expected to target and compile with the current master branch of `rustc`.
* The generated documentation is available [here](http://seb.dbzteam.org/tars/).
* [Talk](https://github.com/seb-m/tars/raw/master/rust-meetup-122014/rust-meetup-122014-tars.pdf) given on TARS at [Bay Area Rust Meetup](https://air.mozilla.org/bay-area-rust-meetup-december-2014/) held by Mozilla SF on 2014/12/18.
* Take a look at [Curve41417.rs](https://github.com/seb-m/curve41417.rs) for an example of how this library can be used.


## License

This code is distributed under the terms of both the MIT license and the Apache License (Version 2.0).
