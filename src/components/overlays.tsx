import { useEffect, type ReactNode } from "react";
import { X } from "lucide-react";
import clsx from "clsx";
import { Button } from "./ui";

/**
 * Close on Escape, but only while `enabled`.
 *
 * ⚠ The guard is not an optimisation. This used to register unconditionally,
 * ABOVE each component's `if (!open) return null`, so every mounted-but-closed
 * overlay in the tree also had a live listener — and, more seriously, an
 * overlay had no way to refuse dismissal. A program dialog dismissed mid-write
 * unmounts while the Tauri command runs on to completion: the radio is still
 * being written, and the operator is looking at an ordinary page with no
 * spinner, no "keep the radio on" warning, and no backup path. (#65)
 */
function useEscape(onClose: () => void, enabled: boolean) {
  useEffect(() => {
    if (!enabled) return;
    const h = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose, enabled]);
}

/// Props every overlay shares for refusing to be dismissed.
interface Dismissal {
  /// `false` closes off all three exits the overlay itself owns — Escape, the
  /// backdrop and the ✕. Default `true`. Set it from the dialog's own busy
  /// state, never unconditionally: an overlay nobody can leave is a trap if the
  /// work it is waiting on never finishes.
  ///
  /// ⚠ It cannot reach a Close button the dialog draws in its OWN footer. Every
  /// program dialog had one wired straight to `onClose`, and it stayed live
  /// through a write — found on a real TD-H3, restoring, by clicking it. Use
  /// [`FooterClose`] for those, with the same flag.
  dismissible?: boolean;
  /// Why it cannot be dismissed, shown on the disabled ✕. The overlay does not
  /// know what it is running, so the caller says it.
  lockedHint?: string;
}

// Centered modal dialog.
export function Modal({
  open,
  onClose,
  title,
  children,
  width = "max-w-3xl",
  dismissible = true,
  lockedHint,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  width?: string;
} & Dismissal) {
  useEscape(onClose, open && dismissible);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div
        className="absolute inset-0 bg-black/40"
        onClick={dismissible ? onClose : undefined}
      />
      <div
        className={clsx(
          "relative flex max-h-[85vh] w-full flex-col overflow-hidden rounded-lg border border-slate-200 bg-white shadow-xl dark:border-slate-700 dark:bg-slate-800",
          width,
        )}
      >
        <div className="flex items-center justify-between border-b border-slate-200 px-4 py-2.5 dark:border-slate-700">
          <h2 className="text-sm font-semibold text-slate-800 dark:text-slate-100">
            {title}
          </h2>
          <CloseButton
            onClose={onClose}
            dismissible={dismissible}
            lockedHint={lockedHint}
          />
        </div>
        <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
          {children}
        </div>
      </div>
    </div>
  );
}

// Right-side slide-over panel.
export function SlideOver({
  open,
  onClose,
  title,
  subtitle,
  children,
  footer,
  dismissible = true,
  lockedHint,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  subtitle?: string;
  children: ReactNode;
  footer?: ReactNode;
} & Dismissal) {
  useEscape(onClose, open && dismissible);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50">
      <div
        className="absolute inset-0 bg-black/30"
        onClick={dismissible ? onClose : undefined}
      />
      <div className="absolute right-0 top-0 flex h-full w-[520px] max-w-[90vw] flex-col border-l border-slate-200 bg-white shadow-2xl dark:border-slate-700 dark:bg-slate-800">
        <div className="flex items-start justify-between border-b border-slate-200 px-4 py-3 dark:border-slate-700">
          <div className="min-w-0">
            <h2 className="truncate text-sm font-semibold text-slate-800 dark:text-slate-100">
              {title}
            </h2>
            {subtitle && (
              <p className="truncate text-xs text-slate-500 dark:text-slate-400">
                {subtitle}
              </p>
            )}
          </div>
          <CloseButton
            onClose={onClose}
            dismissible={dismissible}
            lockedHint={lockedHint}
          />
        </div>
        <div className="flex-1 overflow-auto p-4">{children}</div>
        {footer && (
          <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
            {footer}
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * A dialog's own footer Close button, refusing the same way the ✕ does.
 *
 * The overlay can gate Escape, the backdrop and its ✕, and nothing more — a
 * button the dialog renders inside `children` is out of its reach. All three
 * program dialogs drew one calling `onClose` unconditionally, so the widest
 * target on the screen was the one exit #65 left open: clicking it mid-restore
 * unmounted the dialog while the TD-H3 was still being written, and the write
 * announced itself into an ordinary page.
 *
 * The disabled button is wrapped rather than given a `title`, because `Button`
 * carries `disabled:pointer-events-none` — the tooltip on the control itself
 * would never fire.
 */
export function FooterClose({
  onClose,
  dismissible = true,
  lockedHint,
}: { onClose: () => void } & Dismissal) {
  const button = (
    <Button variant="ghost" onClick={onClose} disabled={!dismissible}>
      Close
    </Button>
  );
  return dismissible ? (
    button
  ) : (
    <span title={lockedHint} aria-label={lockedHint} className="cursor-not-allowed">
      {button}
    </span>
  );
}

/// The ✕, which stays VISIBLE but disabled while an overlay refuses dismissal —
/// a control that vanishes reads as a rendering bug, where a dead one with a
/// reason on it reads as "not yet".
function CloseButton({
  onClose,
  dismissible,
  lockedHint,
}: { onClose: () => void } & Required<Pick<Dismissal, "dismissible">> &
  Pick<Dismissal, "lockedHint">) {
  return (
    <button
      onClick={dismissible ? onClose : undefined}
      disabled={!dismissible}
      title={dismissible ? undefined : lockedHint}
      aria-label={dismissible ? "Close" : lockedHint}
      className={clsx(
        "rounded p-1 text-slate-400",
        dismissible
          ? "hover:bg-slate-100 hover:text-slate-600 dark:hover:bg-slate-700"
          : "cursor-not-allowed opacity-40",
      )}
    >
      <X size={16} />
    </button>
  );
}
