import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface OperationProgress {
  current: number;
  total: number;
  detail?: string;
  fileBytesDone?: number;
  fileBytesTotal?: number;
  filePhase?: string;
  filePhaseDone?: number;
  filePhaseTotal?: number;
}

export interface FileLogEntry {
  name: string;
  status: "active" | "done";
}

export interface Operation {
  id: string;
  label: string;
  status: "pending" | "running" | "done" | "error";
  progress?: OperationProgress;
  result?: string;
  startedAt: number;
  cancelling?: boolean;
  files: FileLogEntry[];
  /** Files not yet started, received via op:pending_files event. */
  pendingFiles: string[];
}

interface QueueEntry {
  id: string;
  label: string;
  op_type: string;
}

interface QueueSnapshot {
  pending: QueueEntry[];
  active: QueueEntry | null;
}

const DONE_TTL_MS = 5000;
// Maximum completed files kept in memory per op (oldest dropped).
const MAX_FILE_LOG = 500;

let ops = $state<Operation[]>([]);

function dismiss(id: string) {
  ops = ops.filter((o) => o.id !== id);
}

/**
 * Shared helper called by every progress event. Updates the progress bar,
 * accumulates the file log, and removes the current file from pendingFiles.
 */
function applyProgress(
  op_id: string,
  processed: number,
  total: number,
  current_file: string
) {
  const name = current_file.split("/").at(-1) ?? current_file;
  ops = ops.map((o) => {
    if (o.id !== op_id) return o;

    // Remove this file from the pending list. Match on the full current_file
    // value (which may be an absolute path) so that two files in different
    // directories with the same basename are treated independently.
    const pendingIdx = o.pendingFiles.indexOf(current_file);
    const newPendingFiles =
      pendingIdx >= 0
        ? [...o.pendingFiles.slice(0, pendingIdx), ...o.pendingFiles.slice(pendingIdx + 1)]
        : o.pendingFiles;

    // Flip previous active → done, then append the new active entry.
    const prev = o.files.map((f) =>
      f.status === "active" ? { ...f, status: "done" as const } : f
    );
    // Drop oldest done entries if we're over the cap.
    const done = prev.filter((f) => f.status === "done");
    const trimmed =
      done.length >= MAX_FILE_LOG
        ? prev.slice(prev.length - MAX_FILE_LOG)
        : prev;

    return {
      ...o,
      progress: { current: processed, total, detail: name },
      files: [...trimmed, { name, status: "active" as const }],
      pendingFiles: newPendingFiles,
    };
  });
}

// Wire up backend event listeners for the lifetime of the app.
(async () => {
  await listen<QueueSnapshot>("queue:updated", (event) => {
    const { pending, active } = event.payload;

    const knownIds = new Set([
      ...pending.map((e) => e.id),
      ...(active ? [active.id] : []),
    ]);

    // Drop pending ops that were cancelled before they started.
    ops = ops.filter((o) => o.status !== "pending" || knownIds.has(o.id));

    // Add any pending ops not yet in the list.
    for (const entry of pending) {
      if (!ops.find((o) => o.id === entry.id)) {
        ops = [
          ...ops,
          {
            id: entry.id,
            label: entry.label,
            status: "pending",
            startedAt: Date.now(),
            files: [],
            pendingFiles: [],
          },
        ];
      }
    }

    // Transition active op from pending → running.
    if (active) {
      const existing = ops.find((o) => o.id === active.id);
      if (!existing) {
        ops = [
          ...ops,
          {
            id: active.id,
            label: active.label,
            status: "running",
            startedAt: Date.now(),
            files: [],
            pendingFiles: [],
          },
        ];
      } else if (existing.status === "pending") {
        ops = ops.map((o) =>
          o.id === active.id ? { ...o, status: "running" as const } : o
        );
      }
    }
  });

  await listen<{ op_id: string; files: string[] }>("op:pending_files", (event) => {
    const { op_id, files } = event.payload;
    ops = ops.map((o) => (o.id === op_id ? { ...o, pendingFiles: files } : o));
  });

  await listen<{ op_id: string; bytes_done: number; bytes_total: number; phase: string; phase_done: number; phase_total: number }>(
    "upload:progress",
    (event) => {
      const { op_id, bytes_done, bytes_total, phase, phase_done, phase_total } = event.payload;
      ops = ops.map((o) => {
        if (o.id !== op_id) return o;
        return {
          ...o,
          progress: {
            ...(o.progress ?? { current: 0, total: 1 }),
            fileBytesDone: bytes_done,
            fileBytesTotal: bytes_total,
            filePhase: phase,
            filePhaseDone: phase_done,
            filePhaseTotal: phase_total,
          },
        };
      });
    }
  );

  await listen<{ id: string; message: string }>("op:complete", (event) => {
    const { id, message } = event.payload;
    ops = ops.map((o) =>
      o.id === id
        ? {
            ...o,
            status: "done" as const,
            result: message,
            progress: undefined,
            pendingFiles: [],
            // Mark any remaining active file as done.
            files: o.files.map((f) =>
              f.status === "active" ? { ...f, status: "done" as const } : f
            ),
          }
        : o
    );
    setTimeout(() => dismiss(id), DONE_TTL_MS);
  });

  await listen<{ id: string; error: string }>("op:failed", (event) => {
    const { id, error } = event.payload;
    ops = ops.map((o) =>
      o.id === id
        ? { ...o, status: "error" as const, result: error, progress: undefined, pendingFiles: [] }
        : o
    );
  });

  await listen<{ op_id: string; processed: number; total: number; current_file: string }>(
    "backup:progress",
    (event) => {
      const { op_id, processed, total, current_file } = event.payload;
      applyProgress(op_id, processed, total, current_file);
    }
  );

  await listen<{ op_id: string; processed: number; total: number; current_file: string }>(
    "restore:progress",
    (event) => {
      const { op_id, processed, total, current_file } = event.payload;
      applyProgress(op_id, processed, total, current_file);
    }
  );

  await listen<{ op_id: string; processed: number; total: number; current_file: string }>(
    "scramble:progress",
    (event) => {
      const { op_id, processed, total, current_file } = event.payload;
      applyProgress(op_id, processed, total, current_file);
    }
  );

  await listen<{ op_id: string; processed: number; total: number; current_file: string }>(
    "verify:progress",
    (event) => {
      const { op_id, processed, total, current_file } = event.payload;
      applyProgress(op_id, processed, total, current_file);
    }
  );

  await listen<{
    op_id: string;
    processed: number;
    total: number;
    current_item: string;
    deleted: number;
    failed: number;
  }>("cleanup:progress", (event) => {
    const { op_id, processed, total, current_item } = event.payload;
    applyProgress(op_id, processed, total, current_item);
  });
})();

export const operationsStore = {
  get list() {
    return ops;
  },
  get hasAny() {
    return ops.length > 0;
  },
  get hasRunning() {
    return ops.some((o) => o.status === "running" || o.status === "pending");
  },

  cancel(id: string) {
    const op = ops.find((o) => o.id === id);
    if (op?.status === "running") {
      ops = ops.map((o) => (o.id === id ? { ...o, cancelling: true } : o));
    } else if (op?.status === "pending") {
      // Optimistically drop the pending op immediately; the backend will confirm
      // via queue:updated but this makes the cancel feel instant.
      ops = ops.filter((o) => o.id !== id);
    }
    invoke("cancel_operation", { opId: id });
  },

  dismiss,
};
