import { useMemo } from "react";
import { AlertTriangle, RotateCcw } from "lucide-react";
import type { ColumnMapping, CsvInspection, CsvMappableField } from "../../lib/types";
import { Button, Select } from "../ui";

// The order groups appear in. A field whose group is not listed lands at the
// end, so adding one to the Rust catalogue can never make it invisible here.
const GROUP_ORDER = [
  "Identity",
  "Frequency",
  "Tone",
  "Signalling",
  "Location",
  "Links",
  "Other",
];

// A one-line hint of what a column must look like, next to the field name.
const KIND_HINT: Record<CsvMappableField["kind"], string> = {
  text: "",
  freq: "MHz",
  tone: "Hz",
  dcs: "octal",
  number: "number",
  enum: "",
};

/**
 * Tie a CSV's columns to channel fields (issue #115).
 *
 * The guess arrives from the backend and is already applied; everything here
 * exists so the operator can correct it. Corrections matter more than the guess
 * does — a wrong column silently programs a radio with the wrong frequency —
 * so each row shows real values from the chosen column rather than only its
 * header, and a column used twice is called out rather than quietly accepted.
 */
export function ColumnMapper({
  inspection,
  mapping,
  onChange,
}: {
  inspection: CsvInspection;
  mapping: ColumnMapping;
  onChange: (next: ColumnMapping) => void;
}) {
  const groups = useMemo(() => {
    const byGroup = new Map<string, CsvMappableField[]>();
    for (const f of inspection.fields) {
      const list = byGroup.get(f.group) ?? [];
      list.push(f);
      byGroup.set(f.group, list);
    }
    return [...byGroup.entries()].sort(
      (a, b) =>
        (GROUP_ORDER.indexOf(a[0]) + 1 || 99) - (GROUP_ORDER.indexOf(b[0]) + 1 || 99),
    );
  }, [inspection.fields]);

  // Column index -> how many fields are pointed at it. Two fields reading one
  // column is legal but almost always a mistake, and worth saying so.
  const useCount = useMemo(() => {
    const counts = new Map<number, number>();
    for (const i of Object.values(mapping)) counts.set(i, (counts.get(i) ?? 0) + 1);
    return counts;
  }, [mapping]);

  const set = (key: string, value: string) => {
    const next = { ...mapping };
    if (value === "") delete next[key];
    else next[key] = Number(value);
    onChange(next);
  };

  const unmapped = inspection.columns.filter((c) => !useCount.has(c.index));

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 pb-3">
        <p className="text-xs text-slate-500 dark:text-slate-400">
          {inspection.recognized
            ? `This is a ${inspection.recognized}, which has its own importer — mapping the columns yourself overrides it.`
            : "This CSV isn't a RepeaterBook export, so its columns were matched to channel fields by name."}{" "}
          Check them — anything left as{" "}
          <span className="italic">Not imported</span> is skipped.
        </p>
        <Button onClick={() => onChange(inspection.guess)}>
          <RotateCcw size={14} /> Reset to guess
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
        <table className="w-full text-left text-xs">
          <thead className="sticky top-0 z-10 bg-slate-100 text-[10px] uppercase tracking-wide text-slate-500 dark:bg-slate-900 dark:text-slate-400">
            <tr>
              <th className="w-1/3 px-3 py-1.5 font-semibold">Channel field</th>
              <th className="w-1/4 px-3 py-1.5 font-semibold">CSV column</th>
              <th className="px-3 py-1.5 font-semibold">Values in that column</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
            {groups.map(([group, fields]) => (
              <FieldGroup
                key={group}
                group={group}
                fields={fields}
                columns={inspection.columns}
                mapping={mapping}
                useCount={useCount}
                onSet={set}
              />
            ))}
          </tbody>
        </table>
      </div>

      {unmapped.length > 0 && (
        <p className="shrink-0 pt-2 text-[11px] text-slate-400 dark:text-slate-500">
          Not imported: {unmapped.map((c) => c.header || `column ${c.index + 1}`).join(", ")}
        </p>
      )}
    </div>
  );
}

function FieldGroup({
  group,
  fields,
  columns,
  mapping,
  useCount,
  onSet,
}: {
  group: string;
  fields: CsvMappableField[];
  columns: CsvInspection["columns"];
  mapping: ColumnMapping;
  useCount: Map<number, number>;
  onSet: (key: string, value: string) => void;
}) {
  return (
    <>
      <tr className="bg-slate-50 dark:bg-slate-800/60">
        <td
          colSpan={3}
          className="px-3 py-1 text-[10px] font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400"
        >
          {group}
        </td>
      </tr>
      {fields.map((f) => {
        const chosen = mapping[f.key];
        const column = chosen != null ? columns[chosen] : undefined;
        const shared = chosen != null && (useCount.get(chosen) ?? 0) > 1;
        const missing = f.required && chosen == null;
        return (
          <tr key={f.key} className="align-top">
            <td className="px-3 py-1.5">
              <div className="font-medium text-slate-700 dark:text-slate-200">
                {f.label}
                {f.required && <span className="ml-1 text-red-500">*</span>}
                {KIND_HINT[f.kind] && (
                  <span className="ml-1.5 font-normal text-slate-400">
                    ({KIND_HINT[f.kind]})
                  </span>
                )}
              </div>
              {f.help && (
                <div className="text-[11px] text-slate-400 dark:text-slate-500">{f.help}</div>
              )}
            </td>
            <td className="px-3 py-1.5">
              <Select
                className="w-full"
                value={chosen ?? ""}
                onChange={(e) => onSet(f.key, e.target.value)}
              >
                <option value="">Not imported</option>
                {columns.map((c) => (
                  <option key={c.index} value={c.index}>
                    {c.header || `Column ${c.index + 1}`}
                  </option>
                ))}
              </Select>
              {missing && (
                <div className="mt-1 flex items-center gap-1 text-[11px] text-red-500">
                  <AlertTriangle size={11} /> Required
                </div>
              )}
              {shared && (
                <div className="mt-1 flex items-center gap-1 text-[11px] text-amber-600 dark:text-amber-400">
                  <AlertTriangle size={11} /> Also used by another field
                </div>
              )}
            </td>
            <td className="px-3 py-1.5 font-mono text-[11px] text-slate-500 dark:text-slate-400">
              {column
                ? column.samples.length > 0
                  ? column.samples.join(" · ")
                  : "(empty in every sampled row)"
                : ""}
            </td>
          </tr>
        );
      })}
    </>
  );
}
