import { create } from "zustand";

export type ToastVariant = "success" | "warning" | "error" | "info";

export interface Toast {
  id: number;
  variant: ToastVariant;
  message: string;
}

interface ToastState {
  toasts: Toast[];
  addToast: (variant: ToastVariant, message: string, duration?: number) => void;
  removeToast: (id: number) => void;
}

let nextId = 0;

export const useToastStore = create<ToastState>((set) => ({
  toasts: [],

  addToast: (variant, message, duration = 4000) => {
    const id = nextId++;
    set((state) => ({
      toasts: [...state.toasts, { id, variant, message }],
    }));
    setTimeout(() => {
      set((state) => ({
        toasts: state.toasts.filter((t) => t.id !== id),
      }));
    }, duration);
  },

  removeToast: (id) => {
    set((state) => ({
      toasts: state.toasts.filter((t) => t.id !== id),
    }));
  },
}));
