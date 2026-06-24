import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { emptySnapshot } from "../emptyState";
import { ObservabilityPanel } from "./ObservabilityPanel";

describe("ObservabilityPanel", () => {
  it("does not render placeholder audit rows as warning chips", () => {
    const markup = renderToStaticMarkup(
      <ObservabilityPanel
        zones={emptySnapshot.zones}
        audit={{
          ...emptySnapshot.audit,
          recent: [
            {
              action: "unknown action",
              actor: "unknown actor",
              interface: "unknown",
              method: "unknown",
              result: "unknown",
              resource: "observability.telemetry.query",
            },
          ],
        }}
        logpush={emptySnapshot.logpush}
        observability={emptySnapshot.observability}
        collector={emptySnapshot.collector}
      />,
    );

    expect(markup).not.toContain("Audit unknown");
    expect(markup).not.toContain("unknown action");
  });
});
