# Contributing

Textify is a native desktop program, so changes should be checked on the operating systems they
touch. Small bug fixes, documentation corrections, and narrowly scoped editor improvements are the
easiest changes to review.

Before writing code, check the existing issues and [TODO.md](TODO.md). For a larger change, open an
issue first and describe the behavior you want to change. This avoids spending time on an approach
that conflicts with the editor's file-safety or memory limits.

## Development setup

The repository pins Rust 1.93.1 in `rust-toolchain.toml`. On Linux, install the native GPUI build
dependencies first:

```sh
./scripts/install-linux-build-deps.sh
```

Windows development needs the MSVC build tools and Windows SDK. macOS uses the installed Xcode
command-line tools.

Run the editor from the repository root:

```sh
cargo run --locked
```

## Before opening a pull request

Run the checks used by the release workflow:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace --all-targets -- --test-threads=1
cargo clippy --locked --workspace --all-targets -- -D warnings
```

If a change affects large-file handling, also run:

```sh
cargo run --release --bin textify-perf
```

Include the operating system you tested and the steps needed to exercise the change. Add a
screenshot for visible interface changes. Do not commit `target/`, `dist/`, local settings, session
files, or recovery copies.

## Releases

Pull requests target `main`. Releases happen separately, by merging `main` into the `prod` branch
through a pull request labeled `release:major`, `release:minor`, or `release:patch`. That merge
raises the version, builds macOS, Windows, and Linux packages, and publishes a GitHub Release. Do
not edit `version` in `Cargo.toml` by hand; the release workflow owns it.
[docs/deployment.md](docs/deployment.md) describes the whole process.

## Source layout

Application code lives in `src/`. File decoding and atomic writes are kept separate from the GPUI
interface so they can be tested without opening a window. Packaging scripts and platform metadata
live under `scripts/` and `packaging/`.

`vendor/gpui-component` is a source dependency with a few local changes. Read
[`vendor/gpui-component/TEXTIFY_FORK.md`](vendor/gpui-component/TEXTIFY_FORK.md) before editing it,
and keep unrelated upstream formatting changes out of the same pull request. The documented fork
changes are pinned by `tests/vendor_fork.rs`; if you re-vendor upstream sources, that test fails
until the fork changes are reapplied (or deliberately retired in both the guard and the document).
To pull in a new upstream version, use `scripts/upgrade-gpui-component.sh`, which replaces the
sources and reapplies the fork from `vendor/patches/gpui-component-textify.patch`; after editing
the vendored crate, regenerate that patch with the same script.

## File behavior

Changes involving saves, recovery, external file changes, encodings, or large files need tests for
failure cases as well as the successful path. A failed save must leave the existing file intact.
Large-file work must stay within the limits described in [docs/huge-files.md](docs/huge-files.md).

By contributing, you agree that your contribution may be distributed under the repository's MIT
License.
