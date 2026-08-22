import { useCallback, useEffect, useMemo, useState } from "react";
import { confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import { useSearchParams } from "react-router-dom";
import { Upload, Download, Plus, Search, X, Radio, Trash2, ListPlus, MapPin, BookMarked } from "lucide-react";
import { api, withToast } from "../lib/api";
import { useOnlineGeocoding } from "../lib/prefs";
import type { Channel, ChannelFilter, CityCentroid } from "../lib/types";
import { BANDS, MODES, OP_STATUSES } from "../lib/constants";
import { haversineMiles } from "../lib/geo";
import { PageHeader, Button, TextInput, Select, Spinner, EmptyState } from "../components/ui";
import {
  ChannelGrid,
  ALL_COLUMNS,
  DEFAULT_VISIBLE_KEYS,
} from "../components/channels/ChannelGrid";
import { ColumnPicker } from "../components/channels/ColumnPicker";
import { ChannelDetailPanel } from "../components/channels/ChannelDetailPanel";
import { ImportDialog } from "../components/channels/ImportDialog";
import { StandardListDialog } from "../components/channels/StandardListDialog";
import { AddToListModal } from "../components/channels/AddToListModal";
import {
  DistanceControl,
  type DistanceOrigin,
} from "../components/channels/DistanceControl";

const EMPTY: ChannelFilter = {};

const COLUMN_PREFS_KEY = "repeaters.visibleColumns";

// Load the saved column selection, keeping only keys that still exist.
function loadVisibleColumns(): (keyof Channel)[] {
  try {
    const raw = localStorage.getItem(COLUMN_PREFS_KEY);
    if (raw) {
      const arr = JSON.parse(raw);
      if (Array.isArray(arr)) {
        const valid = arr.filter((k) =>
          ALL_COLUMNS.some((c) => c.key === k),
        ) as (keyof Channel)[];
        if (valid.length) return valid;
      }
    }
  } catch {
    /* fall through to defaults */
  }
  return DEFAULT_VISIBLE_KEYS;
}

// Keep only the canonical options actually present in the data, preserving the
// canonical order; append any non-canonical values (sorted) so nothing is lost.
function presentFacet<T extends string>(
  values: (T | null | undefined)[],
  canonical: readonly T[],
): T[] {
  const present = new Set<T>();
  for (const v of values) if (v != null && v !== "") present.add(v);
  const ordered = canonical.filter((c) => present.has(c));
  const extras = [...present].filter((v) => !canonical.includes(v)).sort();
  return [...ordered, ...extras];
}

export function Channels() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [channels, setChannels] = useState<Channel[]>([]);
  // Unfiltered list, used only to populate the filter dropdowns so they show
  // every value present in the data (not just what the active filter leaves).
  const [allChannels, setAllChannels] = useState<Channel[]>([]);
  const [loading, setLoading] = useState(true);

  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<ChannelFilter>(EMPTY);

  const [selected, setSelected] = useState<Channel | null>(null);
  const [panelMode, setPanelMode] = useState<"edit" | "create">("edit");
  const [panelOpen, setPanelOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [standardOpen, setStandardOpen] = useState(false);
  const [addToListOpen, setAddToListOpen] = useState(false);

  const [visibleColumns, setVisibleColumns] =
    useState<(keyof Channel)[]>(loadVisibleColumns);

  // Distance-from-town origin + optional radius filter.
  const [cities, setCities] = useState<CityCentroid[]>([]);
  const [origin, setOrigin] = useState<DistanceOrigin | null>(null);
  const [maxMiles, setMaxMiles] = useState<number | null>(null);

  const changeColumns = (keys: (keyof Channel)[]) => {
    setVisibleColumns(keys);
    localStorage.setItem(COLUMN_PREFS_KEY, JSON.stringify(keys));
  };
  const resetColumns = () => changeColumns(DEFAULT_VISIBLE_KEYS);

  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());

  const load = useCallback(async (f: ChannelFilter) => {
    setLoading(true);
    try {
      setChannels(await api.listChannels(f));
      setSelectedIds(new Set()); // selection is scoped to the current view
    } finally {
      setLoading(false);
    }
  }, []);

  // Refresh the unfiltered list that backs the filter dropdowns.
  const loadFacets = useCallback(async () => {
    setAllChannels(await api.listChannels({}));
  }, []);
  useEffect(() => {
    loadFacets();
  }, [loadFacets]);

  // Distance from the chosen origin for each repeater (miles).
  const distanceById = useMemo(() => {
    if (!origin) return null;
    const m = new Map<number, number>();
    for (const c of channels) {
      if (c.latitude != null && c.longitude != null) {
        m.set(
          c.id,
          haversineMiles(origin.latitude, origin.longitude, c.latitude, c.longitude),
        );
      }
    }
    return m;
  }, [origin, channels]);

  // Apply the optional radius filter on top of the backend-filtered list.
  const visibleChannels = useMemo(() => {
    if (!origin || maxMiles == null || !distanceById) return channels;
    return channels.filter((c) => {
      const d = distanceById.get(c.id);
      return d != null && d <= maxMiles;
    });
  }, [channels, origin, maxMiles, distanceById]);

  const toggleSelect = (id: number) =>
    setSelectedIds((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });

  const allSelected =
    visibleChannels.length > 0 &&
    visibleChannels.every((c) => selectedIds.has(c.id));

  const toggleSelectAll = () =>
    setSelectedIds(
      allSelected ? new Set() : new Set(visibleChannels.map((c) => c.id)),
    );

  const exportSelected = async () => {
    const ids = [...selectedIds];
    if (!ids.length) return;
    const { save } = await import("@tauri-apps/plugin-dialog");
    const stamp = new Date().toISOString().slice(0, 10);
    const path = await save({
      defaultPath: `channels-${stamp}.json`,
      filters: [{ name: "Codeplug Magic channel backup", extensions: ["json"] }],
    });
    if (!path) return;
    const n = await withToast(api.exportChannels(ids, path), {
      error: "Export failed",
    });
    if (n !== undefined) {
      const { toast } = await import("sonner");
      toast.success(`Exported ${n} channel${n === 1 ? "" : "s"}`);
    }
  };

  const deleteSelected = async () => {
    const ids = [...selectedIds];
    if (!ids.length) return;
    const ok = await confirmDialog(
      `Delete ${ids.length} repeater${ids.length === 1 ? "" : "s"}? This cannot be undone.`,
      { title: "Delete repeaters", kind: "warning" },
    );
    if (!ok) return;
    const n = await withToast(api.deleteChannels(ids), { error: "Delete failed" });
    if (n !== undefined) {
      const { toast } = await import("sonner");
      toast.success(`Deleted ${n} repeater${n === 1 ? "" : "s"}`);
      load(filter);
      loadFacets();
    }
  };

  // Channels that have a city but no coordinates — what the backfill targets.
  const missingCoords = useMemo(
    () =>
      allChannels.filter(
        (c) =>
          (c.city ?? "").trim() !== "" &&
          (c.latitude == null || c.longitude == null),
      ).length,
    [allChannels],
  );
  const [backfilling, setBackfilling] = useState(false);
  const [geocodeOnline] = useOnlineGeocoding();

  const backfillCoords = async () => {
    if (missingCoords === 0) return;
    const plural = missingCoords === 1 ? "" : "s";
    const ok = await confirmDialog(
      geocodeOnline
        ? `Look up coordinates for ${missingCoords} channel${plural} that have a city ` +
            `but no lat/lon? Towns matching your existing repeaters are filled ` +
            `instantly and offline; the rest have their town name sent to ` +
            `OpenStreetMap (about 1 per second), so this may take a moment. ` +
            `Channels that already have coordinates are left untouched.`
        : `Fill coordinates for ${missingCoords} channel${plural} that have a city ` +
            `but no lat/lon, using only towns already in your own channels? Online ` +
            `lookup is off, so nothing is sent anywhere and towns you have no ` +
            `repeater in are left blank. Channels that already have coordinates ` +
            `are left untouched.`,
      { title: "Backfill coordinates", kind: "info" },
    );
    if (!ok) return;
    setBackfilling(true);
    const res = await withToast(api.backfillChannelCoordinates(geocodeOnline), {
      error: "Backfill failed",
    });
    setBackfilling(false);
    if (res) {
      const { toast } = await import("sonner");
      const filled = res.from_data + res.from_online;
      const bits = [`Filled ${filled} of ${res.scanned}`];
      if (res.not_found) bits.push(`${res.not_found} not found`);
      if (res.skipped_offline)
        bits.push(`${res.skipped_offline} need online lookup (off)`);
      if (res.failed) bits.push(`${res.failed} failed`);
      toast.success(bits.join(" · "));
      load(filter);
      loadFacets();
      loadCities();
    }
  };

  // Debounce the search box into the active filter.
  useEffect(() => {
    const t = setTimeout(() => {
      setFilter((f) => ({ ...f, search: search || null }));
    }, 250);
    return () => clearTimeout(t);
  }, [search]);

  useEffect(() => {
    load(filter);
  }, [filter, load]);

  // Town list for the distance-origin picker (independent of the active filter).
  const loadCities = useCallback(async () => {
    setCities(await api.listCities());
  }, []);
  useEffect(() => {
    loadCities();
  }, [loadCities]);

  // Auto-open import dialog when navigated from the dashboard quick action.
  useEffect(() => {
    if (searchParams.get("import") === "1") {
      setImportOpen(true);
      searchParams.delete("import");
      setSearchParams(searchParams, { replace: true });
    }
  }, [searchParams, setSearchParams]);

  // Filter dropdown options — only values actually present in the data.
  const bands = useMemo(
    () => presentFacet(allChannels.map((c) => c.band), BANDS),
    [allChannels],
  );
  const modes = useMemo(
    () => presentFacet(allChannels.map((c) => c.mode), MODES),
    [allChannels],
  );
  const statuses = useMemo(
    () => presentFacet(allChannels.map((c) => c.operational_status), OP_STATUSES),
    [allChannels],
  );
  const states = useMemo(() => {
    const s = new Set<string>();
    allChannels.forEach((c) => c.state && s.add(c.state));
    return [...s].sort();
  }, [allChannels]);
  const sources = useMemo(() => {
    const s = new Set<string>();
    allChannels.forEach((c) => c.source && s.add(c.source));
    return [...s].sort();
  }, [allChannels]);

  const setF = (key: keyof ChannelFilter, value: string) =>
    setFilter((f) => ({ ...f, [key]: value || null }));

  const reload = () => {
    load(filter);
    loadCities();
    loadFacets();
  };

  const openCreate = () => {
    setSelected(null);
    setPanelMode("create");
    setPanelOpen(true);
  };
  const openEdit = (c: Channel) => {
    setSelected(c);
    setPanelMode("edit");
    setPanelOpen(true);
  };

  // Build active-filter chips.
  const chips: { key: keyof ChannelFilter; label: string }[] = [];
  if (filter.band) chips.push({ key: "band", label: `Band: ${filter.band}` });
  if (filter.mode) chips.push({ key: "mode", label: `Mode: ${filter.mode}` });
  if (filter.state) chips.push({ key: "state", label: `State: ${filter.state}` });
  if (filter.operational_status)
    chips.push({ key: "operational_status", label: `Status: ${filter.operational_status}` });
  if (filter.source) chips.push({ key: "source", label: `Source: ${filter.source}` });

  return (
    <>
      <PageHeader
        icon={<Radio size={18} />}
        title="Channels"
        subtitle={`Master repeater table · ${visibleChannels.length} ${
          visibleChannels.length === 1 ? "repeater" : "repeaters"
        }${
          origin && maxMiles != null
            ? ` within ${maxMiles} mi of ${origin.label}`
            : ""
        }`}
        actions={
          <>
            {missingCoords > 0 && (
              <Button onClick={backfillCoords} disabled={backfilling}>
                {backfilling ? (
                  <Spinner className="h-3.5 w-3.5" />
                ) : (
                  <MapPin size={14} />
                )}
                Backfill coords ({missingCoords})
              </Button>
            )}
            <Button onClick={() => setStandardOpen(true)}>
              <BookMarked size={14} /> Standard Lists
            </Button>
            <Button onClick={() => setImportOpen(true)}>
              <Upload size={14} /> Import
            </Button>
            <Button variant="primary" onClick={openCreate}>
              <Plus size={14} /> Add Repeater
            </Button>
          </>
        }
      />

      {/* Filter bar */}
      <div className="flex flex-wrap items-center gap-2 border-b border-slate-200 px-4 py-2 dark:border-slate-700">
        <div className="relative">
          <Search
            size={13}
            className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-slate-400"
          />
          <TextInput
            className="w-56 pl-7"
            placeholder="Search name, callsign, city, frequency…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
        <Select value={filter.band ?? ""} onChange={(e) => setF("band", e.target.value)}>
          <option value="">All bands</option>
          {bands.map((b) => (
            <option key={b}>{b}</option>
          ))}
        </Select>
        <Select value={filter.mode ?? ""} onChange={(e) => setF("mode", e.target.value)}>
          <option value="">All modes</option>
          {modes.map((m) => (
            <option key={m}>{m}</option>
          ))}
        </Select>
        <Select value={filter.state ?? ""} onChange={(e) => setF("state", e.target.value)}>
          <option value="">All states</option>
          {states.map((s) => (
            <option key={s}>{s}</option>
          ))}
        </Select>
        <Select
          value={filter.operational_status ?? ""}
          onChange={(e) => setF("operational_status", e.target.value)}
        >
          <option value="">Any status</option>
          {statuses.map((s) => (
            <option key={s}>{s}</option>
          ))}
        </Select>
        {sources.length > 1 && (
          <Select
            value={filter.source ?? ""}
            onChange={(e) => setF("source", e.target.value)}
          >
            <option value="">Any source</option>
            {sources.map((s) => (
              <option key={s}>{s}</option>
            ))}
          </Select>
        )}

        <DistanceControl
          cities={cities}
          origin={origin}
          onSetOrigin={setOrigin}
          maxMiles={maxMiles}
          onSetMaxMiles={setMaxMiles}
        />

        {chips.map((chip) => (
          <button
            key={chip.key}
            onClick={() => setF(chip.key, "")}
            className="inline-flex items-center gap-1 rounded-full bg-sky-100 px-2 py-0.5 text-[11px] font-medium text-sky-700 hover:bg-sky-200 dark:bg-sky-950 dark:text-sky-300"
          >
            {chip.label} <X size={11} />
          </button>
        ))}

        <div className="ml-auto">
          <ColumnPicker
            columns={ALL_COLUMNS.map((c) => ({ key: c.key, label: c.label }))}
            visibleKeys={visibleColumns}
            onChange={changeColumns}
            onReset={resetColumns}
          />
        </div>
      </div>

      {/* Selection action bar */}
      {selectedIds.size > 0 && (
        <div className="flex items-center gap-3 border-b border-slate-200 bg-sky-50 px-4 py-2 text-xs dark:border-slate-700 dark:bg-sky-950/30">
          <span className="font-medium text-slate-700 dark:text-slate-200">
            {selectedIds.size} selected
          </span>
          <Button variant="primary" onClick={() => setAddToListOpen(true)}>
            <ListPlus size={14} /> Add to channel list
          </Button>
          <Button onClick={exportSelected}>
            <Download size={14} /> Export
          </Button>
          <Button variant="danger" onClick={deleteSelected}>
            <Trash2 size={14} /> Delete {selectedIds.size}
          </Button>
          <button
            onClick={() => setSelectedIds(new Set())}
            className="text-slate-500 hover:underline dark:text-slate-400"
          >
            Clear
          </button>
        </div>
      )}

      {/* Grid */}
      {loading ? (
        <div className="flex flex-1 items-center justify-center">
          <Spinner className="h-6 w-6" />
        </div>
      ) : channels.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<Radio size={40} strokeWidth={1.5} />}
            title="No repeaters yet"
            description="Import a RepeaterBook CSV/JSON export or add a repeater manually to start building your master table."
            action={
              <div className="flex gap-2">
                <Button onClick={() => setStandardOpen(true)}>
                  <BookMarked size={14} /> Standard Lists
                </Button>
                <Button onClick={() => setImportOpen(true)}>
                  <Upload size={14} /> Import
                </Button>
                <Button variant="primary" onClick={openCreate}>
                  <Plus size={14} /> Add Repeater
                </Button>
              </div>
            }
          />
        </div>
      ) : visibleChannels.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<Radio size={40} strokeWidth={1.5} />}
            title="No repeaters match"
            description={
              origin && maxMiles != null
                ? `No repeaters within ${maxMiles} mi of ${origin.label}. Try a larger radius.`
                : "No repeaters match the current filters."
            }
          />
        </div>
      ) : (
        <ChannelGrid
          channels={visibleChannels}
          selectedId={selected?.id ?? null}
          onSelect={openEdit}
          visibleKeys={visibleColumns}
          selectedIds={selectedIds}
          onToggleSelect={toggleSelect}
          onToggleSelectAll={toggleSelectAll}
          allSelected={allSelected}
          distanceById={distanceById}
        />
      )}

      <ChannelDetailPanel
        channel={selected}
        mode={panelMode}
        open={panelOpen}
        onClose={() => setPanelOpen(false)}
        onSaved={reload}
        // Stay in the panel but switch it to the copy, so the whole point of
        // copying — tweak one field and save — is the very next action.
        onDuplicated={(copy) => {
          setSelected(copy);
          setPanelMode("edit");
        }}
        sourceSuggestions={sources}
        cities={cities}
      />
      <ImportDialog
        open={importOpen}
        onClose={() => setImportOpen(false)}
        onImported={reload}
      />
      <StandardListDialog
        open={standardOpen}
        onClose={() => setStandardOpen(false)}
        onImported={reload}
      />
      <AddToListModal
        open={addToListOpen}
        onClose={() => setAddToListOpen(false)}
        channels={channels.filter((c) => selectedIds.has(c.id))}
        onAdded={() => setAddToListOpen(false)}
      />
    </>
  );
}
