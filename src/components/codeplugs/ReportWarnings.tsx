import { AlertTriangle } from "lucide-react";

/**
 * The things a write did differently from what was asked.
 *
 * `CodeplugProgramReport.warnings` is filled by the command layer for EVERY
 * driver — a settings value outside the range its schema declares is dropped
 * there and named here (#87) — but only the AnyTone dialog ever rendered the
 * field. On the UV-5R, the radio #87 was written for, all four band TX limits
 * were being dropped from a program and nothing said so. Any dialog that shows
 * a program report shows this.
 */
export function WarningList({ warnings }: { warnings: string[] }) {
  if (warnings.length === 0) return null;
  return (
    <div className="rounded border border-amber-300/70 bg-amber-50 p-2 text-[11px] dark:border-amber-800/60 dark:bg-amber-950/30">
      <div className="mb-1 flex items-center gap-1 font-semibold text-amber-800 dark:text-amber-300">
        <AlertTriangle size={12} /> Warnings
      </div>
      <ul className="list-inside list-disc space-y-0.5 text-amber-800 dark:text-amber-200">
        {warnings.map((w, i) => (
          <li key={i}>{w}</li>
        ))}
      </ul>
    </div>
  );
}
