export function evaluateEnvironment(checks) {
  const failures = checks.filter((check) => check.required && !check.ok).map((check) => check.name);
  const degraded = checks
    .filter((check) => !check.required && !check.ok)
    .map((check) => check.name);

  return {
    schemaVersion: 1,
    status: failures.length > 0 ? "failed" : degraded.length > 0 ? "degraded" : "ready",
    checks,
    failures,
    degraded,
    exitCode: failures.length > 0 ? 1 : 0,
  };
}

// pnpm 12 prepends a package-manager manifest document to pnpm-lock.yaml, so
// the resolved lockfile copied to node_modules/.pnpm/lock.yaml is the last
// YAML document in the file. Single-document lockfiles (pnpm <= 11) pass
// through unchanged.
export function resolvedLockfileDocument(lockfileContent) {
  return lockfileContent.split(/^---\r?\n/m).at(-1);
}
