import type { MouseEvent } from "react";
import { Minus, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isDesktopRuntime } from "../api";

type ResizeDirection = "East" | "North" | "NorthEast" | "NorthWest" | "South" | "SouthEast" | "SouthWest" | "West";

type WindowChromeProps = {
  accountName?: string;
  connected: boolean;
  syncing: boolean;
  live: boolean;
};

const resizeHandles: Array<{ direction: ResizeDirection; className: string }> = [
  { direction: "North", className: "north" },
  { direction: "South", className: "south" },
  { direction: "West", className: "west" },
  { direction: "East", className: "east" },
  { direction: "NorthWest", className: "north-west" },
  { direction: "NorthEast", className: "north-east" },
  { direction: "SouthWest", className: "south-west" },
  { direction: "SouthEast", className: "south-east" },
];

async function runWindowCommand(command: "minimize" | "toggleMaximize" | "close") {
  if (!isDesktopRuntime()) return;

  try {
    const appWindow = getCurrentWindow();
    if (command === "minimize") await appWindow.minimize();
    if (command === "toggleMaximize") await appWindow.toggleMaximize();
    if (command === "close") await appWindow.close();
  } catch {
    // Window APIs are unavailable in plain browser previews.
  }
}

async function startWindowDrag() {
  if (!isDesktopRuntime()) return;

  try {
    await getCurrentWindow().startDragging();
  } catch {
    // Window APIs are unavailable in plain browser previews.
  }
}

async function startResize(direction: ResizeDirection) {
  if (!isDesktopRuntime()) return;

  try {
    await getCurrentWindow().startResizeDragging(direction);
  } catch {
    // Window APIs are unavailable in plain browser previews.
  }
}

export function WindowChrome({ accountName, connected, syncing, live }: WindowChromeProps) {
  const statusLabel = syncing ? "Syncing" : connected ? (live ? "Live" : "Connected") : "Local preview";

  function handleChromeMouseDown(event: MouseEvent<HTMLElement>) {
    if (event.button !== 0) return;
    if (event.detail > 1) return;
    if ((event.target as HTMLElement).closest("button")) return;
    void startWindowDrag();
  }

  return (
    <>
      <header
        className="window-chrome"
        data-tauri-drag-region
        onMouseDown={handleChromeMouseDown}
        onDoubleClick={(event) => {
          if ((event.target as HTMLElement).closest(".window-control")) return;
          void runWindowCommand("toggleMaximize");
        }}
      >
        <div className="chrome-identity" data-tauri-drag-region>
          <strong data-tauri-drag-region>Cedar</strong>
          <span className="chrome-account" data-tauri-drag-region>
            {accountName ?? "Cloudflare ops"}
          </span>
        </div>

        <div className={`chrome-status ${live ? "live" : connected ? "ready" : "idle"}`} data-tauri-drag-region>
          <i />
          <span>{statusLabel}</span>
        </div>

        <div className="window-controls" aria-label="Window controls">
          <button className="window-control" type="button" aria-label="Minimize window" onClick={() => void runWindowCommand("minimize")}>
            <Minus size={15} />
          </button>
          <button
            className="window-control"
            type="button"
            aria-label="Toggle maximize window"
            onClick={() => void runWindowCommand("toggleMaximize")}
          >
            <Square size={13} />
          </button>
          <button className="window-control close" type="button" aria-label="Close window" onClick={() => void runWindowCommand("close")}>
            <X size={15} />
          </button>
        </div>
      </header>

      {resizeHandles.map((handle) => (
        <button
          aria-hidden="true"
          className={`resize-handle ${handle.className}`}
          key={handle.direction}
          onMouseDown={(event) => {
            event.preventDefault();
            void startResize(handle.direction);
          }}
          tabIndex={-1}
          type="button"
        />
      ))}
    </>
  );
}
