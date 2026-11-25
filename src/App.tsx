import { useEffect, useState } from "react";
import Dashboard from "./pages/Dashboard";
import AddProject from "./pages/AddProject";
import Wizard from "./pages/Wizard";
import History from "./pages/History";
import Settings from "./pages/Settings";
import Runners from "./pages/Runners";

export type Route =
  | { page: "dashboard" }
  | { page: "add" }
  | { page: "edit"; id: number }
  | { page: "wizard"; id: number; runId?: number }
  | { page: "history"; id?: number }
  | { page: "runners" }
  | { page: "settings" };

const NAV: { key: Route["page"]; label: string; hotkey: string }[] = [
  { key: "dashboard", label: "Projects", hotkey: "1" },
  { key: "history", label: "History", hotkey: "2" },
  { key: "runners", label: "Runners", hotkey: "3" },
  { key: "settings", label: "Settings", hotkey: "4" },
];

export default function App() {
  const [route, setRoute] = useState<Route>({ page: "dashboard" });

  useEffect(() => {
    // Ctrl+1/2/3 switch sections; Ctrl+N adds a project.
    const k = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.altKey) return;
      const n = NAV.find((x) => x.hotkey === e.key);
      if (n) {
        e.preventDefault();
        setRoute({ page: n.key } as Route);
      } else if (e.key.toLowerCase() === "n") {
        e.preventDefault();
        setRoute({ page: "add" });
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, []);

  return (
    <div className="flex h-full">
      <nav className="flex w-48 shrink-0 flex-col border-r border-line bg-panel p-3">
        <div className="mb-5 px-2 pt-1">
          <div className="text-[15px] font-semibold">Release Desk</div>
          <div className="text-[11px] text-mute">commit to release</div>
        </div>
        {NAV.map((n) => (
          <button
            key={n.key}
            onClick={() => setRoute({ page: n.key } as Route)}
            className={`mb-0.5 flex items-center justify-between rounded-md px-2 py-1.5 text-left text-[13px] ${
              route.page === n.key || (n.key === "dashboard" && ["add", "edit", "wizard"].includes(route.page))
                ? "bg-panel2 text-ink"
                : "text-mute hover:text-ink"
            }`}
          >
            {n.label}
            <kbd className="text-[10px] text-mute">Ctrl {n.hotkey}</kbd>
          </button>
        ))}
      </nav>
      <main className="min-w-0 flex-1 overflow-auto">
        {route.page === "dashboard" && <Dashboard go={setRoute} />}
        {route.page === "add" && <AddProject go={setRoute} />}
        {route.page === "edit" && <AddProject go={setRoute} editId={route.id} />}
        {route.page === "wizard" && <Wizard key={`${route.id}-${route.runId ?? ""}`} id={route.id} resumeRunId={route.runId} go={setRoute} />}
        {route.page === "history" && <History projectId={route.id} go={setRoute} />}
        {route.page === "runners" && <Runners />}
        {route.page === "settings" && <Settings />}
      </main>
    </div>
  );
}
