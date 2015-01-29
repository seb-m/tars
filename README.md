# TARS

[![Build Status](https://travis-ci.org/seb-m/tars.svg?branch=master)](https://travis-ci.org/seb-m/tars)


## Description

Data structure containers with protected memory.

The goal is to provide data structure containers with protected memory. To achieve this goal, at a low level this project implements a custom allocator inspired by [OpenBSD's malloc](http://www.openbsd.org/cgi-bin/man.cgi?query=malloc&arch=default&manpath=OpenBSD-current) which is used to allocate heap memory and provide memory protections.

Two data containers are currently built on top of this allocator. They follow two common use cases. `ProtBuf` is a fixed-length array that can be used as buffer to handle data used in sensible operations like for instance internal buffers in crypto operations. While `ProtKey` extending `ProtBuf` is itself well suited for storing and handling more persistent data like secret keys with more fine grained access control.


### Known limitations

* It's not currently possible to be sure if the compiler/LLVM won't do something unexpected such as optimizing-out instructions, or generate intermediate variables with copy of protected data on the stack. There's actually a lot of moving parts: language, compiler, code generation, target architectures...
* Experimental code, interfaces may change.
* Only tested on OS X and Linux (x86, x86_64). Not compatible with Windows.
* Slow allocations compared to general purpose allocators.


## Documentation

* The generated documentation is available [here](http://seb.dbzteam.org/rs/tars/tars/).
* [Talk](https://github.com/seb-m/tars/raw/master/rust-meetup-122014/rust-meetup-122014-tars.pdf) given on TARS at [Bay Area Rust Meetup](https://air.mozilla.org/bay-area-rust-meetup-december-2014/) held by Mozilla SF on 2014/12/18.
* Take a look at [Curve41417.rs](https://github.com/seb-m/curve41417.rs) to have an example of how this library can be used.


## License

This code is distributed under the terms of both the MIT license and the Apache License (Version 2.0).
