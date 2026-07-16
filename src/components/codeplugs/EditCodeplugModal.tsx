import { useState } from "react";
import { api, withToast } from "../../lib/api";
import type { Codeplug } from "../../lib/types";
import { Modal } from "../overlays";
import { Button, TextInput } from "../ui";

export function EditCodeplugModal({
  open,
  onClose,
  codeplug,
  onSaved,
}: {
  open: boolean;
  onClose: () => void;
  codeplug: Codeplug;
  onSaved: (codeplug: Codeplug) => void;
}) {
  const [name, setName] = useState(codeplug.name);
  const [notes, setNotes] = useState(codeplug.notes ?? "");
  const [saving, setSaving] = useState(false);

  // Re-seed when the modal is opened fresh.
  const [wasOpen, setWasOpen] = useState(false);
  if (open && !wasOpen) {
    setName(codeplug.name);
    setNotes(codeplug.notes ?? "");
    setWasOpen(true);
  } else if (!open && wasOpen) {
    setWasOpen(false);
  }

  const save = async () => {
    if (!name.trim()) return;
    setSaving(true);
    const cp = await withToast(
      api.updateCodeplug(codeplug.id, {
        name: name.trim(),
        // The radio profile has its own control on the detail page; preserve it.
        radio_profile_id: codeplug.radio_profile_id,
        notes: notes.trim() || null,
      }),
      { success: "Codeplug updated" },
    );
    setSaving(false);
    if (cp) {
      onSaved(cp);
      onClose();
    }
  };

  return (
    <Modal open={open} onClose={onClose} title="Edit Codeplug" width="max-w-md">
      <div className="flex flex-col gap-3 p-4">
        <label className="flex flex-col gap-1">
          <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Name
          </span>
          <TextInput
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && save()}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Notes
          </span>
          <textarea
            className="w-full rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs text-slate-800 focus:border-sky-500 focus:outline-none focus:ring-1 focus:ring-sky-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
            rows={2}
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
          />
        </label>
      </div>
      <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" onClick={save} disabled={saving || !name.trim()}>
          Save
        </Button>
      </div>
    </Modal>
  );
}
