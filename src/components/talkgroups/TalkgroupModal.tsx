import { useMemo, useState } from "react";
import { X } from "lucide-react";
import { api, withToast } from "../../lib/api";
import type { Talkgroup, TalkgroupInput } from "../../lib/types";
import { DMR_NETWORKS, CALL_TYPES, presentFacet } from "../../lib/constants";
import { Button, TextInput, Select } from "../ui";
import { Modal } from "../overlays";

const EMPTY: TalkgroupInput = {
  tg_number: 0,
  name: "",
  network: DMR_NETWORKS[0],
  call_type: CALL_TYPES[0],
  notes: null,
};

// Sentinel option that switches the network picker into free-text entry.
const CUSTOM_NETWORK = "__custom__";

/// Create or edit one talkgroup. Shared by the Talkgroups page and the
/// channel detail panel, so a missing talkgroup can be added without leaving
/// the channel you're editing.
export function TalkgroupModal({
  open,
  onClose,
  title,
  existing,
  networks,
  initialNumber,
  onSaved,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  existing?: Talkgroup | null;
  // Networks already present in the library, merged with the canonical list.
  networks: string[];
  // Prefills the number field when creating (the number typed into a picker).
  initialNumber?: string;
  onSaved: (tg: Talkgroup) => void;
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
      setTgText(initialNumber ?? "");
      setCustomNetwork(false);
    }
    setWasOpen(true);
  } else if (!open && wasOpen) {
    setWasOpen(false);
  }

  // With the number prefilled from a picker, the name is what's left to type —
  // put the cursor there instead of on the number.
  const focusName = !existing && !!initialNumber;

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
    if (res) onSaved(res);
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
              autoFocus={!focusName}
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
              autoFocus={focusName}
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
