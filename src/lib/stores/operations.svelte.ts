export interface OperationProgress {
  current: number;
  total: number;
  detail?: string;
}

export interface Operation {
  id: string;
  label: string;
  status: "running" | "done" | "error";
  progress?: OperationProgress;
  result?: string;
  startedAt: number;
  oncancel?: () => void;
}

const MAX_OPS = 5;
const DONE_TTL_MS = 5000;

let ops = $state<Operation[]>([]);

function dismiss(id: string) {
  ops = ops.filter((o) => o.id !== id);
}

export const operationsStore = {
  get list() {
    return ops;
  },
  get hasAny() {
    return ops.length > 0;
  },
  get hasRunning() {
    return ops.some((o) => o.status === "running");
  },

  add(label: string, options?: { oncancel?: () => void }): string {
    const id = crypto.randomUUID();
    ops = [
      { id, label, status: "running", startedAt: Date.now(), oncancel: options?.oncancel },
      ...ops,
    ].slice(0, MAX_OPS);
    return id;
  },

  updateProgress(id: string, progress: OperationProgress) {
    ops = ops.map((o) => (o.id === id ? { ...o, progress } : o));
  },

  complete(id: string, result: string) {
    ops = ops.map((o) =>
      o.id === id ? { ...o, status: "done" as const, result, oncancel: undefined } : o
    );
    setTimeout(() => dismiss(id), DONE_TTL_MS);
  },

  fail(id: string, message: string) {
    ops = ops.map((o) =>
      o.id === id ? { ...o, status: "error" as const, result: message, oncancel: undefined } : o
    );
  },

  dismiss,
};
