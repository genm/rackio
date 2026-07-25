import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const output = resolve(process.argv[2] ?? `${repoRoot}/THIRDPARTY-JAVASCRIPT.html`);
const inventory = JSON.parse(
  execFileSync("pnpm", ["licenses", "list", "--prod", "--json"], {
    cwd: repoRoot,
    encoding: "utf8",
  }),
);

const escapeHtml = (value) =>
  value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");

const packages = Object.values(inventory)
  .flat()
  .flatMap((entry) =>
    entry.paths.map((packagePath) => {
      const manifest = JSON.parse(readFileSync(resolve(packagePath, "package.json"), "utf8"));
      const licenseFiles = readdirSync(packagePath)
        .filter((name) => /^licen[cs]e(?:[._-].*)?$/iu.test(name))
        .sort();
      if (licenseFiles.length === 0) {
        throw new Error(`${manifest.name}@${manifest.version} has no packaged license notice`);
      }
      return {
        name: manifest.name,
        version: manifest.version,
        declaredLicense: entry.license,
        homepage: entry.homepage,
        notices: licenseFiles.map((name) => ({
          name,
          text: readFileSync(resolve(packagePath, name), "utf8"),
        })),
      };
    }),
  )
  .sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`),
  );

const sections = packages
  .map(
    ({ name, version, declaredLicense, homepage, notices }) => `
      <section>
        <h2>${escapeHtml(name)} ${escapeHtml(version)}</h2>
        <p>Declared license: ${escapeHtml(declaredLicense)}</p>
        ${homepage ? `<p><a href="${escapeHtml(homepage)}">${escapeHtml(homepage)}</a></p>` : ""}
        ${notices
          .map(
            (notice) => `<h3>${escapeHtml(notice.name)}</h3><pre>${escapeHtml(notice.text)}</pre>`,
          )
          .join("\n")}
      </section>`,
  )
  .join("\n");

const html = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Rackio JavaScript third-party licenses</title>
    <style>
      body { font: 16px/1.5 system-ui, sans-serif; margin: 2rem auto; max-width: 72rem; padding: 0 1rem; }
      section { border-top: 1px solid #aaa; margin-top: 2rem; padding-top: 1rem; }
      pre { overflow: auto; white-space: pre-wrap; }
    </style>
  </head>
  <body>
    <h1>Rackio JavaScript third-party licenses</h1>
    <p>Generated from the production dependency graph in pnpm-lock.yaml.</p>
${sections}
  </body>
</html>
`;

writeFileSync(output, html);
