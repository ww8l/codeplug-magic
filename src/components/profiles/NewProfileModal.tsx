import { useMemo, useState } from "react";
import { Search, Radio, Check } from "lucide-react";
import clsx from "clsx";
import { api, withToast } from "../../lib/api";
import type { RadioModel, RadioProfile } from "../../lib/types";
import { modelBands, modelModes } from "../../lib/profiles";
import { Modal } from "../overlays";
import { Button, TextInput, Badge } from "../ui";

export function NewProfileModal({
  open,
  onClose,
  models,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  models: RadioModel[];
  onCreated: (profile: RadioProfile) => void;
}) {
  const [search, setSearch] = useState("");
  const [modelId, setModelId] = useState<number | null>(null);
  const [name, setName] = useState("");
  const [saving, setSaving] = useState(false);

  // Reset local state each time the modal is freshly opened.
  const [wasOpen, setWasOpen] = useState(false);
  if (open && !wasOpen) {
    setSearch("");
    setModelId(null);
    setName("");
    setWasOpen(true);
  } else if (!open && wasOpen) {
    setWasOpen(false);
  }

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return models;
    return models.filter((m) =>
      `${m.manufacturer} ${m.model} ${m.display_name}`.toLowerCase().includes(q),
    );
  }, [models, search]);

  const pick = (m: RadioModel) => {
    setModelId(m.id);
    // Suggest a starting name based on the model, only if untouched.
    if (!name.trim()) setName(`My ${m.display_name}`);
  };

  const create = async () => {
    if (modelId == null) return;
    if (!name.trim()) return;
    setSaving(true);
    const profile = await withToast(
      api.createRadioProfile({
        display_name: name.trim(),
        radio_model_id: modelId,
        non_channel_settings: null,
        notes: null,
      }),
      { success: "Profile created" },
    );
    setSaving(false);
    if (profile) {
      onCreated(profile);
      onClose();
    }
  };

  return (
    <Modal open={open} onClose={onClose} title="New Radio Profile" width="max-w-2xl">
      <div className="flex flex-col gap-4 overflow-hidden p-4">
        <div className="relative">
          <Search
            size={13}
            className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-slate-400"
          />
          <TextInput
            autoFocus
            className="w-full pl-7"
            placeholder="Search radio models…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>

        <div className="max-h-72 overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
          {filtered.length === 0 ? (
            <div className="px-3 py-6 text-center text-xs text-slate-400">
              No models match “{search}”.
            </div>
          ) : (
            filtered.map((m) => (
              <button
                key={m.id}
                onClick={() => pick(m)}
                className={clsx(
                  "flex w-full items-center justify-between gap-3 border-b border-slate-100 px-3 py-2 text-left last:border-b-0 hover:bg-slate-50 dark:border-slate-700/60 dark:hover:bg-slate-700/40",
                  modelId === m.id && "bg-sky-50 dark:bg-sky-950/40",
                )}
              >
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5 text-xs font-medium text-slate-800 dark:text-slate-100">
                    <Radio size={13} className="shrink-0 text-slate-400" />
                    {m.display_name}
                  </div>
                  <div className="mt-1 flex flex-wrap gap-1">
                    {modelBands(m).map((b) => (
                      <Badge key={b}>{b}</Badge>
                    ))}
                    <span className="text-[11px] text-slate-400">
                      {modelModes(m).join(" · ")}
                    </span>
                  </div>
                </div>
                {modelId === m.id && (
                  <Check size={16} className="shrink-0 text-sky-600" />
                )}
              </button>
            ))
          )}
        </div>

        <label className="flex flex-col gap-1">
          <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Profile name
          </span>
          <TextInput
            placeholder="e.g. My Baofeng UV-5R"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </label>
      </div>

      <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button
          variant="primary"
          onClick={create}
          disabled={saving || modelId == null || !name.trim()}
        >
          Create Profile
        </Button>
      </div>
    </Modal>
  );
}
