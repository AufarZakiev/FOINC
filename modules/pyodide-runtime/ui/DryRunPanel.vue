<script setup lang="ts">
import { ref } from "vue";
import DryRunForm from "./DryRunForm.vue";
import DryRunResults from "./DryRunResults.vue";
import { dryRun, type DryRunResult } from "./dryRun";

const loading = ref(false);
const error = ref<string | null>(null);
const result = ref<DryRunResult | null>(null);

async function handleSubmit(payload: {
  script: string;
  csvRows: string[];
}) {
  loading.value = true;
  error.value = null;
  result.value = null;

  try {
    result.value = await dryRun(payload.script, payload.csvRows);
  } catch (e: unknown) {
    console.error("[DryRunPanel] dryRun failed:", e);
    const msg = e instanceof Error ? e.message : String(e);
    error.value = msg && msg.trim().length > 0 ? msg : "Dry run failed (no error message)";
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <section class="dry-run-panel">
    <h2 class="dry-run-panel__title">Dry run</h2>
    <DryRunForm @submit="handleSubmit" />

    <p v-if="loading" class="status status--loading">Running dry run...</p>
    <p v-else-if="error" class="status status--error">{{ error }}</p>
    <DryRunResults v-else-if="result" :result="result" />
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
</style>
