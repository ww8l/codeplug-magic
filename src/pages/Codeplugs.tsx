import { useCallback, useEffect, useMemo, useState } from "react";
import { confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import {
  Package,
  Plus,
  Download,
  Trash2,
  ArrowLeft,
  List,
  ScanLine,
  ChevronUp,
  ChevronDown,
  X,
  RadioTower,
  ListChecks,
  Pencil,
} from "lucide-react";
import { api, withToast } from "../lib/api";
import type {
  ChannelListSummary,
  Codeplug,
  CodeplugSummary,
  RadioModel,
  RadioProfile,
  ScanListSummary,
} from "../lib/types";
import { modelBands, modelModes } from "../lib/profiles";
import {
  PageHeader,
  Button,
  Card,
  Select,
  Spinner,
  EmptyState,
  Badge,
} from "../components/ui";
import { NewCodeplugModal } from "../components/codeplugs/NewCodeplugModal";
import { EditCodeplugModal } from "../components/codeplugs/EditCodeplugModal";
import { ExportDialog } from "../components/codeplugs/ExportDialog";
import { ProgramRadioDialog } from "../components/codeplugs/ProgramRadioDialog";
import { Tdh3ProgramDialog } from "../components/codeplugs/Tdh3ProgramDialog";
import { AnytoneProgramDialog } from "../components/codeplugs/AnytoneProgramDialog";
import { AssignModal, type AssignItem } from "../components/codeplugs/AssignModal";
import { ScanListAssignChannelsModal } from "../components/codeplugs/ScanListAssignChannelsModal";

// ============================================================
// Codeplug grid
// ============================================================
export function Codeplugs() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [codeplugs, setCodeplugs] = useState<CodeplugSummary[]>([]);
  const [profiles, setProfiles] = useState<RadioProfile[]>([]);
  const [loading, setLoading] = useState(true);
  const [newOpen, setNewOpen] = useState(false);

  useEffect(() => {
    Promise.all([api.listCodeplugs(), api.listRadioProfiles()])
      .then(([c, p]) => {
        setCodeplugs(c);
        setProfiles(p);
      })
      .finally(() => setLoading(false));
  }, []);

  // Auto-open the New Codeplug modal when navigated from the dashboard.
  useEffect(() => {
    if (searchParams.get("new") === "1") {
      setNewOpen(true);
      searchParams.delete("new");
      setSearchParams(searchParams, { replace: true });
    }
  }, [searchParams, setSearchParams]);

  return (
    <>
      <PageHeader
        icon={<Package size={18} />}
        title="Codeplugs"
        subtitle={`Saved per-radio configurations · ${codeplugs.length}`}
        actions={
          <Button variant="primary" onClick={() => setNewOpen(true)}>
            <Plus size={14} /> New Codeplug
          </Button>
        }
      />

      {loading ? (
        <div className="flex flex-1 items-center justify-center">
          <Spinner className="h-6 w-6" />
        </div>
      ) : codeplugs.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<Package size={40} strokeWidth={1.5} />}
            title="No codeplugs yet"
            description="Create a codeplug, assign a radio profile and channel lists, then export a ready-to-load file for your radio."
            action={
              <Button variant="primary" onClick={() => setNewOpen(true)}>
                <Plus size={14} /> New Codeplug
              </Button>
            }
          />
        </div>
      ) : (
        <div className="flex-1 overflow-auto p-5">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {codeplugs.map((cp) => (
              <Card
                key={cp.id}
                className="cursor-pointer p-4 transition-colors hover:border-sky-300 dark:hover:border-sky-700"
              >
                <button
                  onClick={() => navigate(`/codeplugs/${cp.id}`)}
                  className="block w-full text-left"
                >
                  <div className="flex items-start justify-between gap-2">
                    <h3 className="truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
                      {cp.name}
                    </h3>
                    <Package size={16} className="shrink-0 text-slate-300" />
                  </div>
                  <p className="mt-1 truncate text-xs text-slate-500 dark:text-slate-400">
                    {cp.model_display_name ?? "No radio profile"}
                  </p>
                  <div className="mt-3 flex items-center justify-between text-[11px] text-slate-400">
                    <span>{cp.channel_count} channels</span>
                    <span>
                      {cp.last_exported
                        ? `${
                            cp.last_export_kind === "radio"
                              ? "Programmed"
                              : "Exported"
                          } ${cp.last_exported.slice(0, 10)}`
                        : "Never exported"}
                    </span>
                  </div>
                </button>
              </Card>
            ))}
          </div>
        </div>
      )}

      <NewCodeplugModal
        open={newOpen}
        onClose={() => setNewOpen(false)}
        profiles={profiles}
        onCreated={(cp) => navigate(`/codeplugs/${cp.id}`)}
      />
    </>
  );
}

