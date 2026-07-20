/**
 * The `programming_ui` → "Program radio" dialog map (Chunk 3.7).
 *
 * This is the ONLY place a radio is named in the programming UI. Everything
 * else — which driver runs, which actions are offered — comes from the model
 * row and the driver's capability flags.
 *
 * Adding a radio: if the generic dialog covers it (identify → download backup →
 * program → verify, all from `program_radio`), add NOTHING here and leave
 * `programming_ui` at 'generic'. Only radios needing bespoke controls — the
 * TD-H3's Radio Options tab, the AnyTone's three payloads — earn a line.
 */
import type { ProgramDialog } from "../../lib/radioProgramming";
import { ProgramRadioDialog } from "./ProgramRadioDialog";
import { Tdh3ProgramDialog } from "./Tdh3ProgramDialog";
import { AnytoneProgramDialog } from "./AnytoneProgramDialog";

const PROGRAM_DIALOGS: Record<string, ProgramDialog> = {
  generic: ProgramRadioDialog,
  tdh3: Tdh3ProgramDialog,
  anytone: AnytoneProgramDialog,
};

/// Resolve a model's `programming_ui` to its dialog. An unknown or null value
/// falls back to the generic dialog rather than rendering nothing: a model with
/// a driver but no registered UI can still be programmed, and one without a
/// driver gets the generic dialog's "not supported" message.
export function programDialogFor(programmingUi: string | null): ProgramDialog {
  return (programmingUi && PROGRAM_DIALOGS[programmingUi]) || ProgramRadioDialog;
}
