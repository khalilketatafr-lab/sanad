import { useEffect, useRef } from "react";
import { Button } from "react-aria-components";
import { parseRoute } from "./route.ts";

/**
 * Phase 0 reader shell. The page surface is a <canvas> owned by Lumen; this
 * component never renders book content into the DOM (P1). Chrome strings are
 * UI copy and catalog metadata only.
 */
export function App() {
  const route = parseRoute(window.location.pathname);
  const surface = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = surface.current;
    if (canvas === null) return;
    canvas.addEventListener("contextmenu", preventDefault);
    canvas.addEventListener("dragstart", preventDefault);
    // Readiness signal consumed by the P1 harness and RUM. Lumen will move this
    // mark to its first presented frame once the render worker is wired in.
    performance.mark("lumen:ready", { detail: { route: route.kind } });
    return () => {
      canvas.removeEventListener("contextmenu", preventDefault);
      canvas.removeEventListener("dragstart", preventDefault);
    };
  }, [route.kind]);

  return (
    <div className="reader" data-route={route.kind}>
      <Button className="reader__button sr-only">Turn on screen-reader mode</Button>
      <header className="reader__bar">
        <Button className="reader__button" aria-label="Back to library">
          ‹
        </Button>
        <h1 className="reader__title">{route.kind === "read" ? "Reader" : "Sanad"}</h1>
        <Button className="reader__button" aria-label="Reading settings">
          Aa
        </Button>
      </header>
      <main>
        <canvas
          ref={surface}
          className="reader__surface"
          role="img"
          aria-label="Book page. Turn on screen-reader mode to read with assistive technology."
          draggable={false}
        />
      </main>
    </div>
  );
}

function preventDefault(event: Event): void {
  event.preventDefault();
}