// ============================================================
// Codeplug detail
// ============================================================
function SectionCard({
  title,
  icon,
  action,
  children,
}: {
  title: string;
  icon: React.ReactNode;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <Card>
      <div className="flex items-center justify-between border-b border-slate-200 px-4 py-2.5 dark:border-slate-700">
        <h3 className="flex items-center gap-1.5 text-xs font-semibold text-slate-700 dark:text-slate-200">
          {icon}
          {title}
        </h3>
        {action}
      </div>
      {children}
    </Card>
  );
}

export function CodeplugDetail() {
  const { id } = useParams();
  const codeplugId = Number(id);
  const navigate = useNavigate();

  const [codeplug, setCodeplug] = useState<Codeplug | null>(null);
  const [profiles, setProfiles] = useState<RadioProfile[]>([]);
  const [models, setModels] = useState<RadioModel[]>([]);
  const [channelLists, setChannelLists] = useState<ChannelListSummary[]>([]);
  const [scanLists, setScanLists] = useState<ScanListSummary[]>([]);
  const [allChannelLists, setAllChannelLists] = useState<ChannelListSummary[]>([]);
  const [allScanLists, setAllScanLists] = useState<ScanListSummary[]>([]);
  const [loading, setLoading] = useState(true);

  const [profileId, setProfileId] = useState<number | null>(null);
  const [savingProfile, setSavingProfile] = useState(false);

  const [exportOpen, setExportOpen] = useState(false);
  const [programOpen, setProgramOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [addChannelOpen, setAddChannelOpen] = useState(false);
  const [addScanOpen, setAddScanOpen] = useState(false);
  const [assignScanList, setAssignScanList] = useState<ScanListSummary | null>(
    null,
  );

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [cp, p, m, cls, sls, allCl, allSl] = await Promise.all([
        api.getCodeplug(codeplugId),
        api.listRadioProfiles(),
        api.listRadioModels(),
        api.getCodeplugChannelLists(codeplugId),
        api.getCodeplugScanLists(codeplugId),
        api.listChannelLists(),
        api.listScanLists(),
      ]);
      setCodeplug(cp);
      setProfiles(p);
      setModels(m);
      setChannelLists(cls);
      setScanLists(sls);
      setAllChannelLists(allCl);
      setAllScanLists(allSl);
      setProfileId(cp.radio_profile_id);
    } finally {
      setLoading(false);
    }
  }, [codeplugId]);

  useEffect(() => {
    load();
  }, [load]);

  const model = useMemo(() => {
    const prof = profiles.find((p) => p.id === profileId);
    return prof ? models.find((m) => m.id === prof.radio_model_id) : undefined;
  }, [profiles, models, profileId]);

  const totalChannels = useMemo(
    () => channelLists.reduce((sum, l) => sum + l.channel_count, 0),
    [channelLists],
  );

  if (loading) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Spinner className="h-6 w-6" />
      </div>
    );
  }

  if (!codeplug) {
    return (
      <EmptyState
        icon={<Package size={40} strokeWidth={1.5} />}
        title="Codeplug not found"
        action={
          <Button onClick={() => navigate("/codeplugs")}>Back to Codeplugs</Button>
        }
      />
    );
  }

  const saveProfile = async (newId: number | null) => {
    setProfileId(newId);
    setSavingProfile(true);
    const res = await withToast(
      api.updateCodeplug(codeplugId, {
        name: codeplug.name,
        radio_profile_id: newId,
        notes: codeplug.notes,
      }),
      { success: "Radio profile updated" },
    );
    setSavingProfile(false);
    if (res) setCodeplug(res);
    else setProfileId(codeplug.radio_profile_id); // revert on failure
  };

  const deleteCodeplug = async () => {
    const ok = await confirmDialog(
      `Delete codeplug “${codeplug.name}”? This cannot be undone.`,
      { title: "Delete codeplug", kind: "warning" },
    );
    if (!ok) return;
    const res = await withToast(api.deleteCodeplug(codeplugId), {
      success: "Codeplug deleted",
    });
    if (res !== undefined) navigate("/codeplugs");
  };

  // ---- channel-list assignment ----
  const addChannelList = async (listId: number) => {
    const res = await withToast(
      api.addChannelListToCodeplug(codeplugId, listId),
    );
    if (res === undefined) return false;
    setChannelLists(await api.getCodeplugChannelLists(codeplugId));
    return true;
  };

  const removeChannelList = async (listId: number) => {
    const res = await withToast(
      api.removeChannelListFromCodeplug(codeplugId, listId),
    );
    if (res === undefined) return;
    setChannelLists((prev) => prev.filter((l) => l.id !== listId));
  };

  const moveChannelList = async (index: number, dir: -1 | 1) => {
    const target = index + dir;
    if (target < 0 || target >= channelLists.length) return;
    const next = [...channelLists];
    [next[index], next[target]] = [next[target], next[index]];
    const prev = channelLists;
    setChannelLists(next);
    const res = await withToast(
      api.reorderCodeplugChannelLists(
        codeplugId,
        next.map((l) => l.id),
      ),
      { error: "Could not save order" },
    );
    if (res === undefined) setChannelLists(prev);
  };

  // ---- scan-list assignment ----
  const addScanList = async (listId: number) => {
    const res = await withToast(api.addScanListToCodeplug(codeplugId, listId));
    if (res === undefined) return false;
    setScanLists(await api.getCodeplugScanLists(codeplugId));
    return true;
  };

  const removeScanList = async (listId: number) => {
    const res = await withToast(
      api.removeScanListFromCodeplug(codeplugId, listId),
    );
    if (res === undefined) return;
    setScanLists((prev) => prev.filter((l) => l.id !== listId));
  };

  const availableChannelLists: AssignItem[] = allChannelLists
    .filter((l) => !channelLists.some((a) => a.id === l.id))
    .map((l) => ({
      id: l.id,
      name: l.name,
      subtitle: l.description,
      count: l.channel_count,
    }));

  const availableScanLists: AssignItem[] = allScanLists
    .filter((l) => !scanLists.some((a) => a.id === l.id))
    .map((l) => ({
      id: l.id,
      name: l.name,
      subtitle: l.description,
      count: l.channel_count,
    }));

  return (
    <>
      <PageHeader
        title={codeplug.name}
        subtitle={`${model?.display_name ?? "No radio profile"} · ${totalChannels} channels`}
        actions={
          <>
            <Button onClick={() => navigate("/codeplugs")}>
              <ArrowLeft size={14} /> Back
            </Button>
            <Button onClick={() => setEditOpen(true)}>
              <Pencil size={14} /> Edit
            </Button>
            <Button variant="danger" onClick={deleteCodeplug}>
              <Trash2 size={14} />
            </Button>
            <Button
              onClick={() => setExportOpen(true)}
              disabled={profileId == null}
              title={profileId == null ? "Assign a radio profile first" : undefined}
            >
              <Download size={14} /> Export
            </Button>
            <Button
              variant="primary"
              onClick={() => setProgramOpen(true)}
              disabled={profileId == null}
              title={
                profileId == null
                  ? "Assign a radio profile first"
                  : "Program the radio directly over the cable"
              }
            >
              <RadioTower size={14} /> Program Radio
            </Button>
          </>
        }
      />

      <div className="flex-1 overflow-auto p-5">
        <div className="mx-auto grid max-w-4xl grid-cols-1 gap-4 lg:grid-cols-2">
          {/* Radio profile + capability summary */}
          <SectionCard title="Radio Profile" icon={<Package size={13} />}>
            <div className="space-y-3 p-4">
              <Select
                className="w-full"
                value={profileId ?? ""}
                disabled={savingProfile}
                onChange={(e) =>
                  saveProfile(e.target.value ? Number(e.target.value) : null)
                }
              >
                <option value="">No profile (required to export)</option>
                {profiles.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.display_name}
                  </option>
                ))}
              </Select>

              {model ? (
                <div className="space-y-2 rounded-md bg-slate-50 p-3 text-xs dark:bg-slate-900/60">
                  <div className="flex flex-wrap gap-1">
                    {modelBands(model).map((b) => (
                      <Badge key={b}>{b}</Badge>
                    ))}
                  </div>
                  <div className="text-slate-500 dark:text-slate-400">
                    Modes: {modelModes(model).join(", ")}
                  </div>
                  <div className="text-slate-500 dark:text-slate-400">
                    {model.memory_channels ?? "?"} memory channels · max name{" "}
                    {model.max_name_length ?? "?"} chars
                  </div>
                </div>
              ) : (
                <p className="text-[11px] text-amber-600 dark:text-amber-400">
                  Assign a radio profile to enable export and capability
                  filtering.
                </p>
              )}
            </div>
          </SectionCard>

          {/* Scan lists — only for radios that support them */}
          {(!model || model.scan_lists_supported) && (
          <SectionCard
            title="Scan Lists"
            icon={<ScanLine size={13} />}
            action={
              <Button onClick={() => setAddScanOpen(true)}>
                <Plus size={13} /> Add
              </Button>
            }
          >
            {scanLists.length === 0 ? (
              <p className="px-4 py-6 text-center text-xs text-slate-400">
                No scan lists assigned.
              </p>
            ) : (
              <ul className="divide-y divide-slate-100 dark:divide-slate-700/60">
                {scanLists.map((l) => (
                  <li
                    key={l.id}
                    className="flex items-center justify-between gap-2 px-4 py-2 text-xs"
                  >
                    <span className="flex items-center gap-1.5 truncate font-medium text-slate-700 dark:text-slate-200">
                      {l.name}
                      <Badge>{l.channel_count}</Badge>
                    </span>
                    <div className="flex items-center gap-0.5">
                      <button
                        onClick={() => setAssignScanList(l)}
                        title="Assign channels to this scan list"
                        className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-700 dark:hover:bg-slate-600 dark:hover:text-slate-200"
                      >
                        <ListChecks size={14} />
                      </button>
                      <button
                        onClick={() => removeScanList(l.id)}
                        className="rounded p-0.5 text-slate-400 hover:bg-red-100 hover:text-red-600 dark:hover:bg-red-950/50"
                      >
                        <X size={14} />
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </SectionCard>
          )}

          {/* Channel lists — full width */}
          <div className="lg:col-span-2">
            <SectionCard
              title="Channel Lists"
              icon={<List size={13} />}
              action={
                <Button onClick={() => setAddChannelOpen(true)}>
                  <Plus size={13} /> Add
                </Button>
              }
            >
              {channelLists.length === 0 ? (
                <p className="px-4 py-6 text-center text-xs text-slate-400">
                  No channel lists assigned. Add channel lists to include their
                  channels in this codeplug. Order determines memory-channel
                  ordering on export.
                </p>
              ) : (
                <ul className="divide-y divide-slate-100 dark:divide-slate-700/60">
                  {channelLists.map((l, i) => (
                    <li
                      key={l.id}
                      className="flex items-center justify-between gap-2 px-4 py-2 text-xs"
                    >
                      <span className="flex items-center gap-2 truncate">
                        <span className="tabular-nums text-slate-400">
                          {i + 1}
                        </span>
                        <span className="truncate font-medium text-slate-700 dark:text-slate-200">
                          {l.name}
                        </span>
                        <Badge>{l.channel_count}</Badge>
                      </span>
                      <div className="flex items-center gap-0.5">
                        <button
                          onClick={() => moveChannelList(i, -1)}
                          disabled={i === 0}
                          className="rounded p-0.5 text-slate-400 hover:bg-slate-200 disabled:opacity-30 dark:hover:bg-slate-600"
                        >
                          <ChevronUp size={14} />
                        </button>
                        <button
                          onClick={() => moveChannelList(i, 1)}
                          disabled={i === channelLists.length - 1}
                          className="rounded p-0.5 text-slate-400 hover:bg-slate-200 disabled:opacity-30 dark:hover:bg-slate-600"
                        >
                          <ChevronDown size={14} />
                        </button>
                        <button
                          onClick={() => removeChannelList(l.id)}
                          className="ml-1 rounded p-0.5 text-slate-400 hover:bg-red-100 hover:text-red-600 dark:hover:bg-red-950/50"
                        >
                          <X size={14} />
                        </button>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </SectionCard>
          </div>
        </div>
      </div>

      <EditCodeplugModal
        open={editOpen}
        onClose={() => setEditOpen(false)}
        codeplug={codeplug}
        onSaved={setCodeplug}
      />
      <ExportDialog
        open={exportOpen}
        onClose={() => setExportOpen(false)}
        codeplugId={codeplugId}
        codeplugName={codeplug.name}
        onExported={() =>
          api.getCodeplug(codeplugId).then(setCodeplug)
        }
      />
      {model?.model === "TD-H3" ? (
        <Tdh3ProgramDialog
          open={programOpen}
          onClose={() => setProgramOpen(false)}
          codeplugId={codeplugId}
          codeplugName={codeplug.name}
          modelName={model?.display_name ?? "this radio"}
          modelId={model?.id ?? 0}
        />
      ) : model?.model === "AT-D890UV" ? (
        <AnytoneProgramDialog
          open={programOpen}
          onClose={() => setProgramOpen(false)}
          codeplugId={codeplugId}
          codeplugName={codeplug.name}
          modelName={model?.display_name ?? "this radio"}
        />
      ) : (
        <ProgramRadioDialog
          open={programOpen}
          onClose={() => setProgramOpen(false)}
          codeplugId={codeplugId}
          codeplugName={codeplug.name}
          isSupported={model?.model === "UV-5R"}
          modelName={model?.display_name ?? "this radio"}
        />
      )}
      <AssignModal
        open={addChannelOpen}
        onClose={() => setAddChannelOpen(false)}
        title="Add Channel Lists"
        items={availableChannelLists}
        emptyText="All channel lists are already assigned (or none exist yet)."
        onAdd={addChannelList}
      />
      <AssignModal
        open={addScanOpen}
        onClose={() => setAddScanOpen(false)}
        title="Add Scan Lists"
        items={availableScanLists}
        emptyText="All scan lists are already assigned (or none exist yet)."
        onAdd={addScanList}
      />
      <ScanListAssignChannelsModal
        open={assignScanList !== null}
        onClose={() => setAssignScanList(null)}
        codeplugId={codeplugId}
        scanList={assignScanList}
        channelListIds={channelLists.map((l) => l.id)}
        scanLists={scanLists}
        onChanged={() => {}}
      />
    </>
  );
}
