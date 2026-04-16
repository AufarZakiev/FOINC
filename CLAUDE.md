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

### Step 7: Commit
Commit all changes.

The user reviews the commit. This is the only post-pipeline review point.

## Project Structure

- `constitution.md` — spec format and rules
- `ROADMAP.md` — module order and dependencies
- `TECHNICAL_PAPER.md` — architecture, constraints, data flow (read by architect)
- `WHITE_PAPER.md` — domain context (historical, not read by agents)
- `agents/` — agent prompts (architect, spec-writer, spec-reviewer, code-writer, code-reviewer)
- `modules/{name}/spec.md` — module specification
- `modules/{name}/src/` — Rust code (Cargo crate)
- `modules/{name}/ui/` — Vue/TS code
- `integrations/src/` — shared types crate, documented via `///` comments
- `backend/` — thin Axum shell composing modules
- `frontend/` — thin Vite shell composing module UIs
