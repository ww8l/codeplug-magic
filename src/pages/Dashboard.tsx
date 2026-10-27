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

/// What a card's warning says and where each listed record opens.
interface CardWarning {
  title: string;
  detail: string;
  items: DashboardWarningItem[];
  linkLabel: string;
  page: string;
  itemPath: (id: number) => string;
}

function cardWarnings(w: DashboardWarnings): Partial<Record<StatKey, CardWarning>> {
  const out: Partial<Record<StatKey, CardWarning>> = {};
  const n = (items: DashboardWarningItem[], one: string, many: string) =>
    `${items.length} ${items.length === 1 ? one : many}`;
  if (w.dmr_channels_without_talkgroup.length > 0) {
    out.total_channels = {
      title: `${n(w.dmr_channels_without_talkgroup, "DMR channel has", "DMR channels have")} no talkgroup`,
      detail:
        "They will receive but cannot transmit a call. Open one to add its talkgroups under “Talkgroups on this repeater”.",
      items: w.dmr_channels_without_talkgroup,
      linkLabel: "Open Channels",
      page: "/channels",
      itemPath: (id) => `/channels?open=${id}`,
    };
  }
  if (w.unused_channel_lists.length > 0) {
    out.total_channel_lists = {
      title: `${n(w.unused_channel_lists, "zone/group is", "zones/groups are")} not in any codeplug`,
      detail: "They will not reach a radio until a codeplug includes them.",
      items: w.unused_channel_lists,
      linkLabel: "Open Zones/Groups",
      page: "/channel-lists",
      itemPath: () => "/channel-lists",
    };
  }
  if (w.unused_scan_lists.length > 0) {
    out.total_scan_lists = {
      title: `${n(w.unused_scan_lists, "scan list is", "scan lists are")} not in any codeplug`,
      detail: "They will not reach a radio until a codeplug includes them.",
      items: w.unused_scan_lists,
      linkLabel: "Open Scan Lists",
      page: "/scan-lists",
      itemPath: () => "/scan-lists",
    };
  }
  if (w.codeplugs_without_profile.length > 0) {
    out.total_codeplugs = {
      title: `${n(w.codeplugs_without_profile, "codeplug has", "codeplugs have")} no radio profile`,
      detail: "A codeplug needs a radio profile before it can be exported or programmed.",
      items: w.codeplugs_without_profile,
      linkLabel: "Open Codeplugs",
      page: "/codeplugs",
      itemPath: (id) => `/codeplugs/${id}`,
    };
  }
  return out;
}

/// The warning triangle on a stat card, and the popover it opens.
function CardWarningButton({ warning }: { warning: CardWarning }) {
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  // Open toward the roomier side: the triangle sits at a card's right edge, so
  // a popover anchored there on the leftmost card would run off the page.
  const [alignLeft, setAlignLeft] = useState(true);
  const ref = useRef<HTMLDivElement>(null);

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
        title={warning.title}
        aria-label={warning.title}
        aria-expanded={open}
        className="flex rounded p-0.5 text-orange-500 hover:bg-orange-500/10"
      >
        <TriangleAlert size={16} />
      </button>
      {open && (
        <div
          className={clsx(
            "absolute top-full z-20 mt-1 w-72 rounded-lg border border-slate-200 bg-white p-3 shadow-lg dark:border-slate-700 dark:bg-slate-800",
            alignLeft ? "left-0" : "right-0",
          )}
        >
          <div className="flex items-start gap-2">
            <TriangleAlert size={16} className="mt-0.5 shrink-0 text-orange-500" />
            <div>
              <div className="text-[13px] font-medium text-slate-800 dark:text-slate-100">
                {warning.title}
              </div>
              <div className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
                {warning.detail}
              </div>
            </div>
          </div>
          <ul className="mt-2 max-h-48 divide-y divide-slate-100 overflow-auto rounded border border-slate-100 dark:divide-slate-700 dark:border-slate-700">
            {warning.items.map((item) => (
              <li key={item.id}>
                <button
                  type="button"
                  onClick={() => navigate(warning.itemPath(item.id))}
                  className="block w-full truncate px-2 py-1 text-left text-xs text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-700/50"
                >
                  {item.label}
                </button>
              </li>
            ))}
          </ul>
          <div className="mt-2 flex justify-end">
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
