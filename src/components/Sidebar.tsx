import { NavLink } from "react-router-dom";
import clsx from "clsx";
import {
  LayoutDashboard,
  Radio,
  List,
  ScanLine,
  Users,
  IdCard,
  Package,
  Settings,
} from "lucide-react";
import { HandheldRadio } from "./icons/HandheldRadio";
import { APP_VERSION } from "./ui";

const NAV = [
  { to: "/", label: "Dashboard", icon: LayoutDashboard, end: true },
  { to: "/channels", label: "Channels", icon: Radio },
  { to: "/channel-lists", label: "Zones/Groups", icon: List },
  { to: "/scan-lists", label: "Scan Lists", icon: ScanLine },
  { to: "/talkgroups", label: "Talkgroups", icon: Users },
  { to: "/dmr-contacts", label: "DMR Contacts", icon: IdCard },
  { to: "/codeplugs", label: "Codeplugs", icon: Package },
  { to: "/radio-profiles", label: "Radios", icon: HandheldRadio },
  { to: "/settings", label: "Settings", icon: Settings },
];

export function Sidebar() {
  return (
    <aside className="flex w-56 shrink-0 flex-col border-r border-slate-200 bg-slate-50 dark:border-slate-700 dark:bg-slate-900">
      <div className="flex items-center gap-2 px-4 py-3.5">
        <div className="flex h-7 w-7 items-center justify-center rounded-md bg-sky-600 text-white">
          <Radio size={16} />
        </div>
        <div className="text-sm font-semibold text-slate-800 dark:text-slate-100">
          WW8L Codeplug Magic
        </div>
      </div>

      <nav className="flex-1 space-y-0.5 px-2 py-1">
        {NAV.map(({ to, label, icon: Icon, end }) => (
          <NavLink
            key={to}
            to={to}
            end={end}
            className={({ isActive }) =>
              clsx(
                "flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-[13px] font-medium transition-colors",
                isActive
                  ? "bg-sky-600 text-white"
                  : "text-slate-600 hover:bg-slate-200/70 dark:text-slate-300 dark:hover:bg-slate-800",
              )
            }
          >
            <Icon size={16} />
            {label}
          </NavLink>
        ))}
      </nav>

      <div className="px-4 py-2 text-[11px] text-slate-400 dark:text-slate-500">
        v{APP_VERSION}
      </div>
    </aside>
  );
}
