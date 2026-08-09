# Huge-file viewer

Textify routes files at or above 512 MiB to a dedicated read-only viewer. It uses positional reads
and fixed-size pages, so opening a multi-gigabyte file does not allocate a document-sized buffer or
rope. Normal and large editable documents continue to use the main editor backend.

## Controls

- **Previous / Next** moves between bounded pages.
- **Find** streams forward from the visible page. Repeating the same search continues after the
  previous match.
- **Go** accepts a one-based line number or a byte offset such as `b:1048576`.
- **Copy Page** copies the visible page. Clicking lines first selects a contiguous byte range to
  copy instead.
- **Edit Page** opens the visible page as an unsaved, normal editor tab. Clicking lines first limits
  the temporary document to that selected range.

The footer reports indexing/search status and the visible byte range. Line numbers appear after
the background sparse index resolves the page; byte offsets remain available immediately.

## Safety limits

The viewer displays at most 512 lines from a 256 KiB page, copies at most 16 MiB, and reopens at
most 64 MiB for editing. Search, line-index construction, and line navigation are cancellable.
Selected ranges must begin and end on UTF-8 boundaries.
