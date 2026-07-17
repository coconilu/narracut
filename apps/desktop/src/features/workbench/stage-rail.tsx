import { Icon } from "../../components/icons";
import {
  stageNodeLabel,
  stageRunLabel,
  stageStatusLabel,
  stageStatusTone,
  type StageView,
} from "../../model/workbench";

interface StageRailProps {
  readonly stages: readonly StageView[];
  readonly selectedStageId: string;
  readonly onSelect: (stageId: string) => void;
}

export function StageRail({ stages, selectedStageId, onSelect }: StageRailProps) {
  const selectedIndex = Math.max(
    0,
    stages.findIndex((stage) => stage.definition.stageId === selectedStageId),
  );

  return (
    <nav className="stage-rail" aria-label="阶段导航">
      <div className="stage-rail-header">
        <strong>创作流程</strong>
        <span>{selectedIndex + 1} / {stages.length}</span>
      </div>
      <div className="stage-list">
        {stages.map(({ definition, state, index }) => {
          const selected = definition.stageId === selectedStageId;
          const node = stageNodeLabel(state, index);
          return (
            <button
              aria-current={selected ? "step" : undefined}
              className={`stage-item ${selected ? "selected" : ""} ${stageStatusTone(state.status)}`}
              data-testid={`stage-${definition.stageId}`}
              key={definition.stageId}
              onClick={() => onSelect(definition.stageId)}
              type="button"
            >
              <span className="stage-node" aria-hidden="true">
                {node === "check" ? <Icon name="check" size={14} /> : node === "alert" ? "!" : node}
              </span>
              <span className="stage-copy">
                <strong>{definition.title}</strong>
                <span>{stageRunLabel(state)}</span>
              </span>
              <span className="stage-status">{stageStatusLabel(state.status)}</span>
            </button>
          );
        })}
      </div>
    </nav>
  );
}
