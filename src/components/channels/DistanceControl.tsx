import { useEffect, useMemo, useRef, useState } from "react";
import { MapPin, X } from "lucide-react";
import type { CityCentroid } from "../../lib/types";
import { TextInput } from "../ui";

export interface DistanceOrigin {
  label: string;
  latitude: number;
  longitude: number;
}

const cityLabel = (c: CityCentroid) =>
  c.state ? `${c.city}, ${c.state}` : c.city;

export function DistanceControl({
  cities,
  origin,
  onSetOrigin,
  maxMiles,
  onSetMaxMiles,
}: {
  cities: CityCentroid[];
  origin: DistanceOrigin | null;
  onSetOrigin: (o: DistanceOrigin | null) => void;
  maxMiles: number | null;
  onSetMaxMiles: (n: number | null) => void;
}) {
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return cities.slice(0, 20);
    return cities
      .filter((c) => cityLabel(c).toLowerCase().includes(q))
      .slice(0, 20);
  }, [cities, query]);

  const pick = (c: CityCentroid) => {
    onSetOrigin({
      label: cityLabel(c),
      latitude: c.latitude,
      longitude: c.longitude,
    });
    setQuery("");
    setOpen(false);
  };

  if (origin) {
    return (
      <div className="flex items-center gap-2">
        <span className="inline-flex items-center gap-1 rounded-full bg-emerald-100 px-2 py-0.5 text-[11px] font-medium text-emerald-700 dark:bg-emerald-950 dark:text-emerald-300">
          <MapPin size={11} />
          Distance from: {origin.label}
          <button
            onClick={() => onSetOrigin(null)}
            className="hover:text-emerald-900 dark:hover:text-emerald-100"
            title="Clear distance origin"
          >
            <X size={11} />
          </button>
        </span>
        <label className="flex items-center gap-1 text-[11px] text-slate-500 dark:text-slate-400">
          within
          <TextInput
            type="number"
            min={0}
            className="w-16"
            placeholder="any"
            value={maxMiles ?? ""}
            onChange={(e) =>
              onSetMaxMiles(e.target.value === "" ? null : Number(e.target.value))
            }
          />
          mi
        </label>
      </div>
    );
  }

  return (
    <div className="relative" ref={ref}>
      <div className="relative">
        <MapPin
          size={13}
          className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-slate-400"
        />
        <TextInput
          className="w-48 pl-7"
          placeholder="Distance from town…"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setOpen(true);
          }}
          onFocus={() => setOpen(true)}
        />
      </div>
      {open && (
        <div className="absolute left-0 z-30 mt-1 max-h-64 w-64 overflow-auto rounded-md border border-slate-200 bg-white shadow-lg dark:border-slate-700 dark:bg-slate-800">
          {matches.length === 0 ? (
            <div className="px-3 py-4 text-center text-xs text-slate-400">
              No matching towns in your data.
            </div>
          ) : (
            matches.map((c) => (
              <button
                key={`${c.city}|${c.state ?? ""}`}
                onClick={() => pick(c)}
                className="flex w-full items-center justify-between gap-2 border-b border-slate-100 px-3 py-1.5 text-left text-xs last:border-b-0 hover:bg-slate-50 dark:border-slate-700/60 dark:hover:bg-slate-700/40"
              >
                <span className="truncate font-medium text-slate-800 dark:text-slate-100">
                  {cityLabel(c)}
                </span>
                <span className="shrink-0 text-[11px] text-slate-400">
                  {c.count}
                </span>
              </button>
            ))
          )}
        </div>
      )}
    </div>
  );
}
