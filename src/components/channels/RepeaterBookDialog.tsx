import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { Search, KeyRound, SlidersHorizontal } from "lucide-react";
import { toast } from "sonner";
import { api } from "../../lib/api";
import type {
  RbArea,
  RbAreaInfo,
  CityCentroid,
  RbFilters,
  RbImportSummary,
  RbResultRow,
  RbSearchResult,
  RbStatus,
} from "../../lib/types";
import { Modal } from "../overlays";
import { Button, Select, Spinner, TextInput } from "../ui";
import { fmtFreq, fmtOffset } from "../../lib/constants";
import { RbAttribution } from "../../lib/repeaterbook";

/**
 * Search RepeaterBook for one area and import the repeaters the user picks
 * (#136). Every limit the dialog shows is enforced by the backend; the dialog
 * only explains them. "Data courtesy of RepeaterBook.com" sits in the pinned
 * header so it is visible the whole time, as promised.
 */

// Keys match the backend's BANDS / MODES tables in commands/rb_api.rs.
const BANDS: [string, string][] = [
  ["10m", "10 m"],
  ["6m", "6 m"],
  ["2m", "2 m"],
  ["1.25m", "1.25 m"],
  ["70cm", "70 cm"],
  ["33cm", "33 cm"],
  ["23cm", "23 cm"],
];
const MODES: [string, string][] = [
  ["FM", "FM analog"],
  ["DMR", "DMR"],
  ["DSTAR", "D-STAR"],
  ["YSF", "System Fusion"],
  ["NXDN", "NXDN"],
  ["P25", "P25"],
];

// Suggestions for the rest-of-world search. Free text is accepted too: the
// country is sent as typed, and a misspelling simply finds nothing.
const COUNTRIES = [
  "Argentina", "Australia", "Austria", "Belgium", "Brazil", "Bulgaria", "Chile",
  "China", "Colombia", "Costa Rica", "Croatia", "Czech Republic", "Denmark",
  "Dominican Republic", "Ecuador", "Estonia", "Finland", "France", "Germany",
  "Greece", "Guatemala", "Hong Kong", "Hungary", "Iceland", "India", "Indonesia",
  "Ireland", "Israel", "Italy", "Japan", "Latvia", "Lithuania", "Luxembourg",
  "Malaysia", "Malta", "Netherlands", "New Zealand", "Norway", "Panama", "Peru",
  "Philippines", "Poland", "Portugal", "Romania", "Serbia", "Singapore",
  "Slovakia", "Slovenia", "South Africa", "South Korea", "Spain", "Sweden",
  "Switzerland", "Taiwan", "Thailand", "Turkey", "Ukraine", "United Kingdom",
  "Uruguay", "Venezuela",
];

const EMCOMM: [string, string][] = [
  ["ARES", "ARES"],
  ["RACES", "RACES"],
  ["SKYWARN", "SKYWARN"],
  ["CANWARN", "CANWARN"],
];
const DISTANCES = [10, 25, 50, 75, 100, 150];

const cityKey = (c: CityCentroid) => `${c.city}|${c.state ?? ""}`;

/**
 * A row of on/off chips: none on means "any", several on means "any of these".
 */
function Chips({
  label,
  options,
  value,
  onChange,
  disabled,
  title,
}: {
  label: string;
  options: [string, string][];
  value: string[];
  onChange: (v: string[]) => void;
  disabled?: boolean;
  title?: string;
}) {
  const toggle = (k: string) =>
    onChange(value.includes(k) ? value.filter((v) => v !== k) : [...value, k]);
  return (
    <div className="inline-flex flex-wrap items-center gap-1" title={title}>
      <span className="mr-0.5 text-[11px] text-slate-500 dark:text-slate-400">{label}</span>
      {options.map(([k, text]) => {
        const on = value.includes(k);
        return (
          <button
            key={k}
            type="button"
            disabled={disabled}
            aria-pressed={on}
            onClick={() => toggle(k)}
            className={
              "rounded-full border px-2 py-0.5 text-[11px] font-medium transition-colors disabled:opacity-50 " +
              (on
                ? "border-sky-600 bg-sky-600 text-white"
                : "border-slate-300 bg-white text-slate-600 hover:bg-slate-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-300 dark:hover:bg-slate-800")
            }
          >
            {text}
          </button>
        );
      })}
      {value.length === 0 && (
        <span className="text-[11px] text-slate-400 dark:text-slate-500">any</span>
      )}
    </div>
  );
}

