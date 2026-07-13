import { useRef, useState, useMemo, useEffect } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowUp, ArrowDown } from "lucide-react";
import clsx from "clsx";
import type { Channel } from "../../lib/types";
import { fmtFreq, fmtOffset } from "../../lib/constants";

type Align = "left" | "right" | "center";

export interface Column {
  key: keyof Channel;
  label: string;
  width: number;
  align?: Align;
  render?: (c: Channel) => string;
}

const check = (b: boolean) => (b ? "✓" : "");
const tone = (n: number | null) => (n != null ? n.toFixed(1) : "");

// The full catalog of columns a user can show. Order here is the canonical
// left-to-right order; the visible subset is always rendered in this order.
export const ALL_COLUMNS: Column[] = [
  { key: "name_long", label: "Name", width: 150 },
  { key: "name_short", label: "Short Name", width: 110 },
  { key: "callsign", label: "Callsign", width: 90 },
  { key: "rx_freq", label: "RX Freq", width: 92, align: "right", render: (c) => fmtFreq(c.rx_freq) },
  { key: "tx_freq", label: "TX Freq", width: 92, align: "right", render: (c) => (c.tx_freq != null ? fmtFreq(c.tx_freq) : "") },
  { key: "duplex", label: "Dup", width: 56, align: "center" },
  { key: "offset", label: "Offset", width: 78, align: "right", render: (c) => fmtOffset(c.offset) },
  { key: "band", label: "Band", width: 60 },
  { key: "mode", label: "Mode", width: 70 },
  { key: "tone_mode", label: "Tone", width: 70 },
  { key: "ctcss_uplink", label: "CTCSS↑", width: 76, align: "right", render: (c) => tone(c.ctcss_uplink) },
  { key: "ctcss_downlink", label: "CTCSS↓", width: 76, align: "right", render: (c) => tone(c.ctcss_downlink) },
  { key: "dcs_code", label: "DCS", width: 64, align: "center", render: (c) => c.dcs_code ?? "" },
  { key: "power", label: "Power", width: 64, align: "center", render: (c) => c.power ?? "" },
  { key: "dmr_color_code", label: "DMR CC", width: 64, align: "center", render: (c) => (c.dmr_color_code != null ? String(c.dmr_color_code) : "") },
  { key: "dstar_capable", label: "D-STAR", width: 64, align: "center", render: (c) => check(c.dstar_capable) },
  { key: "ysf_capable", label: "YSF", width: 56, align: "center", render: (c) => check(c.ysf_capable) },
  { key: "nxdn_capable", label: "NXDN", width: 58, align: "center", render: (c) => check(c.nxdn_capable) },
  { key: "p25_capable", label: "P25", width: 54, align: "center", render: (c) => check(c.p25_capable) },
  { key: "m17_capable", label: "M17", width: 54, align: "center", render: (c) => check(c.m17_capable) },
  { key: "allstar_node", label: "AllStar", width: 80 },
  { key: "echolink_node", label: "EchoLink", width: 84 },
  { key: "irlp_node", label: "IRLP", width: 70 },
  { key: "wires_node", label: "Wires-X", width: 80 },
  { key: "use_type", label: "Use", width: 76 },
  { key: "operational_status", label: "Status", width: 100 },
  { key: "city", label: "City", width: 150 },
  { key: "county", label: "County", width: 120 },
  { key: "state", label: "State", width: 56 },
  { key: "country", label: "Country", width: 110 },
  { key: "latitude", label: "Lat", width: 86, align: "right", render: (c) => (c.latitude != null ? c.latitude.toFixed(4) : "") },
  { key: "longitude", label: "Lon", width: 90, align: "right", render: (c) => (c.longitude != null ? c.longitude.toFixed(4) : "") },
  { key: "source", label: "Source", width: 96 },
];

// Default columns shown until the user customizes the view.
export const DEFAULT_VISIBLE_KEYS: (keyof Channel)[] = [
  "name_long",
  "callsign",
  "rx_freq",
  "mode",
  "city",
  "state",
  "operational_status",
];

