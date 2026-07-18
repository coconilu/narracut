import type {
  MediaInputOption,
  MediaInputOptionGroup,
} from "./use-media-stage";

export interface MediaInputSelectorProps {
  readonly id: string;
  readonly group?: MediaInputOptionGroup;
  readonly value?: string;
  readonly disabled?: boolean;
  readonly onChange: (option: MediaInputOption | undefined) => void;
}

export function MediaInputSelector({
  id,
  group,
  value,
  disabled = false,
  onChange,
}: MediaInputSelectorProps) {
  const options = group?.options ?? [];
  const selectedValue = options.some((option) => option.artifactId === value)
    ? value
    : options.length === 1
      ? options[0].artifactId
      : "";
  const descriptionId = `${id}-description`;

  return (
    <fieldset className="media-input-selector" disabled={disabled}>
      <legend>
        {group ? `${group.requirement.stageId} 审核输入` : "缺少审核输入"}
      </legend>
      <p className="media-input-meta" id={descriptionId}>
        允许类型：{group?.requirement.artifactKinds.join(" / ") ?? "未声明"}
      </p>

      {options.length === 0 ? (
        <p className="media-input-error" role="alert">
          {group?.error ?? "尚未解析到可用的批准产物。"}
        </p>
      ) : (
        <label htmlFor={id}>
          <span>选择不可变产物</span>
          <select
            aria-describedby={descriptionId}
            id={id}
            onChange={(event) => {
              const option = options.find(
                (candidate) => candidate.artifactId === event.target.value,
              );
              onChange(option);
            }}
            required
            value={selectedValue}
          >
            {options.length > 1 ? (
              <option value="">请选择一个已审核产物</option>
            ) : null}
            {options.map((option) => (
              <option key={option.artifactId} value={option.artifactId}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
      )}

      {options.length > 0 && group?.error ? (
        <p className="media-input-meta" role="status">
          {group.error}
        </p>
      ) : null}
    </fieldset>
  );
}
