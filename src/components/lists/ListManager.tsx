import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { Plus, Pencil, Trash2, ListChecks } from "lucide-react";
import { confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import clsx from "clsx";
import type { Channel, CityCentroid, ScanListSettings } from "../../lib/types";
import { api, withToast } from "../../lib/api";
import { PageHeader, Button, Spinner, EmptyState, Badge } from "../ui";
import { ChannelDetailPanel } from "../channels/ChannelDetailPanel";
import { ChannelPickerModal } from "./ChannelPickerModal";
import { OrderedChannelTable } from "./OrderedChannelTable";
import { ListMetaModal } from "./ListMetaModal";
import { channelLabel } from "./channelLabel";

export interface ListSummary {
  id: number;
  name: string;
  description: string | null;
  // Scan-list-only settings (undefined for zones/groups).
  priority_channel_id?: number | null;
  priority_channel_2_id?: number | null;
  priority_select?: number;
  look_back_a?: number;
  look_back_b?: number;
  dropout_delay?: number;
  dwell_time?: number;
  revert_channel?: number;
  channel_count: number;
}

export interface ListAdapter {
  noun: string; // singular, lowercase: "channel list"
  nounPlural: string; // "channel lists"
  emptyIcon: ReactNode;
  // Optional icon shown next to the page title (matches the sidebar glyph).
  titleIcon?: ReactNode;
  // Optional page-title override; defaults to the capitalized nounPlural.
  title?: string;
  listAll: () => Promise<ListSummary[]>;
  create: (name: string, description: string | null) => Promise<{ id: number }>;
  rename: (
    list: ListSummary,
    name: string,
    description: string | null,
  ) => Promise<void>;
  remove: (id: number) => Promise<void>;
  getChannels: (id: number) => Promise<Channel[]>;
  addChannel: (id: number, channelId: number) => Promise<void>;
  removeChannel: (id: number, channelId: number) => Promise<void>;
  reorder: (id: number, orderedIds: number[]) => Promise<void>;
  // Present for scan lists: persists the seven per-scan-list settings (edited in
  // the Edit modal). Its presence also switches the Edit modal into scan mode.
  saveScanSettings?: (
    list: ListSummary,
    name: string,
    description: string | null,
    settings: ScanListSettings,
  ) => Promise<void>;
  // Present for channel lists ("zones"): create-or-mirror a scan list of the
  // same name from this list's current channels. Returns a summary for toast.
  syncMatchingScanList?: (
    list: ListSummary,
  ) => Promise<{ created: boolean; added: number; removed: number }>;
}

export function ListManager({ adapter }: { adapter: ListAdapter }) {
  const [lists, setLists] = useState<ListSummary[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [loading, setLoading] = useState(true);
  const [channelsLoading, setChannelsLoading] = useState(false);

  const [pickerOpen, setPickerOpen] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [renameOpen, setRenameOpen] = useState(false);
  const [editChannel, setEditChannel] = useState<Channel | null>(null);
  const [editOpen, setEditOpen] = useState(false);
  const [cities, setCities] = useState<CityCentroid[]>([]);

  const Title =
    adapter.title ?? adapter.nounPlural.replace(/\b\w/g, (c) => c.toUpperCase());

  const loadLists = useCallback(async () => {
    setLoading(true);
    try {
      const ls = await adapter.listAll();
      setLists(ls);
      setSelectedId((cur) =>
        cur != null && ls.some((l) => l.id === cur) ? cur : (ls[0]?.id ?? null),
      );
    } finally {
      setLoading(false);
    }
  }, [adapter]);

  useEffect(() => {
    loadLists();
  }, [loadLists]);

  useEffect(() => {
    api.listCities().then(setCities);
  }, []);

  // Re-fetch after an edit: a rename changes the row label, and a delete from
  // the panel drops the channel from the master table (cascading out of this
  // list), so the sidebar count is resynced from what actually came back.
  const refreshChannels = useCallback(async () => {
    if (selectedId == null) return;
    const cs = await adapter.getChannels(selectedId);
    setChannels(cs);
    setLists((prev) =>
      prev.map((l) =>
        l.id === selectedId ? { ...l, channel_count: cs.length } : l,
      ),
    );
  }, [selectedId, adapter]);

  // Load channels for the selected list.
  useEffect(() => {
    if (selectedId == null) {
      setChannels([]);
      return;
    }
    let active = true;
    setChannelsLoading(true);
    adapter
      .getChannels(selectedId)
      .then((cs) => active && setChannels(cs))
      .finally(() => active && setChannelsLoading(false));
    return () => {
      active = false;
    };
  }, [selectedId, adapter]);

  const selected = lists.find((l) => l.id === selectedId) ?? null;
  const existingIds = useMemo(
    () => new Set(channels.map((c) => c.id)),
    [channels],
  );
  const sourceSuggestions = useMemo(() => {
    const s = new Set<string>();
    channels.forEach((c) => c.source && s.add(c.source));
    return [...s].sort();
  }, [channels]);

  const adjustCount = (id: number, delta: number) =>
    setLists((prev) =>
      prev.map((l) =>
        l.id === id ? { ...l, channel_count: l.channel_count + delta } : l,
      ),
    );

  const handleCreate = async (name: string, description: string | null) => {
    const res = await withToast(adapter.create(name, description), {
      success: `${adapter.noun} created`,
    });
    if (!res) return false;
    await loadLists();
    setSelectedId(res.id);
    return true;
  };

  const handleSyncScanList = async () => {
    if (!selected || !adapter.syncMatchingScanList) return;
    const res = await withToast(adapter.syncMatchingScanList(selected), {
      error: "Could not sync the scan list",
    });
    if (res === undefined) return;
    toast.success(
      res.created
        ? `Scan list created (${res.added} ${res.added === 1 ? "channel" : "channels"})`
        : `Scan list updated (+${res.added}/−${res.removed})`,
    );
  };

  const handleRename = async (name: string, description: string | null) => {
    if (!selected) return false;
    const res = await withToast(adapter.rename(selected, name, description), {
      success: "Saved",
    });
    if (res === undefined) return false;
    setLists((prev) =>
      prev.map((l) => (l.id === selected.id ? { ...l, name, description } : l)),
    );
    return true;
  };

  const handleDelete = async () => {
    if (!selected) return;
    const ok = await confirmDialog(
      `Delete ${adapter.noun} “${selected.name}”? This cannot be undone.`,
      { title: `Delete ${adapter.noun}`, kind: "warning" },
    );
    if (!ok) return;
    const res = await withToast(adapter.remove(selected.id), {
      success: `${adapter.noun} deleted`,
    });
    if (res !== undefined) {
      setLists((prev) => {
        const next = prev.filter((l) => l.id !== selected.id);
        setSelectedId(next[0]?.id ?? null);
        return next;
      });
    }
  };

  const addChannel = async (channelId: number) => {
    if (!selected) return false;
    const res = await withToast(adapter.addChannel(selected.id, channelId));
    if (res === undefined) return false;
    const cs = await adapter.getChannels(selected.id);
    setChannels(cs);
    adjustCount(selected.id, 1);
    return true;
  };

  const removeChannel = async (channelId: number) => {
    if (!selected) return;
    const res = await withToast(adapter.removeChannel(selected.id, channelId));
    if (res === undefined) return;
    setChannels((prev) => prev.filter((c) => c.id !== channelId));
    adjustCount(selected.id, -1);
    // Clear a priority pointer that referenced the removed channel. This has to
    // be persisted, not just dropped from React state: the row kept the
    // reference, so the pointer reappeared on the next load and the channel
    // refused to delete with a raw "FOREIGN KEY constraint failed".
    const pointsAtRemoved =
      selected.priority_channel_id === channelId ||
      selected.priority_channel_2_id === channelId;
    if (adapter.saveScanSettings && pointsAtRemoved) {
      const cleared: ScanListSettings = {
        priority_channel_id:
          selected.priority_channel_id === channelId
            ? null
            : (selected.priority_channel_id ?? null),
        priority_channel_2_id:
          selected.priority_channel_2_id === channelId
            ? null
            : (selected.priority_channel_2_id ?? null),
        priority_select: selected.priority_select ?? 0,
        look_back_a: selected.look_back_a ?? 20,
        look_back_b: selected.look_back_b ?? 30,
        dropout_delay: selected.dropout_delay ?? 31,
        dwell_time: selected.dwell_time ?? 31,
        revert_channel: selected.revert_channel ?? 4,
      };
      const saved = await withToast(
        adapter.saveScanSettings(
          selected,
          selected.name,
          selected.description,
          cleared,
        ),
        { error: "Could not clear the priority channel" },
      );
      if (saved === undefined) return;
      setLists((prev) =>
        prev.map((l) => (l.id === selected.id ? { ...l, ...cleared } : l)),
      );
    }
  };

  const reorder = async (orderedIds: number[]) => {
    if (!selected) return;
    const prev = channels;
    const byId = new Map(prev.map((c) => [c.id, c]));
    setChannels(orderedIds.map((id) => byId.get(id)!));
    const res = await withToast(adapter.reorder(selected.id, orderedIds), {
      error: "Could not save new order",
    });
    if (res === undefined) setChannels(prev); // revert on failure
  };

  const handleSaveScanSettings = async (
    name: string,
    description: string | null,
    settings: ScanListSettings,
  ): Promise<boolean> => {
    if (!selected || !adapter.saveScanSettings) return false;
    const res = await withToast(
      adapter.saveScanSettings(selected, name, description, settings),
      { success: "Saved" },
    );
    if (res === undefined) return false;
    setLists((prev) =>
      prev.map((l) =>
        l.id === selected.id ? { ...l, name, description, ...settings } : l,
      ),
    );
    return true;
  };

  return (
    <>
      <PageHeader
        icon={adapter.titleIcon}
        title={Title}
        subtitle={`${lists.length} ${lists.length === 1 ? adapter.noun : adapter.nounPlural}`}
        actions={
          <Button variant="primary" onClick={() => setCreateOpen(true)}>
            <Plus size={14} /> New {adapter.noun.replace(/^\w/, (c) => c.toUpperCase())}
          </Button>
        }
      />

      {loading ? (
        <div className="flex flex-1 items-center justify-center">
          <Spinner className="h-6 w-6" />
        </div>
      ) : lists.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={adapter.emptyIcon}
            title={`No ${adapter.nounPlural} yet`}
            description={`Create a ${adapter.noun} and add channels from your Master Channel Table.`}
            action={
              <Button variant="primary" onClick={() => setCreateOpen(true)}>
                <Plus size={14} /> New {adapter.noun.replace(/^\w/, (c) => c.toUpperCase())}
              </Button>
            }
          />
        </div>
      ) : (
        <div className="flex min-h-0 flex-1">
          {/* Sidebar */}
          <div className="w-64 shrink-0 overflow-auto border-r border-slate-200 dark:border-slate-700">
            {lists.map((l) => (
              <button
                key={l.id}
                onClick={() => setSelectedId(l.id)}
                className={clsx(
                  "block w-full border-b border-slate-100 px-4 py-2.5 text-left dark:border-slate-700/60",
                  l.id === selectedId
                    ? "bg-sky-50 dark:bg-sky-950/40"
                    : "hover:bg-slate-50 dark:hover:bg-slate-700/40",
                )}
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate text-xs font-medium text-slate-800 dark:text-slate-100">
                    {l.name}
                  </span>
                  <Badge>{l.channel_count}</Badge>
                </div>
                {l.description && (
                  <div className="truncate text-[11px] text-slate-400">
                    {l.description}
                  </div>
                )}
              </button>
            ))}
          </div>

          {/* Detail */}
          <div className="flex min-w-0 flex-1 flex-col">
            {selected && (
              <>
                <div className="flex items-start justify-between gap-4 border-b border-slate-200 px-5 py-3 dark:border-slate-700">
                  <div className="min-w-0">
                    <h2 className="truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
                      {selected.name}
                    </h2>
                    <p className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
                      {selected.description || `${channels.length} channels`}
                    </p>
                  </div>
                  <div className="flex items-center gap-2">
                    <Button onClick={() => setPickerOpen(true)}>
                      <Plus size={14} /> Add Channels
                    </Button>
                    {adapter.syncMatchingScanList && (
                      <Button onClick={handleSyncScanList}>
                        <ListChecks size={14} /> Sync Scan List
                      </Button>
                    )}
                    <Button onClick={() => setRenameOpen(true)}>
                      <Pencil size={14} /> Edit
                    </Button>
                    <Button variant="danger" onClick={handleDelete}>
                      <Trash2 size={14} />
                    </Button>
                  </div>
                </div>

                <div className="min-h-0 flex-1 overflow-auto">
                  {channelsLoading ? (
                    <div className="flex items-center justify-center py-16">
                      <Spinner className="h-5 w-5" />
                    </div>
                  ) : channels.length === 0 ? (
                    <EmptyState
                      icon={adapter.emptyIcon}
                      title="No channels in this list"
                      description="Add channels from your Master Channel Table to build this list."
                      action={
                        <Button variant="primary" onClick={() => setPickerOpen(true)}>
                          <Plus size={14} /> Add Channels
                        </Button>
                      }
                    />
                  ) : (
                    <OrderedChannelTable
                      channels={channels}
                      onReorder={reorder}
                      onRemove={removeChannel}
                      onEdit={(c) => {
                        setEditChannel(c);
                        setEditOpen(true);
                      }}
                    />
                  )}
                </div>
              </>
            )}
          </div>
        </div>
      )}

      <ChannelPickerModal
        open={pickerOpen}
        onClose={() => setPickerOpen(false)}
        existingIds={existingIds}
        onAdd={addChannel}
      />
      <ChannelDetailPanel
        channel={editChannel}
        mode="edit"
        open={editOpen}
        onClose={() => setEditOpen(false)}
        onSaved={refreshChannels}
        // A copy made while working inside a list belongs in that list too —
        // otherwise it lands in the master table only and looks like nothing
        // happened. The panel then switches to the copy for the follow-up edit.
        onDuplicated={async (copy) => {
          setEditChannel(copy);
          await addChannel(copy.id);
        }}
        // Suggestions come from this list's channels rather than the whole
        // library — it is only autocomplete, and the master table is not loaded
        // here.
        sourceSuggestions={sourceSuggestions}
        cities={cities}
      />
      <ListMetaModal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        title={`New ${adapter.noun.replace(/^\w/, (c) => c.toUpperCase())}`}
        submitLabel="Create"
        onSubmit={handleCreate}
      />
      <ListMetaModal
        open={renameOpen}
        onClose={() => setRenameOpen(false)}
        title={`Edit ${adapter.noun.replace(/^\w/, (c) => c.toUpperCase())}`}
        initialName={selected?.name ?? ""}
        initialDescription={selected?.description ?? ""}
        submitLabel="Save"
        onSubmit={handleRename}
        scanSettings={
          adapter.saveScanSettings && selected
            ? {
                initial: {
                  priority_channel_id: selected.priority_channel_id ?? null,
                  priority_channel_2_id: selected.priority_channel_2_id ?? null,
                  priority_select: selected.priority_select ?? 0,
                  look_back_a: selected.look_back_a ?? 20,
                  look_back_b: selected.look_back_b ?? 30,
                  dropout_delay: selected.dropout_delay ?? 31,
                  dwell_time: selected.dwell_time ?? 31,
                  revert_channel: selected.revert_channel ?? 4,
                },
                memberChannels: channels.map((c) => ({
                  id: c.id,
                  name: channelLabel(c),
                })),
                onSubmitSettings: handleSaveScanSettings,
              }
            : undefined
        }
      />
    </>
  );
}
