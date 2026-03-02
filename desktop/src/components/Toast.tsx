import { CheckCircle, AlertTriangle, XCircle, Info, X } from "lucide-react";
import { useToastStore, type ToastVariant } from "../hooks/useToastStore";

const variantStyles: Record<ToastVariant, { border: string; icon: typeof Info }> = {
  success: { border: "var(--color-success)", icon: CheckCircle },
  warning: { border: "var(--color-warning)", icon: AlertTriangle },
  error: { border: "var(--color-error)", icon: XCircle },
  info: { border: "var(--color-accent)", icon: Info },
};

export default function ToastContainer() {
  const { toasts, removeToast } = useToastStore();

  if (toasts.length === 0) return null;

  return (
    <div className="fixed right-4 bottom-4 z-50 flex flex-col gap-2">
      {toasts.map((toast) => {
        const style = variantStyles[toast.variant];
        const Icon = style.icon;
        return (
          <div
            key={toast.id}
            className="flex items-center gap-2 rounded-lg border bg-[var(--color-bg-secondary)] px-4 py-3 text-sm shadow-lg"
            style={{ borderColor: style.border }}
          >
            <Icon size={16} style={{ color: style.border }} className="shrink-0" />
            <span className="flex-1">{toast.message}</span>
            <button
              onClick={() => removeToast(toast.id)}
              className="shrink-0 text-[var(--color-text-secondary)] hover:text-[var(--color-text)]"
            >
              <X size={14} />
            </button>
          </div>
        );
      })}
    </div>
  );
}