const COUNTRY_CHOICE = "__country__";
const LAST_AREA_KEY = "rb.lastArea";

const tone = (r: RbResultRow): string => {
  if (r.tone_mode === "off") return "";
  const up = r.ctcss_uplink != null ? r.ctcss_uplink.toFixed(1) : r.dcs_code ? `D${r.dcs_code}` : "";
  const dn = r.ctcss_downlink != null ? r.ctcss_downlink.toFixed(1) : r.dcs_rx_code ? `D${r.dcs_rx_code}` : "";
  return dn && dn !== up ? `${up || "—"} / ${dn}` : up;
};

const COLUMNS: { label: string; render: (r: RbResultRow) => ReactNode; mono?: boolean }[] = [
  { label: "Call", render: (r) => r.callsign },
  { label: "RX", mono: true, render: (r) => fmtFreq(r.rx_freq) },
  { label: "Offset", mono: true, render: (r) => (r.duplex === "none" ? "" : `${r.duplex}${fmtOffset(r.offset)}`) },
  { label: "Tone", mono: true, render: tone },
  { label: "Mode", render: (r) => r.mode },
  { label: "City", render: (r) => r.city ?? "" },
  { label: "County", render: (r) => r.county ?? "" },
  { label: "Status", render: (r) => r.operational_status ?? "" },
  { label: "Use", render: (r) => r.use_type ?? "" },
  {
    label: "In library",
    render: (r) =>
      r.in_library === "api" ? (
        <span title="Already imported; importing again refreshes it">Yes</span>
      ) : r.in_library === "file" ? (
        <span title="Imported from a RepeaterBook file; importing it here takes that row over, keeping your names and edits">
          From file
        </span>
      ) : (
        ""
      ),
  },
];

function loadLastArea(): string {
  try {
    return localStorage.getItem(LAST_AREA_KEY) ?? "";
  } catch {
    return "";
  }
}

