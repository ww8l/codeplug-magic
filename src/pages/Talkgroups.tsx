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
import type { Talkgroup, TalkgroupInput } from "../lib/types";
import { DMR_NETWORKS, CALL_TYPES } from "../lib/constants";
import { PageHeader, Button, Spinner, EmptyState, Badge, TextInput, Select } from "../components/ui";
import { Modal } from "../components/overlays";
import { TalkgroupImportDialog } from "../components/talkgroups/TalkgroupImportDialog";

const EMPTY: TalkgroupInput = {
  tg_number: 0,
  name: "",
  network: DMR_NETWORKS[0],
  call_type: CALL_TYPES[0],
  notes: null,
};

// Keep only the canonical options actually present in the data, preserving the
// canonical order; append any non-canonical values (sorted) so nothing is lost.
function presentFacet(values: (string | null | undefined)[], canonical: readonly string[]): string[] {
  const present = new Set<string>();
  for (const v of values) if (v != null && v !== "") present.add(v);
  const ordered = canonical.filter((c) => present.has(c));
  const extras = [...present].filter((v) => !canonical.includes(v)).sort();
  return [...ordered, ...extras];
}

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
      filters: [{ name: "73plug talkgroup backup", extensions: ["json"] }],
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

// Sentinel option that switches the network picker into free-text entry.
const CUSTOM_NETWORK = "__custom__";

function TalkgroupModal({
  open,
  onClose,
  title,
  existing,
  networks,
  onSaved,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  existing?: Talkgroup | null;
  // Networks already present in the library, merged with the canonical list.
  networks: string[];
  onSaved: () => void;
}) {
  const [form, setForm] = useState<TalkgroupInput>(EMPTY);
  const [tgText, setTgText] = useState("");
  const [saving, setSaving] = useState(false);
  // When true, the network field is a free-text box for a custom network name.
  const [customNetwork, setCustomNetwork] = useState(false);

  // The dropdown choices: every known network plus the canonical defaults.
  const networkOptions = useMemo(
    () => presentFacet(networks, DMR_NETWORKS),
    [networks],
  );

  // Re-seed when opened.
  const [wasOpen, setWasOpen] = useState(false);
  if (open && !wasOpen) {
    if (existing) {
      setForm({
        tg_number: existing.tg_number,
        name: existing.name,
        network: existing.network,
        call_type: existing.call_type,
        notes: existing.notes,
      });
      setTgText(String(existing.tg_number));
      // If the saved network isn't a known option, open in custom-entry mode.
      setCustomNetwork(!networkOptions.includes(existing.network));
    } else {
      setForm(EMPTY);
      setTgText("");
      setCustomNetwork(false);
    }
    setWasOpen(true);
  } else if (!open && wasOpen) {
    setWasOpen(false);
  }

  const set = <K extends keyof TalkgroupInput>(k: K, v: TalkgroupInput[K]) =>
    setForm((f) => ({ ...f, [k]: v }));

  const tgNumber = parseInt(tgText, 10);
  const valid =
    form.name.trim() !== "" &&
    form.network.trim() !== "" &&
    Number.isFinite(tgNumber) &&
    tgNumber > 0;

  const submit = async () => {
    if (!valid) return;
    const payload: TalkgroupInput = {
      ...form,
      tg_number: tgNumber,
      name: form.name.trim(),
      network: form.network.trim(),
      notes: form.notes?.trim() || null,
    };
    setSaving(true);
    const res = await withToast(
      existing
        ? api.updateTalkgroup(existing.id, payload)
        : api.createTalkgroup(payload),
      {
        success: existing ? "Talkgroup saved" : "Talkgroup created",
        error: "Could not save talkgroup (duplicate number on this network?)",
      },
    );
    setSaving(false);
    if (res) onSaved();
  };

  return (
    <Modal open={open} onClose={onClose} title={title} width="max-w-md">
      <div className="flex flex-col gap-3 p-4">
        <div className="grid grid-cols-2 gap-3">
          <label className="flex flex-col gap-1">
            <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
              Talkgroup number
            </span>
            <TextInput
              autoFocus
              inputMode="numeric"
              placeholder="e.g. 3108"
              value={tgText}
              onChange={(e) => setTgText(e.target.value.replace(/[^0-9]/g, ""))}
              onKeyDown={(e) => e.key === "Enter" && submit()}
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
              Name
            </span>
            <TextInput
              placeholder="e.g. Colorado"
              value={form.name}
              onChange={(e) => set("name", e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && submit()}
            />
          </label>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <label className="flex flex-col gap-1">
            <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
              Network
            </span>
            {customNetwork ? (
              <div className="flex gap-1.5">
                <TextInput
                  autoFocus
                  className="flex-1"
                  placeholder="Custom network name"
                  value={form.network}
                  onChange={(e) => set("network", e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && submit()}
                />
                <Button
                  variant="ghost"
                  onClick={() => {
                    setCustomNetwork(false);
                    set("network", networkOptions[0] ?? DMR_NETWORKS[0]);
                  }}
                  title="Choose from the list instead"
                >
                  <X size={13} />
                </Button>
              </div>
            ) : (
              <Select
                value={networkOptions.includes(form.network) ? form.network : ""}
                onChange={(e) => {
                  if (e.target.value === CUSTOM_NETWORK) {
                    setCustomNetwork(true);
                    set("network", "");
                  } else {
                    set("network", e.target.value);
                  }
                }}
              >
                {networkOptions.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
                <option value={CUSTOM_NETWORK}>+ Add custom network…</option>
              </Select>
            )}
          </label>
          <label className="flex flex-col gap-1">
            <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
              Call type
            </span>
            <Select
              value={form.call_type}
              onChange={(e) => set("call_type", e.target.value)}
            >
              {CALL_TYPES.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </Select>
          </label>
        </div>
        <label className="flex flex-col gap-1">
          <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Notes
          </span>
          <textarea
            className="w-full rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs text-slate-800 focus:border-sky-500 focus:outline-none focus:ring-1 focus:ring-sky-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
            rows={2}
            value={form.notes ?? ""}
            onChange={(e) => set("notes", e.target.value)}
          />
        </label>
      </div>
      <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" onClick={submit} disabled={saving || !valid}>
          {existing ? "Save" : "Create"}
        </Button>
      </div>
    </Modal>
  );
}
