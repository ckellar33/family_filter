# Project Instructions

## General

- Prefer small, targeted changes.
- Do not rewrite working code unnecessarily.
- Inspect existing code before proposing changes.
- Follow the existing architecture and naming conventions.
- Do not add dependencies unless necessary.

## Rust

- Run `cargo check` after meaningful changes.
- Run relevant tests after changes.
- Do not suppress compiler warnings without a reason.
- Prefer idiomatic Rust over clever abstractions.
- Preserve Send/Sync safety.

## Debugging

When investigating a bug:

1. Reproduce or trace the failure.
2. Identify the relevant code path.
3. Explain the suspected root cause.
4. Make the smallest reasonable fix.
5. Run the relevant checks/tests.
6. Inspect the resulting errors.
7. Continue until the original problem is resolved.

Do not stop after merely suggesting a fix.

## Embedded

Be especially careful with:

- interrupt context
- concurrency
- ownership
- locking
- async tasks
- hardware state
- power-management transitions
- SPI/CAN transactions
- error recovery
