<script setup lang="ts">
import { onBeforeUnmount, watch } from "vue";
import type { Toast } from "../../integrations/ui/notifications";

type ToastWithId = Toast & { id: number };

const props = defineProps<{
  toasts: ToastWithId[];
}>();

const emit = defineEmits<{
  dismiss: [id: number];
}>();

const timers = new Map<number, ReturnType<typeof setTimeout>>();

function scheduleAutoDismiss(toast: ToastWithId) {
  const duration = toast.durationMs ?? 5000;
  if (duration === 0) return;
  const handle = setTimeout(() => {
    timers.delete(toast.id);
    emit("dismiss", toast.id);
  }, duration);
  timers.set(toast.id, handle);
}

function clearTimer(id: number) {
  const handle = timers.get(id);
  if (handle !== undefined) {
    clearTimeout(handle);
    timers.delete(id);
  }
}

function onClose(id: number) {
  clearTimer(id);
  emit("dismiss", id);
}

// Sync timers with the current toast list. New toasts get a timer scheduled;
// toasts that disappear have their timers cancelled.
watch(
  () => props.toasts.map((t) => t.id),
  (ids, prevIds) => {
    const prev = new Set(prevIds ?? []);
    const curr = new Set(ids);
    for (const id of prev) {
      if (!curr.has(id)) clearTimer(id);
    }
    for (const toast of props.toasts) {
      if (!prev.has(toast.id) && !timers.has(toast.id)) {
        scheduleAutoDismiss(toast);
      }
    }
  },
  { immediate: true, flush: "post" },
);

onBeforeUnmount(() => {
  for (const handle of timers.values()) clearTimeout(handle);
  timers.clear();
});

function iconFor(level: Toast["level"]): string {
  switch (level) {
    case "success":
      return "\u2713"; // ✓
    case "error":
      return "\u2715"; // ✕
    case "info":
    default:
      return "\u2139"; // ℹ
  }
}
</script>

<template>
  <div class="toast-container" role="region" aria-label="Notifications">
    <div
      v-for="toast in toasts"
      :key="toast.id"
      class="toast"
      :class="`toast--${toast.level}`"
      role="status"
    >
      <span class="toast__icon" aria-hidden="true">{{ iconFor(toast.level) }}</span>
      <span class="toast__message">{{ toast.message }}</span>
      <button
        type="button"
        class="toast__close"
        aria-label="Dismiss notification"
        @click="onClose(toast.id)"
      >
        &times;
      </button>
    </div>
  </div>
</template>

<style scoped>
.toast-container {
  position: fixed;
  bottom: 1rem;
  right: 1rem;
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
  z-index: 1000;
  pointer-events: none;
  max-width: min(360px, calc(100vw - 2rem));
}

.toast {
  pointer-events: auto;
  display: flex;
  align-items: flex-start;
  gap: 0.5rem;
  padding: 0.75rem 0.875rem;
  border-radius: 6px;
  background: #fff;
  color: #1a1a1a;
  border-left: 4px solid #888;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.12);
  font-size: 0.9rem;
  line-height: 1.35;
}

.toast--success {
  border-left-color: #2e7d32;
}

.toast--success .toast__icon {
  color: #2e7d32;
}

.toast--error {
  border-left-color: #c62828;
}

.toast--error .toast__icon {
  color: #c62828;
}

.toast--info {
  border-left-color: #1565c0;
}

.toast--info .toast__icon {
  color: #1565c0;
}

.toast__icon {
  flex: 0 0 auto;
  font-weight: bold;
  line-height: 1.35;
}

.toast__message {
  flex: 1 1 auto;
  white-space: pre-wrap;
  word-break: break-word;
}

.toast__close {
  flex: 0 0 auto;
  background: transparent;
  border: none;
  color: #666;
  cursor: pointer;
  font-size: 1.1rem;
  line-height: 1;
  padding: 0 0.25rem;
}

.toast__close:hover {
  color: #1a1a1a;
}
</style>
