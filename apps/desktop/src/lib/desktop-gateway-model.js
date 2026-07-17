export function artifactReadMatchesRun(projectId, run, read) {
  const artifact = read.artifact;
  return (
    read.ownerProjectId === projectId &&
    artifact.projectId === run.projectId &&
    artifact.stageId === run.stageId &&
    artifact.runId === run.runId &&
    run.artifactIds.includes(artifact.artifactId)
  );
}

export function findJobByRunId(jobs, runId) {
  return jobs.find((job) => job.runId === runId);
}
