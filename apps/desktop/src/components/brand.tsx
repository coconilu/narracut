export function Brand({ compact = false }: { readonly compact?: boolean }) {
  return (
    <div className="brand" aria-label="NarraCut 叙剪">
      <span className="brand-mark" aria-hidden="true">N</span>
      {compact ? null : (
        <span className="brand-copy">
          <strong>NarraCut</strong>
          <span>叙剪</span>
        </span>
      )}
    </div>
  );
}