export function RepeaterBookDialog({
  open,
  onClose,
  onImported,
}: {
  open: boolean;
  onClose: () => void;
  onImported: () => void;
}) {
  const navigate = useNavigate();
  const [status, setStatus] = useState<RbStatus | null>(null);
  const [areas, setAreas] = useState<RbAreaInfo[]>([]);
  const [areaChoice, setAreaChoice] = useState<string>(loadLastArea);
  const [country, setCountry] = useState("");
  const [band, setBand] = useState<string[]>([]);
  const [mode, setMode] = useState<string[]>([]);
  // The narrower filters, behind "More filters". All run on RepeaterBook's
  // answer, so changing one inside 10 minutes costs no request.
  const [showMore, setShowMore] = useState(false);
  const [county, setCounty] = useState("");
  const [city, setCity] = useState("");
  const [callsign, setCallsign] = useState("");
  const [onAirOnly, setOnAirOnly] = useState(false);
  const [openOnly, setOpenOnly] = useState(false);
  const [emcomm, setEmcomm] = useState<string[]>([]);
  const [nearCity, setNearCity] = useState("");
  const [nearMiles, setNearMiles] = useState(25);
  const [hideInLibrary, setHideInLibrary] = useState(false);
  const [cities, setCities] = useState<CityCentroid[]>([]);
  const [searching, setSearching] = useState(false);
  const [waitSecs, setWaitSecs] = useState(0);
  const [result, setResult] = useState<RbSearchResult | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [importing, setImporting] = useState(false);
  const [summary, setSummary] = useState<RbImportSummary | null>(null);

  const refreshStatus = () => api.rbStatus().then(setStatus).catch(() => setStatus(null));

  useEffect(() => {
    if (!open) return;
    setResult(null);
    setSelected(new Set());
    setSummary(null);
    void refreshStatus();
    if (areas.length === 0) void api.rbAreas().then(setAreas);
    // Towns already in the library, for "within N miles of".
    void api.listCities().then(setCities).catch(() => setCities([]));
  }, [open]);

  // RepeaterBook said "slow down": the backend is waiting before its retry.
  // Count it down so the dialog doesn't look hung.
  useEffect(() => {
    const un = listen<number>("rb-api-wait", (e) => setWaitSecs(e.payload));
    return () => {
      void un.then((f) => f());
    };
  }, []);
  useEffect(() => {
    if (waitSecs <= 0) return;
    const t = setTimeout(() => setWaitSecs((s) => Math.max(0, s - 1)), 1000);
    return () => clearTimeout(t);
  }, [waitSecs]);

  const area: RbArea | null = useMemo(() => {
    if (areaChoice === COUNTRY_CHOICE) {
      return country.trim() ? { kind: "country", country: country.trim() } : null;
    }
    return areaChoice ? { kind: "region", state_id: areaChoice } : null;
  }, [areaChoice, country]);

  const maxSelected = status?.max_selected ?? 500;

  const isCountry = areaChoice === COUNTRY_CHOICE;
  const origin = cities.find((c) => cityKey(c) === nearCity) ?? null;
  const filters: RbFilters = {
    band,
    mode,
    county: county.trim() || null,
    city: city.trim() || null,
    callsign: callsign.trim() || null,
    on_air_only: onAirOnly,
    open_only: openOnly,
    // RepeaterBook only records these services for North America.
    emcomm: isCountry ? [] : emcomm,
    near: origin
      ? { latitude: origin.latitude, longitude: origin.longitude, miles: nearMiles }
      : null,
    hide_in_library: hideInLibrary,
  };
  const moreCount = [
    filters.county,
    filters.city,
    filters.callsign,
    onAirOnly,
    openOnly,
    filters.emcomm?.length,
    filters.near,
    hideInLibrary,
  ].filter(Boolean).length;

  const search = async () => {
    if (!area || searching) return;
    try {
      localStorage.setItem(LAST_AREA_KEY, areaChoice);
    } catch {
      // Only a convenience.
    }
    setSearching(true);
    setResult(null);
    setSelected(new Set());
    setSummary(null);
    try {
      const r = await api.rbSearch(area, filters);
      setResult(r);
      // Pre-select nothing: nothing is saved until the user picks it.
    } catch (e) {
      toast.error(String(e));
    } finally {
      setSearching(false);
      setWaitSecs(0);
      void refreshStatus();
    }
  };

  const rows = result?.rows ?? [];
  // The limit is checked against the current selection outside the updater:
  // React may run an updater twice (StrictMode does), and a toast is a side
  // effect.
  const toggle = (id: string) => {
    if (!selected.has(id) && selected.size >= maxSelected) {
      toast.error(`At most ${maxSelected} repeaters can be imported at once.`);
      return;
    }
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else if (next.size < maxSelected) next.add(id);
      return next;
    });
  };
  // "Select all" takes repeaters not yet imported from the API, up to the
  // per-import limit, so a search bigger than one import is a few presses of
  // the same button rather than hunting for the rows the last import missed.
  const pickable = rows.filter((r) => r.in_library !== "api");
  const nextBatch = pickable.slice(0, maxSelected).map((r) => r.repeaterbook_id);
  const allPicked =
    nextBatch.length > 0 &&
    selected.size === nextBatch.length &&
    nextBatch.every((id) => selected.has(id));
  const toggleAll = () => setSelected(allPicked ? new Set() : new Set(nextBatch));

  const doImport = async () => {
    const picked = rows.filter((r) => selected.has(r.repeaterbook_id));
    if (picked.length === 0) return;
    setImporting(true);
    try {
      const s = await api.rbImport(picked);
      setSummary(s);
      // Stay on these results: mark what was just imported, clear the
      // selection, and the next "select all" picks up where this one stopped.
      const done = new Set(picked.map((r) => r.repeaterbook_id));
      setResult((prev) =>
        prev && {
          ...prev,
          rows: prev.rows.map((r) =>
            done.has(r.repeaterbook_id) ? { ...r, in_library: "api" as const } : r,
          ),
        },
      );
      setSelected(new Set());
      onImported();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setImporting(false);
      void refreshStatus();
    }
  };

  const noToken = status != null && !status.has_token;
  const us = areas.filter((a) => a.country === "United States");
  const ca = areas.filter((a) => a.country === "Canada");

  let body: ReactNode;
  if (noToken) {
    body = (
      <div className="flex flex-col items-center gap-3 py-16 text-center text-xs text-slate-500 dark:text-slate-400">
        <KeyRound size={28} className="text-slate-400" />
        <p className="max-w-md">
          Searching RepeaterBook needs your own RepeaterBook token. Create one
          free on RepeaterBook, then paste it in Settings — it is kept in this
          computer's credential store and sent only to RepeaterBook.
        </p>
        <Button
          variant="primary"
          onClick={() => {
            onClose();
            navigate("/settings");
          }}
        >
          Open Settings
        </Button>
      </div>
    );
  } else if (searching) {
    body = (
      <div className="flex flex-col items-center gap-3 py-16 text-xs text-slate-500 dark:text-slate-400">
        <Spinner className="h-6 w-6" />
        {waitSecs > 0
          ? `RepeaterBook asked this app to wait — trying again in ${waitSecs} s.`
          : "Asking RepeaterBook…"}
      </div>
    );
  } else if (result && (result.too_many || result.server_truncated)) {
    body = (
      <div className="py-16 text-center text-xs text-slate-600 dark:text-slate-300">
        <p className="mx-auto max-w-lg">
          {result.too_many
            ? `${result.matched.toLocaleString()} repeaters in ${result.area} match — more than the ${(status?.max_results ?? 1000).toLocaleString()} one search may show.`
            : `RepeaterBook sent the most it returns at once for ${result.area}, so some repeaters may be missing from the answer.`}{" "}
          Nothing was kept. Narrow the search by band or mode and try again.
        </p>
      </div>
    );
  } else if (result) {
    body =
      rows.length === 0 ? (
        <div className="py-16 text-center text-xs text-slate-500 dark:text-slate-400">
          No repeaters in {result.area} match.
          {area?.kind === "country" && " Check the country is spelled the way RepeaterBook spells it."}
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col p-4">
          {summary && (
            <div className="mb-3 shrink-0 rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-900 dark:border-emerald-900 dark:bg-emerald-950/40 dark:text-emerald-200">
              <p className="font-semibold">
                Imported {summary.added} repeater{summary.added === 1 ? "" : "s"}
                {summary.updated ? ` · ${summary.updated} refreshed` : ""}
              </p>
              {summary.took_over > 0 && (
                <p>
                  {summary.took_over} of them were already in your library from a
                  RepeaterBook file and are now kept as RepeaterBook API records,
                  with your names and edits intact.
                </p>
              )}
              <p>
                {pickable.length > 0
                  ? `${pickable.length} in this search aren't imported yet — select all picks them up next.`
                  : "Everything in this search is now in your library."}
              </p>
              <RbAttribution className="mt-1 block" />
            </div>
          )}
          <p className="mb-2 shrink-0 text-xs text-slate-500 dark:text-slate-400">
            {result.matched} repeater{result.matched === 1 ? "" : "s"} in {result.area}
            {result.from_cache ? " (same search a moment ago — not asked again)" : ""}. Tick
            the ones to keep; nothing is saved until you import. At most {maxSelected} per import.
          </p>
          <div className="min-h-0 flex-1 overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
            <table className="w-full text-left text-[11px]">
              <thead className="sticky top-0 z-10 bg-slate-100 text-[10px] uppercase tracking-wide text-slate-500 dark:bg-slate-900 dark:text-slate-400">
                <tr>
                  <th className="px-2 py-1.5">
                    <input
                      type="checkbox"
                      className="h-3.5 w-3.5 accent-sky-600"
                      checked={allPicked}
                      onChange={toggleAll}
                      disabled={nextBatch.length === 0}
                      title={
                        nextBatch.length === 0
                          ? "Everything here is already imported"
                          : pickable.length > maxSelected
                            ? `Select the next ${maxSelected} not yet imported`
                            : `Select the ${pickable.length} not yet imported`
                      }
                    />
                  </th>
                  {COLUMNS.map((c) => (
                    <th key={c.label} className="whitespace-nowrap px-2 py-1.5 font-semibold">
                      {c.label}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                {rows.map((r) => (
                  <tr
                    key={r.repeaterbook_id}
                    onClick={() => toggle(r.repeaterbook_id)}
                    className="cursor-pointer text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-700/30"
                  >
                    <td className="px-2 py-1">
                      <input
                        type="checkbox"
                        className="h-3.5 w-3.5 accent-sky-600"
                        checked={selected.has(r.repeaterbook_id)}
                        onChange={() => toggle(r.repeaterbook_id)}
                        onClick={(e) => e.stopPropagation()}
                      />
                    </td>
                    {COLUMNS.map((c) => (
                      <td
                        key={c.label}
                        className={`whitespace-nowrap px-2 py-1 ${c.mono ? "font-mono" : ""}`}
                      >
                        {c.render(r)}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      );
  } else {
    body = (
      <div className="py-16 text-center text-xs text-slate-400">
        Pick one state, province or country, narrow it if you like, and search.
      </div>
    );
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Search RepeaterBook"
      width="max-w-5xl"
      footer={
        <div className="flex items-center justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            {summary ? "Done" : "Cancel"}
          </Button>
          <Button
            variant="primary"
            onClick={doImport}
            disabled={selected.size === 0 || importing || searching}
          >
            {importing ? "Importing…" : `Import ${selected.size} Selected`}
          </Button>
        </div>
      }
    >
      <div className="flex shrink-0 flex-col gap-2 border-b border-slate-200 px-4 py-2.5 dark:border-slate-700">
        <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-slate-500 dark:text-slate-400">
          <RbAttribution />
          {status && (
            <span>
              {status.requests_last_hour} of {status.requests_per_hour} searches used this hour
            </span>
          )}
        </div>
        {!noToken && (
          <div className="flex flex-wrap items-center gap-2">
            <Select
              value={areaChoice}
              onChange={(e) => setAreaChoice(e.target.value)}
              disabled={searching}
            >
              <option value="">Choose an area…</option>
              <optgroup label="United States">
                {us.map((a) => (
                  <option key={a.state_id} value={a.state_id}>
                    {a.name}
                  </option>
                ))}
              </optgroup>
              <optgroup label="Canada">
                {ca.map((a) => (
                  <option key={a.state_id} value={a.state_id}>
                    {a.name}
                  </option>
                ))}
              </optgroup>
              <option value={COUNTRY_CHOICE}>A country outside North America…</option>
            </Select>
            {areaChoice === COUNTRY_CHOICE && (
              <>
                <TextInput
                  list="rb-countries"
                  value={country}
                  placeholder="Country"
                  onChange={(e) => setCountry(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && search()}
                  disabled={searching}
                  className="w-44"
                />
                <datalist id="rb-countries">
                  {COUNTRIES.map((c) => (
                    <option key={c} value={c} />
                  ))}
                </datalist>
              </>
            )}
            <Button
              variant="ghost"
              onClick={() => setShowMore((v) => !v)}
              title="County, city, call sign, status, emergency service, distance"
            >
              <SlidersHorizontal size={14} /> More filters
              {moreCount > 0 && (
                <span className="rounded-full bg-sky-600 px-1.5 text-[10px] font-semibold text-white">
                  {moreCount}
                </span>
              )}
            </Button>
            <Button variant="primary" onClick={search} disabled={!area || searching || importing}>
              {searching ? <Spinner className="h-3.5 w-3.5" /> : <Search size={14} />} Search
            </Button>
          </div>
        )}
        {!noToken && (
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5">
            <Chips label="Band" options={BANDS} value={band} onChange={setBand} disabled={searching} />
            <Chips label="Mode" options={MODES} value={mode} onChange={setMode} disabled={searching} />
          </div>
        )}
        {!noToken && showMore && (
          <div className="flex flex-wrap items-center gap-2 text-xs text-slate-600 dark:text-slate-300">
            <TextInput
              value={county}
              placeholder="County"
              onChange={(e) => setCounty(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && search()}
              disabled={searching}
              className="w-32"
            />
            <TextInput
              value={city}
              placeholder="City"
              onChange={(e) => setCity(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && search()}
              disabled={searching}
              className="w-32"
            />
            <TextInput
              value={callsign}
              placeholder="Call sign"
              onChange={(e) => setCallsign(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && search()}
              disabled={searching}
              className="w-28"
            />
            <Chips
              label="Service"
              options={EMCOMM}
              value={isCountry ? [] : emcomm}
              onChange={setEmcomm}
              disabled={searching || isCountry}
              title={isCountry ? "RepeaterBook records these for North America only" : undefined}
            />
            <span className="inline-flex items-center gap-1">
              Within
              <Select
                value={String(nearMiles)}
                onChange={(e) => setNearMiles(Number(e.target.value))}
                disabled={searching}
              >
                {DISTANCES.map((d) => (
                  <option key={d} value={d}>
                    {d} mi
                  </option>
                ))}
              </Select>
              of
              <Select
                value={nearCity}
                onChange={(e) => setNearCity(e.target.value)}
                disabled={searching || cities.length === 0}
                title={
                  cities.length === 0
                    ? "Towns come from the channels already in your library"
                    : "Repeaters RepeaterBook has no coordinates for are left out"
                }
                className="max-w-44"
              >
                <option value="">anywhere</option>
                {cities.map((c) => (
                  <option key={cityKey(c)} value={cityKey(c)}>
                    {c.city}
                    {c.state ? `, ${c.state}` : ""}
                  </option>
                ))}
              </Select>
            </span>
            <label className="inline-flex items-center gap-1">
              <input
                type="checkbox"
                className="h-3.5 w-3.5 accent-sky-600"
                checked={onAirOnly}
                onChange={(e) => setOnAirOnly(e.target.checked)}
                disabled={searching}
              />
              On the air only
            </label>
            <label className="inline-flex items-center gap-1">
              <input
                type="checkbox"
                className="h-3.5 w-3.5 accent-sky-600"
                checked={openOnly}
                onChange={(e) => setOpenOnly(e.target.checked)}
                disabled={searching}
              />
              Open only
            </label>
            <label className="inline-flex items-center gap-1">
              <input
                type="checkbox"
                className="h-3.5 w-3.5 accent-sky-600"
                checked={hideInLibrary}
                onChange={(e) => setHideInLibrary(e.target.checked)}
                disabled={searching}
              />
              Hide ones I already have
            </label>
            <span className="text-[11px] text-slate-400 dark:text-slate-500">
              These narrow RepeaterBook's answer here — changing them within 10
              minutes doesn't ask RepeaterBook again.
            </span>
          </div>
        )}
      </div>
      {body}
    </Modal>
  );
}
