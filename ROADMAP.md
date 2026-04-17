# Roadmap

## Phase 1: upload-and-preview
No dependencies. Foundation for everything.

## Phase 2: pyodide-runtime
No dependencies. Can be built in parallel with Phase 1. Includes its own UI (dry-run trigger + results preview).

## Phase 3: task-distribution
Depends on: upload-and-preview, integrations (Job, Task types)

## Phase 4: result-aggregation
Depends on: task-distribution

## Phase 5: progress-tracking
Depends on: task-distribution, result-aggregation
