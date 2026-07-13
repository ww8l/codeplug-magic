import { useState } from "react";
import { api, withToast } from "../../lib/api";
import type { Codeplug, RadioProfile } from "../../lib/types";
import { Modal } from "../overlays";
import { Button, TextInput, Select } from "../ui";

export function NewCodeplugModal({
  open,
  onClose,
  profiles,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  profiles: RadioProfile[];
  onCreated: (codeplug: Codeplug) => void;
}) {
  const [name, setName] = useState("");
  const [profileId, setProfileId] = useState<number | null>(null);
  const [saving, setSaving] = useState(false);

  const [wasOpen, setWasOpen] = useState(false);
  if (open && !wasOpen) {
    setName("");
    setProfileId(null);
    setWasOpen(true);
  } else if (!open && wasOpen) {
    setWasOpen(false);
  }

  const create = async () => {
    if (!name.trim()) return;
    setSaving(true);
    const cp = await withToast(
      api.createCodeplug({
        name: name.trim(),
        radio_profile_id: profileId,
        notes: null,
      }),
      { success: "Codeplug created" },
    );
    setSaving(false);
    if (cp) {
      onCreated(cp);
      onClose();
    }
  };

  return (
    <Modal open={open} onClose={onClose} title="New Codeplug" width="max-w-md">
      <div className="flex flex-col gap-3 p-4">
        <label className="flex flex-col gap-1">
          <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Name
          </span>
          <TextInput
            autoFocus
            placeholder="e.g. W0CPH-AT878-Truck"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && create()}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Radio Profile
          </span>
          <Select
            value={profileId ?? ""}
            onChange={(e) =>
              setProfileId(e.target.value ? Number(e.target.value) : null)
            }
          >
            <option value="">No profile (assign later)</option>
            {profiles.map((p) => (
              <option key={p.id} value={p.id}>
                {p.display_name}
              </option>
            ))}
          </Select>
          {profiles.length === 0 && (
            <span className="text-[11px] text-amber-600 dark:text-amber-400">
              No radio profiles yet — create one under Radio Profiles to enable
              export.
            </span>
          )}
        </label>
      </div>
      <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" onClick={create} disabled={saving || !name.trim()}>
          Create Codeplug
        </Button>
      </div>
    </Modal>
  );
}
