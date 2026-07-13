import { useCallback, useEffect, useMemo, useState } from "react";
import { Plus, SlidersHorizontal } from "lucide-react";
import clsx from "clsx";
import { api } from "../lib/api";
import type { RadioModel, RadioProfile } from "../lib/types";
import { PageHeader, Button, Spinner, EmptyState } from "../components/ui";
import { HandheldRadio } from "../components/icons/HandheldRadio";
import { NewProfileModal } from "../components/profiles/NewProfileModal";
import { ProfileEditor } from "../components/profiles/ProfileEditor";

export function RadioProfiles() {
  const [profiles, setProfiles] = useState<RadioProfile[]>([]);
  const [models, setModels] = useState<RadioModel[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [newOpen, setNewOpen] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [p, m] = await Promise.all([
        api.listRadioProfiles(),
        api.listRadioModels(),
      ]);
      setProfiles(p);
      setModels(m);
      setSelectedId((cur) =>
        cur != null && p.some((x) => x.id === cur) ? cur : (p[0]?.id ?? null),
      );
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const modelById = useMemo(() => {
    const map = new Map<number, RadioModel>();
    models.forEach((m) => map.set(m.id, m));
    return map;
  }, [models]);

  const selected = profiles.find((p) => p.id === selectedId) ?? null;

  const onCreated = (profile: RadioProfile) => {
    setProfiles((prev) =>
      [...prev, profile].sort((a, b) =>
        a.display_name.localeCompare(b.display_name),
      ),
    );
    setSelectedId(profile.id);
  };

  const onSaved = (updated: RadioProfile) => {
    setProfiles((prev) =>
      prev
        .map((p) => (p.id === updated.id ? updated : p))
        .sort((a, b) => a.display_name.localeCompare(b.display_name)),
    );
  };

  const onDeleted = (id: number) => {
    setProfiles((prev) => {
      const next = prev.filter((p) => p.id !== id);
      setSelectedId((cur) => (cur === id ? (next[0]?.id ?? null) : cur));
      return next;
    });
  };

  return (
    <>
      <PageHeader
        icon={<HandheldRadio size={18} />}
        title="Radios"
        subtitle={`Your instances of radio models · ${profiles.length} ${
          profiles.length === 1 ? "profile" : "profiles"
        }`}
        actions={
          <Button variant="primary" onClick={() => setNewOpen(true)}>
            <Plus size={14} /> New Profile
          </Button>
        }
      />

      {loading ? (
        <div className="flex flex-1 items-center justify-center">
          <Spinner className="h-6 w-6" />
        </div>
      ) : profiles.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<SlidersHorizontal size={40} strokeWidth={1.5} />}
            title="No radio profiles yet"
            description="Create a profile from one of the built-in radio models to configure its non-channel settings and use it in codeplugs."
            action={
              <Button variant="primary" onClick={() => setNewOpen(true)}>
                <Plus size={14} /> New Profile
              </Button>
            }
          />
        </div>
      ) : (
        <div className="flex min-h-0 flex-1">
          {/* Profile list */}
          <div className="w-64 shrink-0 overflow-auto border-r border-slate-200 dark:border-slate-700">
            {profiles.map((p) => {
              const m = modelById.get(p.radio_model_id);
              return (
                <button
                  key={p.id}
                  onClick={() => setSelectedId(p.id)}
                  className={clsx(
                    "block w-full border-b border-slate-100 px-4 py-2.5 text-left dark:border-slate-700/60",
                    p.id === selectedId
                      ? "bg-sky-50 dark:bg-sky-950/40"
                      : "hover:bg-slate-50 dark:hover:bg-slate-700/40",
                  )}
                >
                  <div className="truncate text-xs font-medium text-slate-800 dark:text-slate-100">
                    {p.display_name}
                  </div>
                  <div className="truncate text-[11px] text-slate-400">
                    {m?.display_name ?? "Unknown model"}
                  </div>
                </button>
              );
            })}
          </div>

          {/* Editor */}
          <div className="min-w-0 flex-1">
            {selected && (
              <ProfileEditor
                key={selected.id}
                profile={selected}
                model={modelById.get(selected.radio_model_id)}
                onSaved={onSaved}
                onDeleted={onDeleted}
              />
            )}
          </div>
        </div>
      )}

      <NewProfileModal
        open={newOpen}
        onClose={() => setNewOpen(false)}
        models={models}
        onCreated={onCreated}
      />
    </>
  );
}
