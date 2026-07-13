import { useEffect, useMemo, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { ArrowUp, ArrowDown, X, Download } from "lucide-react";
import { toast } from "sonner";
import { api, withToast } from "../../lib/api";
import type { DmrExportPreview } from "../../lib/types";
import { Modal } from "../overlays";
import { Button, Spinner, TextInput, Select } from "../ui";

function fmtCount(n: number) {
  return n.toLocaleString();
}

export function ExportDmrUsersDialog({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const [countries, setCountries] = useState<string[]>([]);
  const [continents, setContinents] = useState<string[]>([]);
  const [priority, setPriority] = useState<string[]>([]);
  const [addChoice, setAddChoice] = useState("");
  const [maxCountText, setMaxCountText] = useState("");
  const [preview, setPreview] = useState<DmrExportPreview | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [exporting, setExporting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setPriority([]);
    setAddChoice("");
    setMaxCountText("");
    setPreview(null);
    api.listDmrUserCountries().then(setCountries);
    api.listDmrContinents().then(setContinents);
  }, [open]);

  const maxCount = useMemo(() => {
    const n = parseInt(maxCountText, 10);
    return Number.isFinite(n) && n > 0 ? n : null;
  }, [maxCountText]);

  useEffect(() => {
    if (!open) return;
    setPreviewing(true);
    const t = setTimeout(() => {
      api
        .previewDmrExport(maxCount, priority)
        .then(setPreview)
        .finally(() => setPreviewing(false));
    }, 200);
    return () => clearTimeout(t);
  }, [open, maxCount, priority]);

  // Options not already chosen, continents first so they're easy to spot.
  const availableToAdd = useMemo(() => {
    const chosen = new Set(priority.map((p) => p.toLowerCase()));
    const c = continents.filter((c) => !chosen.has(c.toLowerCase()));
    const k = countries.filter((c) => !chosen.has(c.toLowerCase()));
    return { continents: c, countries: k };
  }, [continents, countries, priority]);

  const addPriority = () => {
    if (!addChoice) return;
    setPriority((p) => [...p, addChoice]);
    setAddChoice("");
  };
  const removePriority = (i: number) =>
    setPriority((p) => p.filter((_, idx) => idx !== i));
  const movePriority = (i: number, dir: -1 | 1) =>
    setPriority((p) => {
      const j = i + dir;
      if (j < 0 || j >= p.length) return p;
      const next = [...p];
      [next[i], next[j]] = [next[j], next[i]];
      return next;
    });

  const doExport = async () => {
    const path = await save({
      defaultPath: "dmr_contacts.csv",
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    if (!path) return;
    setExporting(true);
    const res = await withToast(api.exportDmrUsers(path, maxCount, priority), {
      error: "Export failed",
    });
    setExporting(false);
    if (res) {
      toast.success(`Exported ${fmtCount(res.written)} contacts`);
      onClose();
    }
  };

  return (
    <Modal open={open} onClose={onClose} title="Export DMR contacts" width="max-w-lg">
      <div className="flex flex-col gap-4 p-4">
        <label className="flex flex-col gap-1">
          <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Max contacts (blank = no limit)
          </span>
          <TextInput
            inputMode="numeric"
            placeholder="e.g. 50000 for a radio with limited capacity"
            value={maxCountText}
            onChange={(e) => setMaxCountText(e.target.value.replace(/[^0-9]/g, ""))}
          />
        </label>

        <div className="flex flex-col gap-1.5">
          <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Priority order (matched first, in this order; everyone else fills any remaining space)
          </span>
          <div className="flex gap-1.5">
            <Select
              className="flex-1"
              value={addChoice}
              onChange={(e) => setAddChoice(e.target.value)}
            >
              <option value="">Add a country or continent…</option>
              {availableToAdd.continents.length > 0 && (
                <optgroup label="Continents">
                  {availableToAdd.continents.map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </optgroup>
              )}
              {availableToAdd.countries.length > 0 && (
                <optgroup label="Countries">
                  {availableToAdd.countries.map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </optgroup>
              )}
            </Select>
            <Button onClick={addPriority} disabled={!addChoice}>
              Add
            </Button>
          </div>

          {priority.length > 0 && (
            <ul className="flex flex-col gap-1 rounded-md border border-slate-200 p-1.5 dark:border-slate-700">
              {priority.map((p, i) => (
                <li
                  key={p}
                  className="flex items-center justify-between gap-2 rounded bg-slate-50 px-2 py-1 text-xs dark:bg-slate-800"
                >
                  <span className="text-slate-500 tabular-nums">{i + 1}.</span>
                  <span className="flex-1 text-slate-700 dark:text-slate-200">{p}</span>
                  <div className="flex gap-1">
                    <button
                      className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-600 disabled:opacity-30 dark:hover:bg-slate-700"
                      onClick={() => movePriority(i, -1)}
                      disabled={i === 0}
                      title="Move up"
                    >
                      <ArrowUp size={12} />
                    </button>
                    <button
                      className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-600 disabled:opacity-30 dark:hover:bg-slate-700"
                      onClick={() => movePriority(i, 1)}
                      disabled={i === priority.length - 1}
                      title="Move down"
                    >
                      <ArrowDown size={12} />
                    </button>
                    <button
                      className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-rose-600 dark:hover:bg-slate-700"
                      onClick={() => removePriority(i)}
                      title="Remove"
                    >
                      <X size={12} />
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="rounded-md border border-slate-200 bg-slate-50 p-3 text-xs dark:border-slate-700 dark:bg-slate-800/60">
          {previewing || !preview ? (
            <div className="flex items-center gap-2 text-slate-400">
              <Spinner className="h-3.5 w-3.5" /> Calculating…
            </div>
          ) : (
            <>
              <div className="font-medium text-slate-700 dark:text-slate-200">
                {fmtCount(preview.will_export)} of {fmtCount(preview.total_available)} contacts will
                be exported
              </div>
              {preview.by_country.length > 0 && (
                <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 text-slate-500">
                  {preview.by_country.slice(0, 8).map((b) => (
                    <span key={b.country}>
                      {b.country}: {fmtCount(b.count)}
                    </span>
                  ))}
                  {preview.by_country.length > 8 && <span>…</span>}
                </div>
              )}
            </>
          )}
        </div>
      </div>
      <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" onClick={doExport} disabled={exporting || !preview}>
          {exporting ? <Spinner className="h-3.5 w-3.5" /> : <Download size={14} />} Export CSV
        </Button>
      </div>
    </Modal>
  );
}
