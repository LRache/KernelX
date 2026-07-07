# kmodule usertests

Each kmodule test case is a subdirectory with:

- `loader.c`: the user program copied to `/tests/kmodule/<case>`
- `module.c`: the kernel module source
- `CMakeLists.txt`: the module target declaration

The suite Makefile discovers cases from `*/loader.c`, builds the matching
user program, builds `<case>.ko` through the shared kmodule SDK, and places both
files under `build/<arch>/output/<case>/` for the normal usertests packager.

The default autorun packager automatically derives `/tests/kmodule/<case>` from
this output layout, so adding a new kmodule case does not require editing a
central run script.
