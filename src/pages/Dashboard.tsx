import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  Radio,
  List,
  Package,
  ScanLine,
  Users,
  Upload,
  Plus,
  LayoutDashboard,
  TriangleAlert,
  Info,
} from "lucide-react";
import clsx from "clsx";
import { api } from "../lib/api";
import type {
  CodeplugSummary,
  DashboardStats,
  DashboardWarningItem,
  DashboardWarnings,
} from "../lib/types";
import { Button, Card, PageHeader, Spinner } from "../components/ui";
import { HandheldRadio } from "../components/icons/HandheldRadio";

const STAT_CARDS = [
  { key: "total_channels", label: "Channels", icon: Radio },
  { key: "total_channel_lists", label: "Zones/Groups", icon: List },
  { key: "total_scan_lists", label: "Scan Lists", icon: ScanLine },
  { key: "total_talkgroups", label: "Talkgroups", icon: Users },
  { key: "total_codeplugs", label: "Codeplugs", icon: Package },
  { key: "total_radio_profiles", label: "Radio Profiles", icon: HandheldRadio },
] as const;

type StatKey = (typeof STAT_CARDS)[number]["key"];

/// One problem on a card: what it is and the records it applies to.
interface WarningSection {
  /// "warn" will cost something at program time; "note" is worth knowing but
  /// harmless today (library records no codeplug uses yet).
  severity: "warn" | "note";
  title: string;
  detail: string;
  items: DashboardWarningItem[];
  itemPath: (id: number) => string;
}

/// Everything a card's triangle opens, and where its button goes.
interface CardWarning {
  sections: WarningSection[];
  linkLabel: string;
  page: string;
}

function cardWarnings(w: DashboardWarnings): Partial<Record<StatKey, CardWarning>> {
  const n = (items: DashboardWarningItem[], one: string, many: string) =>
    `${items.length} ${items.length === 1 ? one : many}`;
  const channel = (id: number) => `/channels?open=${id}`;
  const codeplug = (id: number) => `/codeplugs/${id}`;
  const lists = () => "/channel-lists";
  const scans = () => "/scan-lists";

  const cards: Partial<Record<StatKey, CardWarning>> = {
    total_channels: {
      linkLabel: "Open Channels",
      page: "/channels",
      sections: [
        {
          severity: "warn",
          title: `${n(w.dmr_without_talkgroup_in_codeplug, "DMR channel in a codeplug has", "DMR channels in a codeplug have")} no talkgroup`,
          detail:
            "They will receive but cannot transmit a call. Open one to add its talkgroups under “Talkgroups on this repeater”.",
          items: w.dmr_without_talkgroup_in_codeplug,
          itemPath: channel,
        },
        {
          severity: "warn",
          title: `${n(w.dmr_without_color_code, "DMR channel in a codeplug has", "DMR channels in a codeplug have")} no color code`,
          detail: "Without the repeater's color code the radio will not open squelch on it.",
          items: w.dmr_without_color_code,
          itemPath: channel,
        },
        {
          severity: "warn",
          title: `${n(w.off_air_in_codeplug, "channel in a codeplug is", "channels in a codeplug are")} marked Off Air`,
          detail: "These repeaters are listed as off the air, but a codeplug still programs them.",
          items: w.off_air_in_codeplug,
          itemPath: channel,
        },
        {
          severity: "note",
          title: `${n(w.stale_repeaterbook, "RepeaterBook record is", "RepeaterBook records are")} over a year old`,
          detail: "Refresh or delete them from the Channels page.",
          items: w.stale_repeaterbook,
          itemPath: channel,
        },
        {
          severity: "note",
          title: `${n(w.dmr_without_talkgroup_unused, "other DMR channel has", "other DMR channels have")} no talkgroup`,
          detail: "No codeplug includes these yet, so nothing is programmed wrong today.",
          items: w.dmr_without_talkgroup_unused,
          itemPath: channel,
        },
      ],
    },
    total_channel_lists: {
      linkLabel: "Open Zones/Groups",
      page: "/channel-lists",
      sections: [
        {
          severity: "warn",
          title: `${n(w.empty_channel_lists, "zone/group is", "zones/groups are")} empty`,
          detail: "A codeplug that includes one programs an empty zone.",
          items: w.empty_channel_lists,
          itemPath: lists,
        },
        {
          severity: "warn",
          title: `${n(w.unused_channel_lists, "zone/group is", "zones/groups are")} not in any codeplug`,
          detail: "They will not reach a radio until a codeplug includes them.",
          items: w.unused_channel_lists,
          itemPath: lists,
        },
      ],
    },
    total_scan_lists: {
      linkLabel: "Open Scan Lists",
      page: "/scan-lists",
      sections: [
        {
          severity: "warn",
          title: `${n(w.scan_list_channels_outside_zones, "scan list scans", "scan lists scan")} channels its codeplug leaves out`,
          detail: "A scan list can only scan channels that are in one of the codeplug's zones/groups.",
          items: w.scan_list_channels_outside_zones,
          itemPath: scans,
        },
        {
          severity: "warn",
          title: `${n(w.empty_scan_lists, "scan list is", "scan lists are")} empty`,
          detail: "A scan list with no channels has nothing to scan.",
          items: w.empty_scan_lists,
          itemPath: scans,
        },
        {
          severity: "warn",
          title: `${n(w.unused_scan_lists, "scan list is", "scan lists are")} not in any codeplug`,
          detail: "They will not reach a radio until a codeplug includes them.",
          items: w.unused_scan_lists,
          itemPath: scans,
        },
      ],
    },
    total_codeplugs: {
      linkLabel: "Open Codeplugs",
      page: "/codeplugs",
      sections: [
        {
          severity: "warn",
          title: `${n(w.codeplugs_without_profile, "codeplug has", "codeplugs have")} no radio profile`,
          detail: "A codeplug needs a radio profile before it can be exported or programmed.",
          items: w.codeplugs_without_profile,
          itemPath: codeplug,
        },
        {
          severity: "warn",
          title: `${n(w.codeplugs_without_zones, "codeplug has", "codeplugs have")} no zones/groups`,
          detail: "With no zones/groups there are no channels to program.",
          items: w.codeplugs_without_zones,
          itemPath: codeplug,
        },
        {
          severity: "warn",
          title: `Programming will drop or change ${n(w.codeplugs_program_problems, "thing", "things")}`,
          detail:
            "What the radio's own programmer reports for these codeplugs, before you connect it: anything it cannot hold, truncates, or substitutes.",
          items: w.codeplugs_program_problems,
          itemPath: codeplug,
        },
        {
          severity: "warn",
          title: `${n(w.codeplugs_changed_since_programmed, "codeplug has", "codeplugs have")} changed since last programmed`,
          detail: "The radio no longer matches. Program it again to bring it up to date.",
          items: w.codeplugs_changed_since_programmed,
          itemPath: codeplug,
        },
      ],
    },
  };

  const out: Partial<Record<StatKey, CardWarning>> = {};
  for (const [key, card] of Object.entries(cards) as [StatKey, CardWarning][]) {
    const sections = card.sections.filter((sec) => sec.items.length > 0);
    if (sections.length > 0) out[key] = { ...card, sections };
  }
  return out;
}

