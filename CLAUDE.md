# FOINC

Distributed volunteer computing platform. Rust + Axum backend, Vue 3 + Vite frontend.

## SDD Pipeline

When the user says "implement feature X in module Y" (or similar), follow this pipeline.

### Step 1: Architecture
Spawn an Agent with the prompt from `agents/architect.md`.
Pass: the user's feature description, the target module name (if known).
The agent outputs a short approach (5-10 lines) + open questions.

**STOP: show the approach to the user. Wait for approval or corrections.**
This is the only mid-pipeline pause. The user approves the direction before any spec or code is written.

### Step 2: Branch
Create a git branch: `feat/{module}/{short-description}`

### Step 3: Spec Writing
Spawn an Agent with the prompt from `agents/spec-writer.md`.
Pass: the user's feature description, the approved approach from Step 1, the target module name.
The agent reads constitution.md, the module's spec.md, and integrations/src/.
The agent updates or creates `modules/{module}/spec.md`.

### Step 4: Spec Review
Spawn an Agent with the prompt from `agents/spec-reviewer.md`.
Pass: the target module name.
The agent reads constitution.md, the updated spec.md, and integrations/src/.
The agent outputs APPROVE or ISSUES.

If ISSUES: re-run Step 3 with the reviewer's feedback appended. Max 3 iterations.
If still not approved after 3 iterations: stop, report the unresolved issues to the user.

### Step 5: Code Writing
Spawn an Agent with the prompt from `agents/code-writer.md`.
Pass: the target module name, "implement the full spec" (or the specific task).
The agent reads the approved spec.md, integrations/src/, and existing module code.
The agent writes code in `modules/{module}/src/` and/or `modules/{module}/ui/`.

### Step 6: Code Review
Spawn an Agent with the prompt from `agents/code-reviewer.md`.
Pass: the target module name.
The agent reads spec.md and the code written in Step 5.
The agent outputs APPROVE or ISSUES.

If ISSUES: re-run Step 5 with the reviewer's feedback appended. Max 3 iterations.
If still not approved after 3 iterations: stop, report the unresolved issues to the user.

### Step 7: Test Writing
Spawn an Agent with the prompt from `agents/test-writer.md`.
Pass: the target module name, "test the full spec" (or the specific task).
The agent reads spec.md, integrations/src/, and the module code.
The agent writes tests in `modules/{module}/src/` and/or `modules/{module}/ui/`.

### Step 8: Test Review
Spawn an Agent with the prompt from `agents/test-reviewer.md`.
Pass: the target module name.
The agent reads spec.md and the tests written in Step 7.
The agent outputs APPROVE or ISSUES.

If ISSUES: re-run Step 7 with the reviewer's feedback appended. Max 3 iterations.
If still not approved after 3 iterations: stop, report the unresolved issues to the user.

### Step 9: Composition (conditional)
Run this step ONLY if one of the following is true:
- A type in `integrations/` was added or changed during this pipeline run.
- The module gained a new emit, prop, or public consumer surface that the shell must wire up.
- The module started or stopped handling an existing cross-module contract.

Otherwise skip to Step 10.

Spawn an Agent with the prompt from `agents/compositor.md`.
Pass: the target shell (`frontend/` or `backend/`) and the contract(s) that changed.
The agent reads ONLY `integrations/` and the target shell's current code; it does NOT read any module's spec or internals.
The agent updates `frontend/src/` or `backend/src/` to wire the contract (e.g., route an event from one module's component to another's prop via shell state).

If the agent reports missing fields or ambiguous flow, stop and fix the contract in `integrations/` (that is a spec-writer change, not a compositor change) before retrying.

### Step 10: Commit
Commit all changes.

### Step 11: Pull Request
Push the branch and create a PR into `main` using `gh pr create`.
Title: short description of the feature/module.
Body: summary of what was implemented, link to the spec, test plan.

The user reviews the PR. This is the only post-pipeline review point.

## Bug Fix Flow

When the user reports a bug (description + logs, or "this is broken because Y"), use this flow instead of SDD directly.

### Step B1: Hunt
Spawn an Agent with the prompt from `agents/bug-hunter.md`.
Pass: the user's bug description and any attached logs verbatim.
The agent reads freely across the whole repo (specs, module code, integrations, shells, tests) and returns a diagnosis report with a classification (`spec-gap` / `code-drift` / `contract-bug` / `composition-bug` / `env/infra`) and a concrete route to SDD.
The agent's context is one-shot — after it returns, discard it.

### Step B2: Confirm
**STOP: show the diagnosis and route to the user. Wait for approval or redirection.**
Bug-hunter can misdiagnose, especially in mutt cases. Cheap to sanity-check once; expensive to run SDD on the wrong trail.

### Step B3: Execute the route
Run the specific SDD entrypoint the bug-hunter handed off:
- `spec-gap` → SDD from Step 3 (spec-writer) for the named module(s)
- `code-drift` → SDD from Step 5 (code-writer), skip architect/spec
- `contract-bug` → edit `integrations/` first, then SDD from Step 3 for every module the contract touches
- `composition-bug` → Step 9 (compositor) only
- `env/infra` → not SDD; follow the manual checklist from the report

Steps 10 and 11 (Commit, PR) apply as usual.

## Project Structure

- `constitution.md` — spec format and rules
- `ROADMAP.md` — module order and dependencies
- `TECHNICAL_PAPER.md` — architecture, constraints, data flow (read by architect)
- `WHITE_PAPER.md` — domain context (historical, not read by agents)
- `agents/` — agent prompts (architect, spec-writer, spec-reviewer, code-writer, code-reviewer, test-writer, test-reviewer, compositor, bug-hunter)
- `modules/{name}/spec.md` — module specification
- `modules/{name}/src/` — Rust code (Cargo crate)
- `modules/{name}/ui/` — Vue/TS code
- `integrations/src/` — shared Rust types (backend/cross-service), documented via `///` comments
- `integrations/ui/` — shared TypeScript contracts (UI events, cross-module DTOs), documented via JSDoc
- `backend/` — thin Axum shell composing modules
- `frontend/` — thin Vite shell composing module UIs and owning flows (which module hands off to which)

## Cross-module contracts

If two modules need to communicate (events, shared DTOs, flow state), the contract lives in `integrations/` — never in another module's spec. Each module's spec references the type by name. Renaming a field in `integrations/` forces both sides to update at once; that is the synchronization mechanism.

UI flows (the order in which modules appear, how outputs feed inputs) live in `frontend/`, not in module specs. A module stays unaware of every other module.
