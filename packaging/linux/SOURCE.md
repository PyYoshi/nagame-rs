# Nagame source bundle

Distribute this archive alongside the matching AppImage and SHA256SUMS.
`nagame/` contains the exact working-tree snapshot, Cargo.lock, the local xattr
patch and vendored Rust dependencies. `licenses/` contains the application,
Rust dependency, Rust toolchain, native library and AppImage runtime notices.

## Application

With Rust 1.99.1 and the native development libraries described in
`nagame/packaging/linux/Containerfile`, run from `nagame/`:

```sh
cargo build --frozen --release --bin nagame --target x86_64-unknown-linux-gnu
```

`.cargo/config.toml` selects the included `rust-vendor/` directory. The source
bundle does not require crates.io access. Container images, Rust itself and OS
build tools are obtained separately. See `nagame/docs/distribution.md` for the
normal cross/Podman packaging workflow. Bit-for-bit reproducibility is not claimed.

## Native libraries

`native/manifest.json` records the exact binary/source package versions included
in the AppDir. `native/` contains their Ubuntu source packages (.dsc, original
tarballs and Debian patches). Use `dpkg-source -x PACKAGE.dsc` to extract each
package; its `debian/` directory contains the Ubuntu build/installation rules.
Install the corresponding build dependencies separately.

Libraries in the AppDir are dynamically linked. To replace an LGPL library,
extract the AppImage with `--appimage-extract`, replace the library in
`squashfs-root/usr/lib/` (libheif plugins: `usr/lib/libheif/`), then run
`squashfs-root/AppRun`. No signature check prevents replacement.

## PDFium

`usr/lib/libpdfium.so` in the AppDir is the prebuilt PDFium 158.0.8125.0
(`chromium/8125`, without V8/XFA) from bblanchon/pdfium-binaries, pinned by URL
and SHA256 in `nagame/packaging/linux/pdfium.json` (also recorded in
`build-info.json`). PDFium is BSD-3-Clause/Apache-2.0; its own and its bundled
third-party notices are in `licenses/third-party/pdfium/`. `just pdfium` (or
`python3 tools/fetch_pdfium.py`) downloads and verifies the same archive.

Nagame loads PDFium only in its PDF helper process, from `../lib/libpdfium.so`
relative to the executable or from the absolute path in `NAGAME_PDFIUM_LIBRARY`.
A replacement must provide the same PDFium API version (pdfium-render's
`pdfium_8125` feature); replace the file in an extracted AppDir as above.

## AppImage runtime

`appimage-runtime/` contains the upstream type2-runtime 20251108 source and its
build/relink scripts, the patched libfuse 3.15.0 input, squashfuse 0.5.2 input,
and permissive dependency source inputs. `sources.json` records download URLs
and SHA256 hashes. The upstream runtime build uses Alpine 3.21; follow its
`BUILD.md`, `scripts/common/install-dependencies.sh` (including the libfuse
patch), `scripts/docker/` and `src/runtime/Makefile` to compile/relink the
runtime with a modified libfuse. The permissive dependency snapshots provide
source and notices; they do not assert the exact APK versions in the upstream
precompiled runtime.

To use your replacement runtime, extract the matching AppImage and run the
version of appimagetool specified in `nagame/aqua.yaml`:

```sh
ARCH=x86_64 appimagetool --runtime-file ./runtime-x86_64 squashfs-root ./Nagame-modified.AppImage
```

The packaging tools themselves (linuxdeploy, appimagetool, cargo-about) are
build tools and are not included in Nagame's AppDir.
