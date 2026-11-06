# Vendored dependency

`xattr/` is xattr 0.2.3 from crates.io, used transitively by GPUI's zed-font-kit.
The original MIT and Apache-2.0 licenses are retained in that directory.

Local change: `src/sys/mod.rs` uses `libc::ENODATA` on Linux/Android; other
supported platforms keep `libc::ENOATTR`. libc 0.2.191 no longer exposes the
historical Linux alias, which otherwise prevents this dependency compiling.
No other upstream source changes.
