# Textify performance baseline

Milestone 2 established a repeatable performance corpus rather than committing large binary
fixtures. Run it with:

```sh
cargo run --release --bin textify-perf
```

The generator creates 1/25/100 MiB JSON, 200,000 short lines, one 5 MiB line, Unicode and IME
samples, and a 100-tab session under the system temporary directory. Open and save measurements
exercise Textify's production UTF-8 loader, file policy, disk revision fingerprinting, and
chunked atomic writer.

## Baseline — 2026-08-08

Apple M1 Max (10 cores, 32 GiB), macOS arm64, Rust 1.93.1, optimized build:

| Fixture | Bytes | Mode | Open | Save |
| --- | ---: | --- | ---: | ---: |
| 1 MiB JSON | 1,048,576 | Normal | 1.09 ms | 18.33 ms |
| 25 MiB minified JSON | 26,214,400 | Large | 25.35 ms | 28.40 ms |
| 100 MiB minified JSON | 104,857,600 | Large | 110.13 ms | 69.34 ms |
| 200,000 lines | 6,800,000 | Large | 6.93 ms | 20.73 ms |
| One 5 MiB line | 5,242,880 | Large | 5.16 ms | 16.20 ms |
| Unicode/IME sample | 98 | Normal | 0.03 ms | 11.89 ms |

Peak resident memory for the complete core run was 143.38 MiB. The 25 and 100 MiB JSON files
and the 5 MiB line all selected parser-free, non-wrapping large-file mode.

The native UI logs first workspace paint plus per-file open and save durations through `tracing`.
Typing latency and scroll frame pacing remain interaction measurements: collect them with Apple
Instruments using the generated fixtures when changing GPUI, the editor fork, text layout, or
rendering code. Dependency upgrades are not accepted solely on core benchmark results.

## Huge-file viewer baseline — 2026-08-08

A native debug build opened a sparse 512 MiB UTF-8 log through the paged viewer. First workspace
paint took 1.47 ms. After the background sparse line index completed, that Textify process used
63.4 MiB resident memory. The viewer reads fixed-size pages with positional I/O and never creates
a document-sized string or rope, so resident memory is independent of the file's total size.

Core tests cover bounded pages, sparse line/byte navigation, search matches that cross read-buffer
boundaries, cancellation, UTF-8 range validation, and copy/edit limits. Repeat the UI measurement
with a real multi-gigabyte log when changing the page layout, indexing stride, or viewer controls.

## Feature-complete regression — 2026-08-08

The optimized corpus was repeated after the multicursor and lazy IDE phases. This run confirms that
the new services do not affect the production file path when no folder or language server is open:

| Fixture | Open | Save |
| --- | ---: | ---: |
| 1 MiB JSON | 1.13 ms | 21.46 ms |
| 25 MiB minified JSON | 26.61 ms | 26.97 ms |
| 100 MiB minified JSON | 107.32 ms | 71.41 ms |
| 200,000 lines | 6.60 ms | 18.00 ms |
| One 5 MiB line | 5.09 ms | 18.35 ms |
| Unicode/IME sample | 0.03 ms | 10.09 ms |

Peak resident memory was 143.27 MiB. The 25/100 MiB JSON files and 5 MiB line again selected the
parser-free large-file policy. Textify's native headless smoke test rendered an indexed project
sidebar and command palette while asserting that the initial shell owns neither a project index nor
an LSP process.

## Native memory audit — 2026-08-09

The optimized ARM64 GUI was launched once with a fresh, isolated data directory and left idle for
2 minutes 34 seconds. First workspace paint took 0.98 ms. `ps` resident-memory samples were
48,592 KiB at 13 seconds, 48,144 KiB at 51 and 91 seconds, and 35,360 KiB at 154 seconds. The
process did not exhibit idle resident-memory growth during this bounded soak.

Apple `vmmap -summary` reported a 79.0 MiB physical footprint and a 92.0 MiB peak. Most of its
large virtual address ranges were unallocated malloc zones, shared frameworks, graphics mappings,
and guard regions; virtual size is therefore not a useful proxy for Textify's physical memory use.

Two Apple `leaks` scans 30 seconds apart both reported the same 449 allocations and 51,184 bytes.
The dominant root was a 47.5 KiB Cocoa `NSArray`; the restricted release process did not expose
allocation stacks. This is a small, startup-stable framework allocation rather than evidence of
ongoing growth, but it remains a baseline to compare after GPUI or macOS upgrades. A bounded smoke
test cannot prove that no interaction-specific leak exists, so long-soak Instruments runs should
still be repeated after changes to tabs, project indexing, LSP lifecycle, or rendering.

The same profiling pass found a transient full-file copy introduced by encoding detection: valid
UTF-8 bytes were borrowed for decoding and then cloned into a `String`. Transferring ownership of
the loaded byte buffer directly into `String::from_utf8` reduced peak RSS for the identical core
corpus from 280.97 MiB to 143.38 MiB. A unit test now protects that zero-copy UTF-8 handoff.

Optimized core results after the fix:

| Fixture | Open | Save |
| --- | ---: | ---: |
| 1 MiB JSON | 1.24 ms | 17.73 ms |
| 25 MiB minified JSON | 26.32 ms | 28.01 ms |
| 100 MiB minified JSON | 113.40 ms | 81.23 ms |
| 200,000 lines | 7.13 ms | 19.23 ms |
| One 5 MiB line | 5.31 ms | 16.47 ms |
| Unicode/IME sample | 0.05 ms | 11.55 ms |

The release executables measured 20 MiB for `textify` and 1.4 MiB for `textify-perf`.
