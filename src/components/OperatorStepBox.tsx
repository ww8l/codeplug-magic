import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { RadioTower } from "lucide-react";
import { api } from "../lib/api";
import type { OperatorStep } from "../lib/types";
import { Button } from "./ui";

/**
 * What the operator must do at the radio right now, sent by the backend in the
 * middle of a radio session (`radio-operator-step`). Only the FT-857D/897 sends
 * any: its clone is started from the radio's own front panel, so a read asks
 * for SEND and a write waits — Continue/Cancel — for RCV, which the radio cannot
 * announce on its own.
 *
 * Render it wherever a radio command can be running; it draws nothing while
 * `active` is false or no step is pending.
 */
export function OperatorStepBox({ active, className }: { active: boolean; className?: string }) {
  const [step, setStep] = useState<OperatorStep | null>(null);
  useEffect(() => {
    const un = listen<OperatorStep>("radio-operator-step", (e) =>
      setStep(e.payload.text ? e.payload : null),
    );
    return () => {
      void un.then((f) => f());
    };
  }, []);
  useEffect(() => {
    if (!active) setStep(null);
  }, [active]);

  if (!active || !step?.text) return null;
  return (
    <div
      className={
        "rounded-md border border-sky-300 bg-sky-50 px-3 py-2.5 text-sm text-sky-900 dark:border-sky-800 dark:bg-sky-950/40 dark:text-sky-100 " +
        (className ?? "")
      }
    >
      <div className="flex items-start gap-2">
        <RadioTower size={16} className="mt-0.5 shrink-0" />
        <span className="font-medium">{step.text}</span>
      </div>
      {step.needs_continue && (
        <div className="mt-2.5 flex justify-end gap-2">
          <Button variant="ghost" onClick={() => void api.operatorAnswer(false)}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void api.operatorAnswer(true)}>
            Continue
          </Button>
        </div>
      )}
    </div>
  );
}
