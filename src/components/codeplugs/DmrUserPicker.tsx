import { useEffect, useMemo, useState, type Dispatch, type SetStateAction } from "react";
import { ArrowUp, ArrowDown, X } from "lucide-react";
import { api } from "../../lib/api";
import type { DmrExportPreview } from "../../lib/types";
import { Button, Spinner, Select, TextInput } from "../ui";

/// Which users from the local DMR Contacts library a write takes: a size cap and
/// a country/continent priority, previewed live. Shared by every place that
/// sends library users to a radio (the AnyTone call-sign DB, the MD-380 user DB
/// and the MD-380's spare codeplug contacts) so the three choose the same way.
export type DmrUserPick = {
  maxCount: number | null;
  priority: string[];
  preview: DmrExportPreview | null;
  previewing: boolean;
  maxText: string;
  setMaxText: (s: string) => void;
  setPriority: Dispatch<SetStateAction<string[]>>;
  countries: string[];
  continents: string[];
};

/// `open` resets the choice each time the dialog opens; the preview runs only
/// while `active` — the card holding the picker is the selected one. `cap` is a
/// hard limit the radio imposes (the MD-380 codeplug's 1000 contacts): a blank
/// or larger max is previewed and sent as the cap.
export function useDmrUserPick(open: boolean, active: boolean, cap: number | null = null): DmrUserPick {
  const [countries, setCountries] = useState<string[]>([]);
  const [continents, setContinents] = useState<string[]>([]);
  const [priority, setPriority] = useState<string[]>([]);
  const [maxText, setMaxText] = useState("");
  const [preview, setPreview] = useState<DmrExportPreview | null>(null);
  const [previewing, setPreviewing] = useState(false);

  const maxCount = useMemo(() => {
    const n = parseInt(maxText, 10);
    const typed = Number.isFinite(n) && n > 0 ? n : null;
    if (cap == null) return typed;
    return typed == null ? cap : Math.min(typed, cap);
  }, [maxText, cap]);

  // The pickable lists load the first time the picker is actually shown, not
  // on every open of a dialog that may never show it (a radio with no DMR).
  const [listsLoaded, setListsLoaded] = useState(false);
  useEffect(() => {
    if (!open) return;
    setPriority([]);
    setMaxText("");
    setPreview(null);
  }, [open]);
  useEffect(() => {
    if (!open || !active || listsLoaded) return;
    setListsLoaded(true);
    api.listDmrUserCountries().then(setCountries).catch(() => {});
    api.listDmrContinents().then(setContinents).catch(() => {});
  }, [open, active, listsLoaded]);

  // Debounced: how many users, by country, the current choice takes.
  useEffect(() => {
    if (!open || !active) return;
    setPreviewing(true);
    const t = setTimeout(() => {
      api
        .previewDmrExport(maxCount, priority)
        .then(setPreview)
        .catch(() => setPreview(null))
        .finally(() => setPreviewing(false));
    }, 200);
    return () => clearTimeout(t);
  }, [open, active, maxCount, priority]);

  return { maxCount, priority, preview, previewing, maxText, setMaxText, setPriority, countries, continents };
}

export function DmrUserPicker({
  pick,
  maxLabel = "Max contacts (blank = no limit)",
  maxPlaceholder = "e.g. 50000 — large batches take longer to write over USB",
  noun = "contacts",
}: {
  pick: DmrUserPick;
  maxLabel?: string;
  maxPlaceholder?: string;
  noun?: string;
}) {
  const { priority, setPriority, preview, previewing } = pick;
  const [addChoice, setAddChoice] = useState("");

  const availableToAdd = useMemo(() => {
    const chosen = new Set(priority.map((p) => p.toLowerCase()));
    return {
      continents: pick.continents.filter((c) => !chosen.has(c.toLowerCase())),
      countries: pick.countries.filter((c) => !chosen.has(c.toLowerCase())),
    };
  }, [pick.continents, pick.countries, priority]);

  const add = () => {
    if (!addChoice) return;
    setPriority((p) => [...p, addChoice]);
    setAddChoice("");
  };
  const remove = (i: number) => setPriority((p) => p.filter((_, idx) => idx !== i));
  const move = (i: number, dir: -1 | 1) =>
    setPriority((p) => {
      const j = i + dir;
      if (j < 0 || j >= p.length) return p;
      const next = [...p];
      [next[i], next[j]] = [next[j], next[i]];
      return next;
    });

  return (
    <>
      <label className="flex flex-col gap-1">
        <span className="text-[11px] font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
          {maxLabel}
        </span>
        <TextInput
          inputMode="numeric"
          placeholder={maxPlaceholder}
          value={pick.maxText}
          onChange={(e) => pick.setMaxText(e.target.value.replace(/[^0-9]/g, ""))}
        />
      </label>

      <div className="flex flex-col gap-1.5">
        <span className="text-[11px] font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
          Priority order (matched first; everyone else fills any remaining space)
        </span>
        <div className="flex gap-1.5">
          <Select className="flex-1" value={addChoice} onChange={(e) => setAddChoice(e.target.value)}>
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
          <Button onClick={add} disabled={!addChoice}>
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
                    onClick={() => move(i, -1)}
                    disabled={i === 0}
                    title="Move up"
                  >
                    <ArrowUp size={12} />
                  </button>
                  <button
                    className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-600 disabled:opacity-30 dark:hover:bg-slate-700"
                    onClick={() => move(i, 1)}
                    disabled={i === priority.length - 1}
                    title="Move down"
                  >
                    <ArrowDown size={12} />
                  </button>
                  <button
                    className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-rose-600 dark:hover:bg-slate-700"
                    onClick={() => remove(i)}
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
              {preview.will_export.toLocaleString()} of {preview.total_available.toLocaleString()} {noun} will
              be written
            </div>
            {preview.by_country.length > 0 && (
              <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 text-slate-500">
                {preview.by_country.slice(0, 8).map((b) => (
                  <span key={b.country}>
                    {b.country}: {b.count.toLocaleString()}
                  </span>
                ))}
                {preview.by_country.length > 8 && <span>…</span>}
              </div>
            )}
          </>
        )}
      </div>
    </>
  );
}
