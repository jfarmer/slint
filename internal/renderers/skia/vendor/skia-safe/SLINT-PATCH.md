# CoreText Font Manager PoC

This is the published `skia-safe 0.153.3` crate with one macOS-only constructor and build shim.
Other targets compile the original Rust API and do not compile the shim.
The dependency on `skia-bindings` stays pinned to `=0.153.3`.

`core_text_without_catalogue()` creates a CoreText manager with an explicitly empty collection.
It loads font bytes through the same native typeface factory as the system manager.
Slint already selects fonts through Fontique/Parley, so its renderer does not need another family catalogue.
Slint falls back to the system manager if collection creation fails.

The pinned native source has a null-descriptor crash with empty collections.
`coretext-poc/fontmgr.cpp` copies `src/ports/SkFontMgr_mac_ct.cpp` from Skia tag `m153-0.101.2`, adds that guard,
and wraps the implementation in `slint_coretext_poc` to avoid replacing or colliding with the existing Skia implementation.
The exported C factory transfers an owned reference into the Rust handle.

Only this translation unit and its 49 transitive native headers are built locally.
The remaining native code uses the ordinary Skia artifact supplied by `skia-bindings`, including its binary cache.
No downloaded binary or temporary source path is required by the shim.
The headers are checked on macOS arm64 and x86_64; this is a version-specific PoC, not an ABI-stable extension.
Keep the source/header tag and the exact `skia-bindings` pin together when updating.

Upstream integration would put the null guard in Skia and expose the CoreText factory through Rust-Skia.
That would remove this extra translation unit and the vendored wrapper.

Rust-Skia is MIT licensed; see `LICENSE`.
The native source and headers retain their Skia notices; see `coretext-poc/LICENSE_SKIA`.
