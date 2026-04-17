<script setup lang="ts">
import type { DryRunResult } from "./dryRun";

defineProps<{
  result: DryRunResult;
}>();
</script>

<template>
  <div class="dry-run-results">
    <table class="results-table">
      <thead>
        <tr>
          <th>Input</th>
          <th>stdout</th>
          <th>stderr</th>
          <th>Duration (ms)</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="(row, idx) in result.rows" :key="idx">
          <td>
            <pre class="cell-pre">{{ row.input }}</pre>
          </td>
          <td>
            <pre class="cell-pre">{{ row.stdout }}</pre>
          </td>
          <td>
            <pre class="cell-pre">{{ row.stderr }}</pre>
          </td>
          <td class="cell-duration">{{ row.durationMs.toFixed(2) }}</td>
        </tr>
      </tbody>
      <tfoot>
        <tr>
          <td colspan="3" class="footer-label">Total duration</td>
          <td class="cell-duration">{{ result.totalDurationMs.toFixed(2) }} ms</td>
        </tr>
      </tfoot>
    </table>
  </div>
</template>

<style scoped>
.dry-run-results {
  width: 100%;
  overflow-x: auto;
}

.results-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.85rem;
}

.results-table th,
.results-table td {
  border: 1px solid #e5e7eb;
  padding: 0.5rem;
  vertical-align: top;
  text-align: left;
}

.results-table th {
  background: #f3f4f6;
  font-weight: 600;
  color: #374151;
}

.cell-pre {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.8rem;
  color: #1f2937;
}

.cell-duration {
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.footer-label {
  text-align: right;
  font-weight: 600;
  color: #374151;
}
</style>
