export type ToastType = 'success' | 'error' | 'warning' | 'info';

export interface Toast {
  id: number;
  type: ToastType;
  message: string;
}

const DEFAULT_DURATION: Record<ToastType, number> = {
  success: 3500,
  info: 3500,
  warning: 5000,
  error: 6000,
};

let _toasts = $state<Toast[]>([]);
let _nextId = 0;

function add(type: ToastType, message: string, duration?: number): number {
  const id = _nextId++;
  _toasts = [..._toasts, { id, type, message }];
  setTimeout(() => dismiss(id), duration ?? DEFAULT_DURATION[type]);
  return id;
}

function dismiss(id: number): void {
  _toasts = _toasts.filter((t) => t.id !== id);
}

export const toast = {
  get items(): Toast[] {
    return _toasts;
  },
  success: (msg: string) => add('success', msg),
  error: (msg: string) => add('error', msg),
  warning: (msg: string) => add('warning', msg),
  info: (msg: string) => add('info', msg),
  dismiss,
};
