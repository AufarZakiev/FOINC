# Constitution

## Spec Format

Each module lives in `modules/{module-name}/spec.md` with these sections:

### Purpose
One sentence: what the module does and why it exists.

### State Machine
Transition table per entity:

| State | Event | → State | Side effect |

Unlisted transitions are invalid.

### API / Interface
Per endpoint: method, path, request/response JSON schema, status codes.
Per internal function: signature, one-line description, errors.
Per message (WS, Worker postMessage): format and semantics.

### Non-goals
3-4 lines: what this module does NOT do.

## Integrations

Shared types live in `integrations/` as a single Rust crate.
Types are documented via Rust doc-comments (`///`) — no separate spec files.
Module-internal types are not specified — they are the code-writer's decision.

## Project Structure

- `modules/{name}/` — spec + code (Rust crate and/or `ui/` for Vue)
- `integrations/` — shared types crate, doc-commented
- `backend/` — thin shell: Axum router composing module handlers
- `frontend/` — thin shell: Vite app composing module UIs

## Rules

- Spec is self-contained: agent reads one spec + `integrations/src/`, nothing else.
- No prose where a table works.
- No examples where a schema works.
- If the specifier can't fit it in the format — the module is too big, split it.