const SELECT_W = 32;
const INDICATOR_W = 26;
const DISTANCE_W = 96;
const ROW_H = 30;

// Sentinel sort key for the computed distance column.
const DISTANCE_KEY = "__distance__";
type SortKey = keyof Channel | typeof DISTANCE_KEY;
type SortDir = "asc" | "desc";

function cellText(c: Channel, col: Column): string {
  if (col.render) return col.render(c);
  const v = c[col.key];
  return v == null ? "" : String(v);
}

export function ChannelGrid({
  channels,
  selectedId,
  onSelect,
  visibleKeys,
  selectedIds,
  onToggleSelect,
  onToggleSelectAll,
  allSelected,
  distanceById,
}: {
  channels: Channel[];
  selectedId: number | null;
  onSelect: (c: Channel) => void;
  visibleKeys: (keyof Channel)[];
  selectedIds: Set<number>;
  onToggleSelect: (id: number) => void;
  onToggleSelectAll: () => void;
  allSelected: boolean;
  // When set, a sortable "Distance" column appears (miles, keyed by channel id).
  distanceById?: Map<number, number> | null;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [sortKey, setSortKey] = useState<SortKey | null>(null);
  const [sortDir, setSortDir] = useState<SortDir>("asc");

  const hasDistance = !!distanceById;

  // Default to sorting by distance when an origin is set; clear it when removed.
  useEffect(() => {
    if (hasDistance) {
      setSortKey((k) => (k == null ? DISTANCE_KEY : k));
    } else {
      setSortKey((k) => (k === DISTANCE_KEY ? null : k));
    }
  }, [hasDistance]);

  // Render the visible columns in the user's chosen order.
  const byKey = useMemo(
    () => new Map(ALL_COLUMNS.map((c) => [c.key, c])),
    [],
  );
  const columns = useMemo(
    () =>
      visibleKeys
        .map((k) => byKey.get(k))
        .filter((c): c is Column => c != null),
    [visibleKeys, byKey],
  );
  const distancePrefix = hasDistance ? `${DISTANCE_W}px ` : "";
  const totalW = useMemo(
    () =>
      SELECT_W +
      INDICATOR_W +
      (hasDistance ? DISTANCE_W : 0) +
      columns.reduce((s, c) => s + c.width, 0),
    [columns, hasDistance],
  );
  const template = useMemo(
    () =>
      `${SELECT_W}px ${INDICATOR_W}px ${distancePrefix}${columns
        .map((c) => `${c.width}px`)
        .join(" ")}`,
    [columns, distancePrefix],
  );

  const sorted = useMemo(() => {
    if (!sortKey) return channels;
    const arr = [...channels];
    arr.sort((a, b) => {
      if (sortKey === DISTANCE_KEY) {
        // Channels without coordinates have no distance; sort them last.
        const av = distanceById?.get(a.id) ?? Infinity;
        const bv = distanceById?.get(b.id) ?? Infinity;
        return sortDir === "asc" ? av - bv : bv - av;
      }
      const av = a[sortKey];
      const bv = b[sortKey];
      if (av == null && bv == null) return 0;
      if (av == null) return 1;
      if (bv == null) return -1;
      let cmp: number;
      if (typeof av === "number" && typeof bv === "number") cmp = av - bv;
      else cmp = String(av).localeCompare(String(bv), undefined, { numeric: true });
      return sortDir === "asc" ? cmp : -cmp;
    });
    return arr;
  }, [channels, sortKey, sortDir, distanceById]);

  const rowVirtualizer = useVirtualizer({
    count: sorted.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_H,
    overscan: 16,
  });

  const toggleSort = (key: SortKey) => {
    if (sortKey === key) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir("asc");
    }
  };

  const alignClass = (a?: Align) =>
    a === "right" ? "justify-end" : a === "center" ? "justify-center" : "justify-start";

  return (
    <div ref={parentRef} className="flex-1 overflow-auto">
      <div style={{ minWidth: totalW }}>
        {/* Header */}
        <div
          className="sticky top-0 z-10 grid border-b border-slate-200 bg-slate-100 text-[11px] font-semibold uppercase tracking-wide text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400"
          style={{ gridTemplateColumns: template }}
        >
          <div className="flex items-center justify-center">
            <input
              type="checkbox"
              checked={allSelected}
              onChange={onToggleSelectAll}
              title="Select all"
            />
          </div>
          <div />
          {hasDistance && (
            <button
              onClick={() => toggleSort(DISTANCE_KEY)}
              className="flex items-center justify-end gap-1 px-2 py-1.5 hover:text-slate-700 dark:hover:text-slate-200"
            >
              <span className="truncate">Distance</span>
              {sortKey === DISTANCE_KEY &&
                (sortDir === "asc" ? <ArrowUp size={11} /> : <ArrowDown size={11} />)}
            </button>
          )}
          {columns.map((col) => (
            <button
              key={String(col.key)}
              onClick={() => toggleSort(col.key)}
              className={clsx(
                "flex items-center gap-1 px-2 py-1.5 hover:text-slate-700 dark:hover:text-slate-200",
                alignClass(col.align),
              )}
            >
              <span className="truncate">{col.label}</span>
              {sortKey === col.key &&
                (sortDir === "asc" ? <ArrowUp size={11} /> : <ArrowDown size={11} />)}
            </button>
          ))}
        </div>

        {/* Virtualized rows */}
        <div style={{ height: rowVirtualizer.getTotalSize(), position: "relative" }}>
          {rowVirtualizer.getVirtualItems().map((vr) => {
            const c = sorted[vr.index];
            const selected = c.id === selectedId;
            return (
              <div
                key={c.id}
                onClick={() => onSelect(c)}
                className={clsx(
                  "absolute left-0 grid cursor-pointer items-center border-b border-slate-100 text-[12px] dark:border-slate-800",
                  selected
                    ? "bg-sky-50 dark:bg-sky-950/40"
                    : vr.index % 2
                      ? "bg-white hover:bg-slate-50 dark:bg-slate-800 dark:hover:bg-slate-700/40"
                      : "bg-slate-50/40 hover:bg-slate-50 dark:bg-slate-800/40 dark:hover:bg-slate-700/40",
                )}
                style={{
                  gridTemplateColumns: template,
                  height: ROW_H,
                  width: "100%",
                  transform: `translateY(${vr.start}px)`,
                }}
              >
                <div
                  className="flex items-center justify-center"
                  onClick={(e) => e.stopPropagation()}
                >
                  <input
                    type="checkbox"
                    checked={selectedIds.has(c.id)}
                    onChange={() => onToggleSelect(c.id)}
                  />
                </div>
                <div className="flex items-center justify-center">
                  {c.has_overrides && (
                    <span
                      title="Has local overrides vs RepeaterBook"
                      className="h-1.5 w-1.5 rounded-full bg-amber-500"
                    />
                  )}
                </div>
                {hasDistance && (
                  <div className="truncate px-2 text-right font-mono tabular-nums text-slate-700 dark:text-slate-200">
                    {(() => {
                      const d = distanceById?.get(c.id);
                      return d != null ? `${d.toFixed(1)} mi` : "—";
                    })()}
                  </div>
                )}
                {columns.map((col) => (
                  <div
                    key={String(col.key)}
                    className={clsx(
                      "truncate px-2 text-slate-700 dark:text-slate-200",
                      col.align === "right" && "text-right tabular-nums",
                      col.align === "center" && "text-center",
                      (col.key === "rx_freq" ||
                        col.key === "tx_freq" ||
                        col.key === "offset" ||
                        col.key === "ctcss_uplink" ||
                        col.key === "ctcss_downlink") &&
                        "font-mono",
                    )}
                  >
                    {cellText(c, col)}
                  </div>
                ))}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
