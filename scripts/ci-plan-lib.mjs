const GLOBAL_PATHS = [
  /^\.github\/workflows\//,
  /^mise\.toml$/,
  /^scripts\/ci-plan(?:-lib)?\.mjs$/,
  /^scripts\/ci-plan\.test\.mjs$/,
];

const CROSS_PLATFORM_RUST_PATHS = [
  /^Cargo\.(?:toml|lock)$/,
  /^rust-toolchain\.toml$/,
  /^(?:apps\/agent|crates|proto)\//,
  /^apps\/desktop\/src-tauri\//,
  /^packaging\/(?!linux\/|macos\/|windows\/|.*\.md$)/,
];

const LINUX_RUST_PATHS = [
  /^install\.sh$/,
  /^packaging\/linux\/(?!.*\.md$)/,
  /^scripts\/(?:benchmark-agent-resources|test-two-daemon-pairing)\.sh$/,
];

const MACOS_RUST_PATHS = [/^packaging\/macos\/(?!.*\.md$)/];

const WINDOWS_RUST_PATHS = [
  /^packaging\/windows\/(?!.*\.md$)/,
  /^scripts\/test-windows-named-pipe\.ps1$/,
];

const FRONTEND_PATHS = [
  /^(?:package\.json|pnpm-lock\.yaml|pnpm-workspace\.yaml|commitlint\.config\.mjs)$/,
  /^apps\/desktop\/(?!src-tauri\/)/,
  /^apps\/desktop\/src-tauri\/(?:capabilities\/.*|tauri\.conf)\.json$/,
  /^scripts\/.*\.mjs$/,
];

const SECURITY_POLICY_PATHS = [
  /^\.github\/dependabot\.yml$/,
  /^(?:Cargo\.(?:toml|lock)|deny\.toml|rust-toolchain\.toml)$/,
  /^(?:package\.json|pnpm-lock\.yaml|pnpm-workspace\.yaml)$/,
  /^THIRDPARTY(?:-JAVASCRIPT)?\.html$/,
  /^(?:apps\/agent|crates|proto)\//,
  /^(?:packaging|relay-package)\/(?!.*\.md$)/,
  /^install\.sh$/,
  /^apps\/desktop\/(?:package\.json|src-tauri\/)/,
  /^scripts\/generate-(?:javascript-licenses|third-party-licenses)\.(?:mjs|sh)$/,
];

const SECURITY_SOURCE_PATHS = [/^(?:apps|crates)\//];

function matchesAny(path, patterns) {
  return patterns.some((pattern) => pattern.test(path));
}

export function fullPlan(reason) {
  return {
    full_run: true,
    rust: true,
    rust_linux: true,
    rust_macos: true,
    rust_windows: true,
    frontend: true,
    security_policy: true,
    security_source: true,
    reason,
  };
}

export function classifyChangedFiles(files) {
  // CI routing changes own the selector, so they deliberately exercise every gate.
  if (files.some((path) => matchesAny(path, GLOBAL_PATHS))) {
    return fullPlan("global_ci_change");
  }

  const crossPlatformRust = files.some((path) => matchesAny(path, CROSS_PLATFORM_RUST_PATHS));
  const rustLinux = crossPlatformRust || files.some((path) => matchesAny(path, LINUX_RUST_PATHS));
  const rustMacos = crossPlatformRust || files.some((path) => matchesAny(path, MACOS_RUST_PATHS));
  const rustWindows =
    crossPlatformRust || files.some((path) => matchesAny(path, WINDOWS_RUST_PATHS));

  return {
    full_run: false,
    rust: rustLinux || rustMacos || rustWindows,
    rust_linux: rustLinux,
    rust_macos: rustMacos,
    rust_windows: rustWindows,
    frontend: files.some((path) => matchesAny(path, FRONTEND_PATHS)),
    security_policy: files.some((path) => matchesAny(path, SECURITY_POLICY_PATHS)),
    security_source: files.some((path) => matchesAny(path, SECURITY_SOURCE_PATHS)),
    reason: files.length === 0 ? "no_changes" : "affected",
  };
}

export function planForEvent({ eventName, eventAction, files = [] }) {
  if (eventName === "schedule") {
    return fullPlan("scheduled_full_run");
  }

  if (eventName === "pull_request") {
    // GitHub supplies comparable base/head SHAs for every supported PR
    // transition, so lifecycle events can use the same affected plan as pushes.
    if (!["opened", "ready_for_review", "reopened", "synchronize"].includes(eventAction)) {
      return fullPlan("unknown_pull_request_action");
    }
  } else if (eventName !== "push") {
    return fullPlan("unknown_event");
  }

  return classifyChangedFiles(files);
}
