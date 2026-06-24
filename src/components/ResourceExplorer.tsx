import { InventoryTable } from "./InventoryTable";
import type { ResourceRow } from "../types";

type ResourceExplorerProps = {
  rows: ResourceRow[];
  selectedResourceKey?: string;
  title: string;
  description: string;
  onSelectResource: (resource: ResourceRow) => void;
};

export function ResourceExplorer({
  rows,
  selectedResourceKey,
  title,
  description,
  onSelectResource,
}: ResourceExplorerProps) {
  return (
    <section className="resource-explorer" aria-label="Resource explorer">
      <InventoryTable
        rows={rows}
        selectedResourceKey={selectedResourceKey}
        onSelectResource={onSelectResource}
        title={title}
        description={description}
      />
    </section>
  );
}
