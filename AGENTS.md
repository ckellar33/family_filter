# Project Instructions

## Working style
- Act on the most reasonable interpretation. State your assumption in one line and proceed; don't wait for confirmation.
- Ask a question only if you are blocked: missing information you cannot find in the repo, or an action that is destructive or irreversible. Never ask just to confirm a choice you can make.
- Make one decision and commit to it. Don't re-debate a choice once made unless a test or compiler error shows it was wrong.
- Prefer small, targeted diffs. Don't rewrite working code or refactor unrelated code.
- Follow existing architecture and naming. No new dependencies without asking.

## Commands
- Check: `cargo check --target <TARGET> --features <FEATURES>`
- Test: `cargo test <name>`  (single test), `cargo test` (all)
- Lint: `cargo clippy -- -D warnings`
- Format: `cargo fmt`

## Debugging loop
1. Reproduce: run the failing command and read the full error.
2. Locate the code path (use graphify first, see below).
3. State the suspected root cause in 1-2 sentences.
4. Make the smallest fix, then re-run the same command.
5. If it fails, read the NEW error before changing anything. Do not repeat a fix that already failed.
6. After 2 failed attempts at the same approach, stop and change strategy: re-read the relevant code, form a different hypothesis, or add logging/a test to get evidence.
7. After 4 failed attempts overall, report: what you tried, what each result showed, your current best hypothesis. Then ask.
8. Done means the original failure is gone and check/tests pass. Don't stop at suggesting a fix.

## Rust
- Run `cargo check` after every meaningful change, tests after logic changes.
- Don't suppress warnings or `unwrap()` away errors to make things compile.
- Idiomatic Rust over clever abstractions.
- Preserve `Send`/`Sync` bounds; don't add `unsafe` without a comment explaining the invariant.

## Embedded rules
- No blocking calls, allocation, or long locks in interrupt context.
- Keep lock scopes short; note lock ordering when taking more than one.
- Don't hold a lock across an `.await`.
- Check hardware state before and after power-management transitions; don't assume a peripheral is still configured after sleep/wake.
- SPI/CAN: every transaction must handle error and timeout paths and leave the bus in a known state.
- On error, recover to a safe state rather than propagating a half-configured one.

## Code navigation (graphify)
- For "how does X work / where is Y / what depends on Z": call graphify `query_graph` and `query_stats` first, then read only the files it returns.
- Skip graphify for edits in a file you already have open.
- Don't scan the repo with grep or directory listings before querying graphify.
