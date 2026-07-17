import type { ReviewDecision, ReviewRecord } from "@narracut/contracts";
import type { StageStudioController } from "../use-stage-studio";
import {
  StudioEmpty,
  StudioHeading,
  formatDate,
  reviewLabels,
  runStatusLabels,
} from "../stage-studio-primitives";

export function ReviewView({
  controller,
  disabled,
}: {
  readonly controller: StageStudioController;
  readonly disabled: boolean;
}) {
  const run = controller.selectedRun;
  if (!run) {
    return (
      <StudioEmpty
        title="尚无可审核运行"
        text="运行完成后才能写入审核记录。"
      />
    );
  }
  const reviews =
    controller.snapshot?.reviews.filter((review) => review.runId === run.runId) ?? [];
  return (
    <div className="studio-scroll review-view">
      <StudioHeading
        eyebrow={`${run.runId} · ${runStatusLabels[run.status]}`}
        title="审核候选运行"
        text="每次提交都会写入不可变 ReviewRecord；采用时必须明确勾选进入下游的产物。"
      />
      <div className="review-layout">
        <section className="review-form">
          <DecisionButtons controller={controller} disabled={disabled} />
          <fieldset className="artifact-checklist" disabled={disabled}>
            <legend>本次审核引用的产物</legend>
            {run.artifactIds.length ? (
              run.artifactIds.map((artifactId) => (
                <label key={artifactId}>
                  <input
                    checked={controller.selectedArtifactIds.includes(artifactId)}
                    onChange={() => controller.toggleArtifact(artifactId)}
                    type="checkbox"
                  />
                  <span>{artifactId}</span>
                </label>
              ))
            ) : (
              <p>该运行没有产物，不能被采用。</p>
            )}
          </fieldset>
          <label className="studio-field">
            <span>审核意见</span>
            <textarea
              aria-label="审核意见"
              disabled={disabled}
              onChange={(event) => controller.setReviewComments(event.target.value)}
              placeholder="说明采用依据、需要修改的内容或拒绝原因。"
              value={controller.reviewComments}
            />
          </label>
          <button
            className="button primary studio-primary-action"
            disabled={disabled}
            onClick={() => void controller.submitReview()}
            type="button"
          >
            提交审核记录
          </button>
        </section>
        <aside className="review-history">
          <strong>该运行的审核历史</strong>
          {reviews.length ? (
            [...reviews].reverse().map((review) => (
              <ReviewCard key={review.reviewId} review={review} />
            ))
          ) : (
            <p>还没有审核记录。</p>
          )}
        </aside>
      </div>
    </div>
  );
}

function DecisionButtons({
  controller,
  disabled,
}: {
  readonly controller: StageStudioController;
  readonly disabled: boolean;
}) {
  const run = controller.selectedRun;
  if (!run) return null;
  return (
    <div className="decision-buttons" role="group" aria-label="审核结论">
      {(Object.keys(reviewLabels) as ReviewDecision[]).map((decision) => (
        <button
          aria-pressed={controller.reviewDecision === decision}
          className={controller.reviewDecision === decision ? "selected" : ""}
          disabled={
            disabled || (decision === "approved" && run.status !== "succeeded")
          }
          key={decision}
          onClick={() => controller.setReviewDecision(decision)}
          type="button"
        >
          {reviewLabels[decision]}
        </button>
      ))}
    </div>
  );
}

function ReviewCard({ review }: { readonly review: ReviewRecord }) {
  return (
    <article>
      <div>
        <span>{reviewLabels[review.decision]}</span>
        <time>{formatDate(review.createdAt)}</time>
      </div>
      <code>{review.reviewId}</code>
      <p>{review.comments}</p>
      <small>{review.reviewer.displayName} · {review.artifactIds.length} 个产物</small>
    </article>
  );
}
