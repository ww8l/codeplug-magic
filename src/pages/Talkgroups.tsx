import { useCallback, useEffect, useMemo, useState } from "react";
import { confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import {
  Plus,
  Pencil,
  Trash2,
  Radio,
  Search,
  Upload,
  Download,
  Users,
  ArrowUp,
  ArrowDown,
  X,
} from "lucide-react";
import clsx from "clsx";
import { api, withToast } from "../lib/api";
import type { Talkgroup } from "../lib/types";
import { DMR_NETWORKS, CALL_TYPES, presentFacet } from "../lib/constants";
import { PageHeader, Button, Spinner, EmptyState, Badge, TextInput, Select } from "../components/ui";
import { TalkgroupImportDialog } from "../components/talkgroups/TalkgroupImportDialog";
import { TalkgroupModal } from "../components/talkgroups/TalkgroupModal";

type SortKey = "tg_number" | "name" | "network" | "call_type" | "notes";
type SortDir = "asc" | "desc";

const SORT_COLUMNS: { key: SortKey; label: string; className: string }[] = [
  { key: "tg_number", label: "Number", className: "px-5 py-2" },
  { key: "name", label: "Name", className: "px-3 py-2" },
  { key: "network", label: "Network", className: "px-3 py-2" },
  { key: "call_type", label: "Type", className: "px-3 py-2" },
  { key: "notes", label: "Notes", className: "px-3 py-2" },
];

export function Talkgroups() {
  const [talkgroups, setTalkgroups] = useState<Talkgroup[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [networkFilter, setNetworkFilter] = useState("");
  const [callTypeFilter, setCallTypeFilter] = useState("");
  const [sortKey, setSortKey] = useState<SortKey>("name");
  const [sortDir, setSortDir] = useState<SortDir>("asc");
  const [editing, setEditing] = useState<Talkgroup | null>(null);
  const [creating, setCreating] = useState(false);
  const [importing, setImporting] = useState(false);

  // Load the full library; search/filter/sort are applied client-side so the
  // filter dropdowns can reflect every value present in the data.
  const load = useCallback(async () => {
    setLoading(true);
    try {
      setTalkgroups(await api.listTalkgroups());
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // Filter dropdown options + the modal's network suggestions — only values
  // actually present in the data (plus the canonical ones).
  const networks = useMemo(
    () => presentFacet(talkgroups.map((t) => t.network), DMR_NETWORKS),
    [talkgroups],
  );
  const callTypes = useMemo(
    () => presentFacet(talkgroups.map((t) => t.call_type), CALL_TYPES),
    [talkgroups],
  );

  const visible = useMemo(() => {
    const term = search.trim().toLowerCase();
    const filtered = talkgroups.filter((tg) => {
      if (networkFilter && tg.network !== networkFilter) return false;
      if (callTypeFilter && tg.call_type !== callTypeFilter) return false;
      if (term) {
        const hay = `${tg.name} ${tg.tg_number} ${tg.notes ?? ""}`.toLowerCase();
        if (!hay.includes(term)) return false;
      }
      return true;
    });
    const arr = [...filtered];
    arr.sort((a, b) => {
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
  }, [talkgroups, search, networkFilter, callTypeFilter, sortKey, sortDir]);

  const toggleSort = (key: SortKey) => {
    if (sortKey === key) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir("asc");
    }
  };

  const hasFilters = !!(search || networkFilter || callTypeFilter);
  const clearFilters = () => {
    setSearch("");
    setNetworkFilter("");
    setCallTypeFilter("");
  };

  // Back up the ENTIRE talkgroup library (including orphans not assigned to any
  // channel) to a lossless JSON file — safe to restore after a database flush.
  const exportAll = async () => {
    if (talkgroups.length === 0) return;
    const { save } = await import("@tauri-apps/plugin-dialog");
    const stamp = new Date().toISOString().slice(0, 10);
    const path = await save({
      defaultPath: `talkgroups-${stamp}.json`,
      filters: [{ name: "Codeplug Magic talkgroup backup", extensions: ["json"] }],
    });
    if (!path) return;
    const n = await withToast(api.exportTalkgroups(path), {
      error: "Export failed",
    });
    if (n !== undefined) {
      const { toast } = await import("sonner");
      toast.success(`Exported ${n} talkgroup${n === 1 ? "" : "s"}`);
    }
  };

  const handleDelete = async (tg: Talkgroup) => {
    const ok = await confirmDialog(
      `Delete talkgroup “${tg.name}” (${tg.tg_number})? It will also be removed from any repeaters it's assigned to.`,
      { title: "Delete talkgroup", kind: "warning" },
    );
    if (!ok) return;
    const res = await withToast(api.deleteTalkgroup(tg.id), {
      success: "Talkgroup deleted",
    });
    if (res !== undefined) setTalkgroups((prev) => prev.filter((t) => t.id !== tg.id));
  };

  return (
    <>
      <PageHeader
        icon={<Users size={18} />}
        title="Talkgroups"
        subtitle={
          hasFilters
            ? `${visible.length} of ${talkgroups.length} talkgroup${talkgroups.length === 1 ? "" : "s"}`
            : `${talkgroups.length} talkgroup${talkgroups.length === 1 ? "" : "s"}`
        }
        actions={
          <>
            <Button onClick={() => setImporting(true)}>
              <Upload size={14} /> Import
            </Button>
            <Button onClick={exportAll} disabled={talkgroups.length === 0}>
              <Download size={14} /> Export
            </Button>
            <Button variant="primary" onClick={() => setCreating(true)}>
              <Plus size={14} /> New Talkgroup
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
            placeholder="Search name, number, notes…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
        <Select value={networkFilter} onChange={(e) => setNetworkFilter(e.target.value)}>
          <option value="">All networks</option>
          {networks.map((n) => (
            <option key={n}>{n}</option>
          ))}
        </Select>
        <Select value={callTypeFilter} onChange={(e) => setCallTypeFilter(e.target.value)}>
          <option value="">Any type</option>
          {callTypes.map((c) => (
            <option key={c}>{c}</option>
          ))}
        </Select>
        {hasFilters && (
          <button
            onClick={clearFilters}
            className="inline-flex items-center gap-1 rounded-full bg-sky-100 px-2 py-0.5 text-[11px] font-medium text-sky-700 hover:bg-sky-200 dark:bg-sky-950 dark:text-sky-300"
          >
            Clear filters <X size={11} />
          </button>
        )}
      </div>

      {loading ? (
        <div className="flex flex-1 items-center justify-center">
          <Spinner className="h-6 w-6" />
        </div>
      ) : talkgroups.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<Radio size={40} strokeWidth={1.5} />}
            title="No talkgroups yet"
            description="Create talkgroups here, then assign them to DMR repeaters from the repeater detail panel."
            action={
              <Button variant="primary" onClick={() => setCreating(true)}>
                <Plus size={14} /> New Talkgroup
              </Button>
            }
          />
        </div>
      ) : visible.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<Radio size={40} strokeWidth={1.5} />}
            title="No matching talkgroups"
            description="No talkgroups match the current filters."
            action={
              <Button onClick={clearFilters}>
                <X size={14} /> Clear filters
              </Button>
            }
          />
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          <table className="w-full text-left text-xs">
            <thead className="sticky top-0 bg-slate-50 text-[11px] uppercase tracking-wide text-slate-400 dark:bg-slate-800">
              <tr>
                {SORT_COLUMNS.map((col) => (
                  <th key={col.key} className={clsx("font-semibold", col.className)}>
                    <button
                      onClick={() => toggleSort(col.key)}
                      className="inline-flex items-center gap-1 uppercase tracking-wide hover:text-slate-600 dark:hover:text-slate-200"
                    >
                      {col.label}
                      {sortKey === col.key &&
                        (sortDir === "asc" ? <ArrowUp size={11} /> : <ArrowDown size={11} />)}
                    </button>
                  </th>
                ))}
                <th className="px-5 py-2" />
              </tr>
            </thead>
            <tbody>
              {visible.map((tg) => (
                <tr
                  key={tg.id}
                  className="border-b border-slate-100 hover:bg-slate-50 dark:border-slate-700/60 dark:hover:bg-slate-800/40"
                >
                  <td className="px-5 py-2 font-mono tabular-nums text-slate-700 dark:text-slate-200">
                    {tg.tg_number}
                  </td>
                  <td className="px-3 py-2 font-medium text-slate-800 dark:text-slate-100">
                    {tg.name}
                  </td>
                  <td className="px-3 py-2">
                    <Badge>{tg.network}</Badge>
                  </td>
                  <td className="px-3 py-2 text-slate-500">{tg.call_type}</td>
                  <td className="max-w-xs truncate px-3 py-2 text-slate-400">
                    {tg.notes}
                  </td>
                  <td className="px-5 py-2">
                    <div className="flex justify-end gap-1.5">
                      <Button onClick={() => setEditing(tg)}>
                        <Pencil size={13} />
                      </Button>
                      <Button variant="danger" onClick={() => handleDelete(tg)}>
                        <Trash2 size={13} />
                      </Button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <TalkgroupModal
        open={creating}
        onClose={() => setCreating(false)}
        title="New Talkgroup"
        networks={networks}
        onSaved={() => {
          setCreating(false);
          load();
        }}
      />
      <TalkgroupModal
        open={editing != null}
        onClose={() => setEditing(null)}
        title="Edit Talkgroup"
        existing={editing}
        networks={networks}
        onSaved={() => {
          setEditing(null);
          load();
        }}
      />
      <TalkgroupImportDialog
        open={importing}
        onClose={() => setImporting(false)}
        onImported={() => load()}
      />
    </>
  );
}
