Read @AGENTS.md

## Known issues

- **X11 overlay has two fixes for the same bug (#1950, overlay 200px tall).** Upstream `c530a49c` makes the
  overlay resizable on X11 (`overlay.rs`, `resizable = !utils::is_wayland()`). The fork's `f260c85d` sizes it
  through GTK's `set_size_request` + `resize(1, 1)` (`overlay.rs`, the fn documented "Sizes the overlay through
  GTK's size request"). Both are active since the upstream sync in `9984c004`. They should work together but
  haven't been tested on X11. The fork's doc comment "The overlay isn't resizable" is now wrong on X11. The fork's
  fix also covers XWayland, where upstream's `is_wayland()` check keeps the window non-resizable. Left as is on
  purpose; check compact → live → compact at Bottom on X11 before changing either.
- **Soniox custom words are capped by characters, but Soniox limits tokens.** `soniox_transcription.rs` sends
  custom words as `context.terms` up to `TERMS_CHAR_BUDGET` (8,000 characters). Soniox rejects a context over
  8,000 tokens with a 400 "Context is too long", which then fails every Soniox request, live and fallback alike.
  Latin-script words stay well under, but CJK or Indic words can take a token or more per character, so a long
  list of them could go over. Fix by estimating tokens (like Deepgram's `estimated_tokens`) or lowering the
  budget to about 5,000. Not fixed yet.
