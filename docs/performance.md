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