/// The warning triangle on a stat card, and the popover it opens. A card with
/// only notes gets a quieter info icon, so the triangle keeps meaning "this
/// will go wrong at program time".
function CardWarningButton({ warning }: { warning: CardWarning }) {
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  // Open toward the roomier side: the triangle sits at a card's right edge, so
  // a popover anchored there on the leftmost card would run off the page.
  const [alignLeft, setAlignLeft] = useState(true);
  const ref = useRef<HTMLDivElement>(null);

  const warns = warning.sections.some((sec) => sec.severity === "warn");
  const label = warning.sections.map((sec) => sec.title).join("; ");

  const toggle = () => {
    const r = ref.current?.getBoundingClientRect();
    if (r) setAlignLeft(r.left < window.innerWidth / 2);
    setOpen((o) => !o);
  };

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={toggle}
        title={label}
        aria-label={label}
        aria-expanded={open}
        className={clsx(
          "flex rounded p-0.5",
          warns
            ? "text-orange-500 hover:bg-orange-500/10"
            : "text-slate-400 hover:bg-slate-500/10",
        )}
      >
        {warns ? <TriangleAlert size={16} /> : <Info size={16} />}
      </button>
      {open && (
        <div
          className={clsx(
            "absolute top-full z-20 mt-1 max-h-[70vh] w-80 space-y-3 overflow-auto rounded-lg border border-slate-200 bg-white p-3 shadow-lg dark:border-slate-700 dark:bg-slate-800",
            alignLeft ? "left-0" : "right-0",
          )}
        >
          {warning.sections.map((sec) => (
            <div key={sec.title}>
              <div className="flex items-start gap-2">
                {sec.severity === "warn" ? (
                  <TriangleAlert size={16} className="mt-0.5 shrink-0 text-orange-500" />
                ) : (
                  <Info size={16} className="mt-0.5 shrink-0 text-slate-400" />
                )}
                <div>
                  <div className="text-[13px] font-medium text-slate-800 dark:text-slate-100">
                    {sec.title}
                  </div>
                  <div className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
                    {sec.detail}
                  </div>
                </div>
              </div>
              <ul className="mt-2 max-h-36 divide-y divide-slate-100 overflow-auto rounded border border-slate-100 dark:divide-slate-700 dark:border-slate-700">
                {sec.items.map((item, i) => (
                  <li key={i}>
                    <button
                      type="button"
                      onClick={() => navigate(sec.itemPath(item.id))}
                      className="block w-full px-2 py-1 text-left text-xs text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-700/50"
                    >
                      {item.label}
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          ))}
          <div className="flex justify-end">
            <Button onClick={() => navigate(warning.page)}>{warning.linkLabel}</Button>
          </div>
        </div>
      )}
    </div>
  );
}

export function Dashboard() {
  const navigate = useNavigate();
  const [stats, setStats] = useState<DashboardStats | null>(null);
  const [recent, setRecent] = useState<CodeplugSummary[]>([]);
  const [warnings, setWarnings] = useState<Partial<Record<StatKey, CardWarning>>>({});
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    Promise.all([api.getDashboardStats(), api.getRecentCodeplugs()])
      .then(([s, r]) => {
        setStats(s);
        setRecent(r);
      })
      .finally(() => setLoading(false));
    // Warnings are advisory: a failure here must not blank the dashboard.
    api
      .getDashboardWarnings()
      .then((w) => setWarnings(cardWarnings(w)))
      .catch(() => setWarnings({}));
  }, []);

  return (
    <>
      <PageHeader icon={<LayoutDashboard size={18} />} title="Dashboard" subtitle="Overview of your master database" />
      <div className="flex-1 overflow-auto p-5">
        {loading ? (
          <div className="flex justify-center py-20">
            <Spinner className="h-6 w-6" />
          </div>
        ) : (
          <div className="space-y-6">
            {/* Stat cards */}
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
              {STAT_CARDS.map(({ key, label, icon: Icon }) => (
                <Card key={key} className="p-4">
                  <div className="flex items-center justify-between">
                    <span className="text-xs font-medium text-slate-500 dark:text-slate-400">
                      {label}
                    </span>
                    <Icon size={16} className="text-slate-400" />
                  </div>
                  <div className="mt-2 flex items-center justify-between">
                    <div className="text-2xl font-semibold tabular-nums text-slate-900 dark:text-slate-100">
                      {stats ? stats[key] : 0}
                    </div>
                    {warnings[key] && <CardWarningButton warning={warnings[key]} />}
                  </div>
                </Card>
              ))}
            </div>

            {/* Quick actions */}
            <div>
              <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-slate-400">
                Quick Actions
              </h2>
              <div className="flex gap-2">
                <Button variant="primary" onClick={() => navigate("/channels?import=1")}>
                  <Upload size={14} /> Import CSV
                </Button>
                <Button onClick={() => navigate("/codeplugs?new=1")}>
                  <Plus size={14} /> New Codeplug
                </Button>
              </div>
            </div>

            {/* Recent codeplugs */}
            <div>
              <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-slate-400">
                Recent Codeplugs
              </h2>
              <Card>
                {recent.length === 0 ? (
                  <div className="px-4 py-8 text-center text-xs text-slate-400">
                    No codeplugs yet. Create one to get started, e.g.
                    "W0CPH-AT878-Truck".
                  </div>
                ) : (
                  <ul className="divide-y divide-slate-100 dark:divide-slate-700">
                    {recent.map((cp) => (
                      <li
                        key={cp.id}
                        onClick={() => navigate(`/codeplugs/${cp.id}`)}
                        className="flex cursor-pointer items-center justify-between px-4 py-2.5 hover:bg-slate-50 dark:hover:bg-slate-700/50"
                      >
                        <div>
                          <div className="text-[13px] font-medium text-slate-800 dark:text-slate-100">
                            {cp.name}
                          </div>
                          <div className="text-xs text-slate-500 dark:text-slate-400">
                            {cp.model_display_name ?? "No radio profile"} ·{" "}
                            {cp.channel_count} channels
                          </div>
                        </div>
                        <div className="text-xs text-slate-400">
                          {cp.last_exported
                            ? `${
                                cp.last_export_kind === "radio"
                                  ? "Programmed"
                                  : "Exported"
                              } ${cp.last_exported.slice(0, 10)}`
                            : "Never exported"}
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </Card>
            </div>
          </div>
        )}
      </div>
    </>
  );
}
