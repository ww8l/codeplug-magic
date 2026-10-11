import { openUrl } from "@tauri-apps/plugin-opener";

/**
 * RepeaterBook API helpers shared by the search dialog, the channel library and
 * Settings (#136). The attribution text and link are a promise made to
 * RepeaterBook in the API application (docs/repeaterbook-api-commitments.md):
 * they appear in the search dialog, the import summary, a record's detail view
 * and the About screen, word for word.
 */

/** `source` of a channel imported from the RepeaterBook API. */
export const RB_API_SOURCE = "repeaterbook_api";
/** `source` of a channel imported from a RepeaterBook CSV/JSON file. */
export const RB_FILE_SOURCE = "repeaterbook";

export const RB_HOME = "https://www.repeaterbook.com";

/** Either kind of RepeaterBook record: both carry the rb_* snapshot fields. */
export const isRbSource = (source: string | null | undefined) =>
  source === RB_API_SOURCE || source === RB_FILE_SOURCE;

/**
 * What the Source column shows. An API record reads "RepeaterBook", as
 * promised; a file-imported one is labelled apart so the two never look like
 * the same thing in a column or a filter.
 */
export function sourceLabel(source: string | null | undefined): string {
  if (source === RB_API_SOURCE) return "RepeaterBook";
  if (source === RB_FILE_SOURCE) return "RepeaterBook file";
  return source ?? "";
}

/** `last_rb_update` is SQLite's CURRENT_TIMESTAMP: UTC, "YYYY-MM-DD HH:MM:SS". */
function parseUtc(ts: string): Date {
  return new Date(ts.replace(" ", "T") + "Z");
}

/** An API record not refreshed within 12 months. Matches the backend's rule. */
export function isStale(lastRbUpdate: string | null): boolean {
  if (!lastRbUpdate) return true;
  const cutoff = new Date();
  cutoff.setFullYear(cutoff.getFullYear() - 1);
  return parseUtc(lastRbUpdate) < cutoff;
}

export function fmtImportDate(lastRbUpdate: string | null): string {
  if (!lastRbUpdate) return "unknown";
  return parseUtc(lastRbUpdate).toLocaleDateString();
}

/** Open a RepeaterBook page in the user's browser. */
export function openRb(url: string = RB_HOME) {
  void openUrl(url);
}

/** "Data courtesy of RepeaterBook.com", linked to RepeaterBook. */
export function RbAttribution({ className = "" }: { className?: string }) {
  return (
    <span className={className}>
      Data courtesy of{" "}
      <a
        href={RB_HOME}
        onClick={(e) => {
          e.preventDefault();
          openRb();
        }}
        className="font-medium text-sky-600 underline-offset-2 hover:underline dark:text-sky-400"
      >
        RepeaterBook.com
      </a>
    </span>
  );
}
