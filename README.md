# TARS

## Description

Data structure containers with protected memory.

The goal is to provide data structure containers with protected memory. It implements a custom allocator inspired by [OpenBSD's malloc](http://www.openbsd.org/cgi-bin/man.cgi?query=malloc&arch=default&manpath=OpenBSD-current) and is used to allocate its heap memory and provide memory protections.

Two data containers are currently built on top of this allocator. They follow two common use cases. `ProtBuf` is a fixed-length array that can be used as buffer to handle data used in sensible operations while `ProtKey` extending `ProtBuf` is well suited for storing and handling more persistent data like secret keys.


### Known limitations

* It's not currently possible to be sure if the compiler/LLVM won't do something unexpected such as optimizing-out instructions, or generate intermediate variables with copy of protected data on the stack. There's actually a lot of moving parts: language, compiler, code generation, target architectures...
* Experimental code, lot of `unsafe`. Code and interfaces may change.
* Only tested on OS X and Linux (x86, x86_64). Not compatible with Windows.
* Slow allocations compared to general purpose allocators.
* Currently code `panic!` on errors so it may not integrate well from C code.


## Documentation

The generated documentation is also available [here](http://seb.dbzteam.org/rs/tars/tars/).


## License

This code is distributed under the terms of both the MIT license and the Apache License (Version 2.0).
