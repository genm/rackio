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
