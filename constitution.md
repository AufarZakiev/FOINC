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

Shared types live in `integrations/`:
- `integrations/src/` — Rust crate for backend/cross-service types, doc-commented via `///`.
- `integrations/ui/` — TypeScript contracts for frontend-module-to-module communication, doc-commented via JSDoc.

Module-internal types are not specified — they are the code-writer's decision.

**Cross-module contracts rule.** If module A and module B need to talk (events, shared DTOs, flow payloads), the contract lives in `integrations/`. Each module's spec references the type by name — it may NOT reference another module's spec or component. Renaming a field in `integrations/` is the only place that forces both sides to update at once, which is how synchronization is enforced.

UI flows (which module comes first, how outputs feed inputs) live in `frontend/`, not in any module spec.

## Project Structure

- `modules/{name}/` — spec + code (Rust crate and/or `ui/` for Vue)
- `integrations/src/` — shared Rust types
- `integrations/ui/` — shared TypeScript types (UI events, cross-module DTOs)
- `backend/` — thin shell: Axum router composing module handlers
- `frontend/` — thin shell: Vite app composing module UIs and owning flows

## Rules

- Spec is self-contained: agent reads one spec + `integrations/`, nothing else.
- No prose where a table works.
- No examples where a schema works.
- If the specifier can't fit it in the format — the module is too big, split it.
