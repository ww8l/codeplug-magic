import { useState } from "react";
import { Modal } from "../overlays";
import { Button, TextInput } from "../ui";
import type { ScanListSettings } from "../../lib/types";

// Priority Channel Select enum (@0x01) and Revert Channel enum (@0xF8).
// NOTE: these byte values come from qdmr and are NOT yet hardware-verified.
const PRIORITY_SELECT_OPTIONS = [
  { value: 0, label: "Off" },
  { value: 1, label: "Priority Channel 1" },
  { value: 2, label: "Priority Channel 2" },
  { value: 3, label: "Priority Channel 1 + 2" },
];
const REVERT_OPTIONS = [
  { value: 0, label: "Selected Channel" },
  { value: 1, label: "Selected + TalkBack" },
  { value: 2, label: "Priority Channel 1" },
  { value: 3, label: "Priority Channel 2" },
  { value: 4, label: "Last Called" },
  { value: 5, label: "Last Used" },
  { value: 6, label: "Priority Ch1 + TalkBack" },
  { value: 7, label: "Priority Ch2 + TalkBack" },
];

export interface ScanSettingsProps {
  initial: ScanListSettings;
  memberChannels: { id: number; name: string }[];
  onSubmitSettings: (
    name: string,
    description: string | null,
    settings: ScanListSettings,
  ) => Promise<boolean>;
}

export function ListMetaModal({
  open,
  onClose,
  title,
  initialName = "",
  initialDescription = "",
  submitLabel,
  onSubmit,
  scanSettings,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  initialName?: string;
  initialDescription?: string;
  submitLabel: string;
  // Name+description path (create, zones/groups). Optional so a scan-settings
  // modal can route submit through `scanSettings.onSubmitSettings` instead.
  onSubmit?: (name: string, description: string | null) => Promise<boolean>;
  // Present only for scan lists: renders the seven extra settings fields.
  scanSettings?: ScanSettingsProps;
}) {
  const [name, setName] = useState(initialName);
  const [description, setDescription] = useState(initialDescription);
  const [settings, setSettings] = useState<ScanListSettings | null>(
    scanSettings?.initial ?? null,
  );
  const [saving, setSaving] = useState(false);

  // Re-seed when the modal is opened fresh.
  const [wasOpen, setWasOpen] = useState(false);
  if (open && !wasOpen) {
    setName(initialName);
    setDescription(initialDescription);
    setSettings(scanSettings?.initial ?? null);
    setWasOpen(true);
  } else if (!open && wasOpen) {
    setWasOpen(false);
  }

  const submit = async () => {
    if (!name.trim()) return;
    setSaving(true);
    const ok =
      scanSettings && settings
        ? await scanSettings.onSubmitSettings(
            name.trim(),
            description.trim() || null,
            settings,
          )
        : onSubmit
          ? await onSubmit(name.trim(), description.trim() || null)
          : false;
    setSaving(false);
    if (ok) onClose();
  };

  // Timer fields are stored in 0.1-s units but edited in seconds.
  const patch = (p: Partial<ScanListSettings>) =>
    setSettings((s) => (s ? { ...s, ...p } : s));

  return (
    <Modal open={open} onClose={onClose} title={title} width="max-w-md">
      <div className="flex flex-col gap-3 p-4">
        <label className="flex flex-col gap-1">
          <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Name
          </span>
          <TextInput
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !scanSettings && submit()}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Description
          </span>
          <textarea
            className="w-full rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs text-slate-800 focus:border-sky-500 focus:outline-none focus:ring-1 focus:ring-sky-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
            rows={2}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
        </label>

        {scanSettings && settings && (
          <ScanSettingsFields
            settings={settings}
            memberChannels={scanSettings.memberChannels}
            patch={patch}
          />
        )}
      </div>
      <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" onClick={submit} disabled={saving || !name.trim()}>
          {submitLabel}
        </Button>
      </div>
    </Modal>
  );
}

