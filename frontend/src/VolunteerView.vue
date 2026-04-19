<script setup lang="ts">
import { ref } from "vue";
import VolunteerRunner from "../../modules/task-distribution/ui/VolunteerRunner.vue";
import type { Toast } from "../../integrations/ui/notifications";
import ToastContainer from "./ToastContainer.vue";

type ToastWithId = Toast & { id: number };

const toasts = ref<ToastWithId[]>([]);

let nextToastId = 1;

function pushToast(t: Toast) {
  toasts.value.push({ ...t, id: nextToastId++ });
}

function removeToast(id: number) {
  toasts.value = toasts.value.filter((t) => t.id !== id);
}
</script>

<template>
  <div class="volunteer">
    <header class="volunteer-header">
      <h1>FOINC &middot; Volunteer</h1>
      <p>Help run queued tasks from your browser.</p>
      <a class="nav-link" href="#/">&larr; Back to Scientist</a>
    </header>

    <main class="volunteer-main">
      <VolunteerRunner @notify="pushToast" />
    </main>

    <ToastContainer :toasts="toasts" @dismiss="removeToast" />
  </div>
</template>

<style scoped>
.volunteer {
  max-width: 720px;
  margin: 0 auto;
  padding: 2rem 1rem;
  font-family: system-ui, -apple-system, sans-serif;
  color: #1a1a1a;
}

.volunteer-header {
  text-align: center;
  margin-bottom: 1.5rem;
}

.volunteer-header h1 {
  margin: 0 0 0.25rem;
  font-size: 1.75rem;
}

.volunteer-header p {
  margin: 0 0 0.5rem;
  color: #666;
  font-size: 0.95rem;
}

.nav-link {
  display: inline-block;
  margin-top: 0.25rem;
  font-size: 0.875rem;
  color: #1565c0;
  text-decoration: none;
}

.nav-link:hover {
  text-decoration: underline;
}

.volunteer-main {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 1.5rem;
}
</style>
