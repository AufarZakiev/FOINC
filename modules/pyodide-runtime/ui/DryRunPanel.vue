<script setup lang="ts">
import { ref, computed, watch } from "vue";
import type { UploadCompleted } from "../../../integrations/ui/events";
import type { Toast } from "../../../integrations/ui/notifications";
import DryRunResults from "./DryRunResults.vue";
import { dryRun, type DryRunResult } from "./dryRun";

const props = defineProps<{
  upload: UploadCompleted;
}>();

const emit = defineEmits<{
  notify: [payload: Toast];
}>();

const loading = ref(false);
const result = ref<DryRunResult | null>(null);

/**
 * Extracts the last non-empty line of a traceback string. The worker's
 * `error.message` is a full Python traceback, which is unusable in a
 * toast — the tail line (e.g. "RuntimeError: network access is disabled")
 * is what the user needs.
 *
 * Algorithm (per spec §DryRunPanel.vue data flow step 5):
 *   - split on "\n"
 *   - drop trailing empty strings
 *   - take the last element
 *   - if the result is empty, fall back to the original message
 */
function tail(message: string): string {
  const lines = message.split("\n");
  while (lines.length > 0 && lines[lines.length - 1].length === 0) {
    lines.pop();
  }
  const last = lines.length > 0 ? lines[lines.length - 1] : "";
  return last.length > 0 ? last : message;
}

/**
 * Parse `upload.csv` into up to 3 data rows:
 *  - split on "\n"
 *  - trim each line
 *  - discard lines empty after trimming
 *  - drop the first remaining line (CSV header)
 *  - take up to 3 of the remaining lines
 */
const csvRows = computed<string[]>(() => {
  const lines = props.upload.csv
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
  if (lines.length <= 1) return [];
  return lines.slice(1, 4);
});

const hasNoDataRows = computed(() => csvRows.value.length === 0);

watch(
  () => props.upload,
  () => {
    result.value = null;
  },
);

async function handleRun() {
  if (hasNoDataRows.value) return;
  loading.value = true;
  result.value = null;

  try {
    result.value = await dryRun(props.upload.script, csvRows.value);
  } catch (e: unknown) {
    emit("notify", { level: "error", message: tail((e as Error).message) });
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <section class="dry-run-panel">
    <h2 class="dry-run-panel__title">Dry run</h2>

    <p v-if="hasNoDataRows" class="status status--error">
      CSV must contain at least 1 data row
    </p>
    <p v-else-if="loading" class="status status--loading">Running dry run...</p>
    <DryRunResults v-else-if="result" :result="result" />
    <div v-else class="actions">
      <button type="button" class="btn btn--primary" @click="handleRun">
        Run dry run
      </button>
    </div>
  </section>
</template>

<style scoped>
.dry-run-panel {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.dry-run-panel__title {
  margin: 0;
  font-size: 1.25rem;
  color: #1a1a1a;
}

.status {
  margin: 0;
  padding: 0.6rem 0.85rem;
  border-radius: 6px;
  font-size: 0.9rem;
}

.status--loading {
  background: #eff6ff;
  color: #1d4ed8;
}

.status--error {
  background: #fef2f2;
  color: #b91c1c;
}

.actions {
  display: flex;
  gap: 0.75rem;
}

.btn {
  padding: 0.55rem 1.25rem;
  border: none;
  border-radius: 6px;
  font-size: 0.95rem;
  cursor: pointer;
  transition: background-color 0.15s;
}

.btn--primary {
  background: #3b82f6;
  color: #fff;
}

.btn--primary:hover {
  background: #2563eb;
}
</style>