const fieldLabel =
  "text-[11px] font-medium text-slate-500 dark:text-slate-400";
const selectClass =
  "w-full rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs text-slate-800 focus:border-sky-500 focus:outline-none focus:ring-1 focus:ring-sky-500 disabled:opacity-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100";

function ScanSettingsFields({
  settings,
  memberChannels,
  patch,
}: {
  settings: ScanListSettings;
  memberChannels: { id: number; name: string }[];
  patch: (p: Partial<ScanListSettings>) => void;
}) {
  const usesCh1 = settings.priority_select === 1 || settings.priority_select === 3;
  const usesCh2 = settings.priority_select === 2 || settings.priority_select === 3;

  const channelSelect = (
    value: number | null,
    onChange: (id: number | null) => void,
    disabled: boolean,
  ) => (
    <select
      className={selectClass}
      disabled={disabled}
      value={value ?? ""}
      onChange={(e) => onChange(e.target.value ? Number(e.target.value) : null)}
    >
      <option value="">— none —</option>
      {memberChannels.map((c) => (
        <option key={c.id} value={c.id}>
          {c.name}
        </option>
      ))}
    </select>
  );

  // Seconds <-> 0.1-s units. Empty/NaN clamps to 0.
  const secInput = (
    units: number,
    onChange: (units: number) => void,
  ) => (
    <TextInput
      type="number"
      min={0}
      step={0.1}
      value={units / 10}
      onChange={(e) => {
        const s = parseFloat(e.target.value);
        onChange(Number.isFinite(s) ? Math.max(0, Math.round(s * 10)) : 0);
      }}
    />
  );

  return (
    <div className="mt-1 flex flex-col gap-3 border-t border-slate-200 pt-3 dark:border-slate-700">
      <span className="text-[11px] font-semibold uppercase tracking-wide text-slate-400">
        Scan settings
      </span>

      <label className="flex flex-col gap-1">
        <span className={fieldLabel}>Priority Channel Select</span>
        <select
          className={selectClass}
          value={settings.priority_select}
          onChange={(e) => patch({ priority_select: Number(e.target.value) })}
        >
          {PRIORITY_SELECT_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </label>

      <div className="grid grid-cols-2 gap-3">
        <label className="flex flex-col gap-1">
          <span className={fieldLabel}>Priority Channel 1</span>
          {channelSelect(
            settings.priority_channel_id,
            (id) => patch({ priority_channel_id: id }),
            !usesCh1,
          )}
        </label>
        <label className="flex flex-col gap-1">
          <span className={fieldLabel}>Priority Channel 2</span>
          {channelSelect(
            settings.priority_channel_2_id,
            (id) => patch({ priority_channel_2_id: id }),
            !usesCh2,
          )}
        </label>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <label className="flex flex-col gap-1">
          <span className={fieldLabel}>Look-back Time A (s)</span>
          {secInput(settings.look_back_a, (u) => patch({ look_back_a: u }))}
        </label>
        <label className="flex flex-col gap-1">
          <span className={fieldLabel}>Look-back Time B (s)</span>
          {secInput(settings.look_back_b, (u) => patch({ look_back_b: u }))}
        </label>
        <label className="flex flex-col gap-1">
          <span className={fieldLabel}>Dropout Delay (s)</span>
          {secInput(settings.dropout_delay, (u) => patch({ dropout_delay: u }))}
        </label>
        <label className="flex flex-col gap-1">
          <span className={fieldLabel}>Dwell Time (s)</span>
          {secInput(settings.dwell_time, (u) => patch({ dwell_time: u }))}
        </label>
      </div>

      <label className="flex flex-col gap-1">
        <span className={fieldLabel}>Revert Channel</span>
        <select
          className={selectClass}
          value={settings.revert_channel}
          onChange={(e) => patch({ revert_channel: Number(e.target.value) })}
        >
          {REVERT_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}
