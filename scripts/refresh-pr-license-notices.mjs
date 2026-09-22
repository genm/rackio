import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export function validateTarget(pull, repository, defaultBranch, expectedHead) {
  if (
    pull.state !== "open" ||
    pull.head?.repo?.full_name !== repository ||
    pull.base?.repo?.full_name !== repository ||
    pull.base?.ref !== defaultBranch ||
    !/^[a-f0-9]{40}$/u.test(pull.head?.sha ?? "") ||
    (expectedHead !== undefined && pull.head.sha !== expectedHead)
  ) {
    throw new Error(
      "Expected an open same-repository PR against the default branch at the generated commit",
    );
  }
  return pull.head.sha;
}

export function noticeEntries(read) {
  // Artifact contents are data, never paths or executable publisher code.
  return ["THIRDPARTY.html", "THIRDPARTY-JAVASCRIPT.html"].map((path) => {
    const bytes = read(path);
    if (!bytes?.length) throw new Error(`Generated notice is missing or empty: ${path}`);
    const content = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return { path, mode: "100644", type: "blob", content };
  });
}

export function dependencyPaths(changed, tracked) {
  const allowed = new Set(
    tracked.filter(
      (path) =>
        /(^|\/)(Cargo\.toml|package\.json)$/u.test(path) ||
        ["Cargo.lock", "pnpm-lock.yaml"].includes(path),
    ),
  );
  const notices = new Set(["THIRDPARTY.html", "THIRDPARTY-JAVASCRIPT.html"]);
  if (changed.some((path) => !allowed.has(path) && !notices.has(path))) {
    throw new Error("Refresh requires a dependency-only PR using existing manifests");
  }
  return changed.filter((path) => allowed.has(path));
}

function main() {
  const {
    GITHUB_REPOSITORY: repository,
    DEFAULT_BRANCH: defaultBranch,
    PR_NUMBER: number,
  } = process.env;
  const mode = process.argv[2];
  if (
    !repository ||
    !defaultBranch ||
    !/^[1-9][0-9]*$/u.test(number ?? "") ||
    !["resolve", "prepare", "publish"].includes(mode)
  ) {
    throw new Error(
      "Require resolve/prepare/publish, GITHUB_REPOSITORY, DEFAULT_BRANCH, and a PR_NUMBER",
    );
  }
  const api = (endpoint, method = "GET", body) =>
    JSON.parse(
      execFileSync(
        "gh",
        [
          "api",
          `repos/${repository}/${endpoint}`,
          "--method",
          method,
          ...(body ? ["--input", "-"] : []),
        ],
        { encoding: "utf8", input: body ? JSON.stringify(body) : undefined },
      ),
    );
  const pull = api(`pulls/${number}`);
  const expectedHead = mode !== "resolve" ? process.env.EXPECTED_HEAD : undefined;
  if (mode !== "resolve" && !expectedHead) throw new Error("EXPECTED_HEAD is required");
  if (
    mode === "publish" &&
    (!expectedHead || !process.env.NOTICES_DIR || !process.env.GITHUB_STEP_SUMMARY)
  ) {
    throw new Error(
      "EXPECTED_HEAD, NOTICES_DIR, and GITHUB_STEP_SUMMARY are required before publishing",
    );
  }
  const head = validateTarget(pull, repository, defaultBranch, expectedHead);
  if (mode === "resolve") {
    appendFileSync(process.env.GITHUB_OUTPUT, `head=${head}\n`);
    return;
  }

  if (mode === "prepare") {
    const git = (...args) => execFileSync("git", args, { encoding: "utf8" });
    git("fetch", "--no-tags", "origin", head);
    // Only dependency data crosses into the trusted checkout. PR scripts,
    // toolchain files, hooks and package-manager configuration never execute.
    git("merge-base", "--is-ancestor", "HEAD", head);
    const paths = dependencyPaths(
      git("diff", "--name-only", "-z", "HEAD", head).split("\0").filter(Boolean),
      git("ls-files", "-z").split("\0").filter(Boolean),
    );
    const packageManager = JSON.parse(readFileSync("package.json", "utf8")).packageManager;
    for (const path of paths) {
      const content = git("show", `${head}:${path}`);
      if (path === "package.json" && JSON.parse(content).packageManager !== packageManager) {
        throw new Error("Update the trusted package manager before refreshing this PR");
      }
      writeFileSync(path, content);
    }
    return;
  }

  const tree = noticeEntries((path) => readFileSync(join(process.env.NOTICES_DIR, path)));
  const original = api(`git/commits/${head}`);
  const generated = api("git/trees", "POST", { base_tree: original.tree.sha, tree });
  if (generated.sha === original.tree.sha) {
    process.stdout.write(JSON.stringify({ status: "unchanged", head }) + "\n");
    return;
  }

  // GITHUB_TOKEN commits do not trigger PR workflows. Make the required human
  // ready-for-review action visible before changing the branch, never after.
  if (!pull.draft) {
    execFileSync("gh", ["pr", "ready", number, "--undo", "--repo", repository], {
      stdio: "inherit",
    });
  }
  const current = api(`pulls/${number}`);
  validateTarget(current, repository, defaultBranch, head);
  if (!current.draft) throw new Error("The PR must be draft before publishing a notice update");
  const commit = api("git/commits", "POST", {
    message: "chore(licenses): refresh bundled third-party notices",
    tree: generated.sha,
    parents: [head],
  });
  // A concurrent push produces a sibling commit: force=false rejects it rather
  // than replacing work that arrived while the notices were being generated.
  const branch = pull.head.ref.split("/").map(encodeURIComponent).join("/");
  const updated = api(`git/refs/heads/${branch}`, "PATCH", { sha: commit.sha, force: false });
  if (updated.object.sha !== commit.sha)
    throw new Error("Updated branch did not return the generated commit");
  const summary = `Updated PR #${number} at ${commit.sha}. Review the notice diff, then run \`gh pr ready ${number} --repo ${repository}\` to trigger the required CI. No merge was requested.\n`;
  appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
  process.stdout.write(JSON.stringify({ status: "updated", head: commit.sha, draft: true }) + "\n");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
