# Technical Reference

## Script Contract

- Input: stdin (CSV rows or CLI args for parametric sweep)
- Output: stdout
- Runtime: Python via Pyodide (CPython compiled to WASM)
- Pre-installed packages: numpy, scipy, pandas, scikit-learn, matplotlib, networkx, sympy, statsmodels

## Input Formats (MVP: CSV only)

| Format | Splitting strategy |
|--------|-------------------|
| `.csv` / `.tsv` | Each row (or group of rows) = one task |
| `.zip` / `.tar.gz` | Each file = one task (post-MVP) |
| Parametric sweep | Each value/subrange = one task (post-MVP) |

## Architecture

```
Scientist (Web UI) → Orchestrator → Volunteer (Browser)
                     - Task queue     WASM worker in
                     - Splitter       Web Worker
                     - Aggregator
                     - Dry runner
                     - Verifier
```

## WASM Runtime

| Property | Value |
|----------|-------|
| Browser runtime | Pyodide in Web Worker |
| Sandbox | No filesystem, no network access |
| Memory limit | ~256 MB linear memory |
| Cross-platform | Single WASM binary, runs everywhere |

## Worker Limits (browser)

| Resource | Limit |
|----------|-------|
| Time per chunk | 30 seconds |
| Memory | 256 MB |
| Output size | 10 MB |
| Network | None |
| Filesystem | Sandbox only |

## Data Flow

1. Scientist uploads CSV + script
2. Dry run: execute script on first 3 rows in browser (Pyodide)
3. Show preview: input vs output side-by-side, time per row
4. Detect aggregation: output rows < input rows → warn
5. Scientist clicks "Process all"
6. Split CSV into chunks (N rows each)
7. Queue chunks as tasks
8. Volunteers poll for tasks, execute, submit results
9. Collect results, restore original row order
10. Result CSV ready for download

## Verification

Redundant computation: each chunk sent to 2 workers independently.
Results match → accepted. Results differ → 3rd worker (tiebreaker).
Majority result (2 of 3) wins. All 3 differ → job failed.

## Security

- WASM sandbox: no FS, no network
- Fuel metering: instruction count budget per chunk
- Behavioral analysis: high CPU + empty stdout = suspicious
- Data is sent to volunteers in plaintext (same as BOINC)

## MVP Scope

**In:** CSV + .py upload, Pyodide runtime, browser workers, CSV row splitting, dry run + preview, basic fuel metering
**Out:** native agent, Docker, R/WebR, ZIP input, parametric sweep, embeddable widget, reputation system
