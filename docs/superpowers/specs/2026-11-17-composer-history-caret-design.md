# Composer History Caret Design

## Goal

Make `ArrowUp` and `ArrowDown` history recall feel like normal command-line input without presenting recalled text as an active full-text selection.

## Interaction

- Recalling an older or newer entry replaces the composer value and collapses the selection at the end of the recalled text.
- Typing after recall appends at the caret instead of replacing the entire recalled entry.
- `Escape` and moving past the newest entry restore the original draft with the caret at its end.
- Existing history eligibility, IME protection, attachment retention, and the `历史 n/总数` indicator remain unchanged.

## Verification

- Component tests assert the recalled value and `selectionStart === selectionEnd === value.length`.
- Existing multiline, IME, draft restoration, and attachment tests continue to pass.
- A production frontend build and the packaged Tauri desktop are used for final validation.

## Attachment Limits

This change does not alter attachment limits: at most 8 non-empty UTF-8 text files, at most 256 KiB per file, and at most 120,000 included bytes per turn.
