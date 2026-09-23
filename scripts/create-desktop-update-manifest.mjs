import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PLATFORMS = ["darwin-aarch64", "darwin-x86_64"];

export function createDesktopUpdateManifest({
  version,
  releaseBaseUrl,
  artifactsDirectory,
  outputDirectory,
}) {
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) {
    throw new Error("desktop updater manifests require a stable semantic version");
  }

  const releaseBase = new URL(releaseBaseUrl);
  if (
    releaseBase.protocol !== "https:" ||
    releaseBase.username ||
    releaseBase.password ||
    releaseBase.search ||
    releaseBase.hash ||
    !releaseBase.pathname.endsWith(`/v${version}`)
  ) {
    throw new Error("release URL must be an HTTPS versioned release asset path");
  }

  const preparedAssets = PLATFORMS.map((platform) => {
    const sourceDirectory = join(artifactsDirectory, platform);
    const sourceArchive = join(sourceDirectory, "Rackio.app.tar.gz");
    const sourceSignature = `${sourceArchive}.sig`;
    const archiveInfo = statSync(sourceArchive, { throwIfNoEntry: false });
    if (!archiveInfo?.isFile() || archiveInfo.size === 0) {
      throw new Error(`${platform} updater archive is missing or empty`);
    }

    const signatureInfo = statSync(sourceSignature, { throwIfNoEntry: false });
    if (!signatureInfo?.isFile() || signatureInfo.size === 0) {
      throw new Error(`${platform} updater signature is missing or empty`);
    }
    const signature = readFileSync(sourceSignature, "utf8").trimEnd();
    if (!signature.trim()) {
      throw new Error(`${platform} updater signature is missing or empty`);
    }

    const archiveName = `Rackio-${platform}.app.tar.gz`;
    return {
      platform,
      sourceArchive,
      sourceSignature,
      archiveName,
      signature,
      url: new URL(archiveName, `${releaseBase.href}/`).href,
    };
  });

  mkdirSync(outputDirectory, { recursive: true });
  const platforms = {};
  for (const asset of preparedAssets) {
    copyFileSync(asset.sourceArchive, join(outputDirectory, asset.archiveName));
    copyFileSync(asset.sourceSignature, join(outputDirectory, `${asset.archiveName}.sig`));
    platforms[asset.platform] = { url: asset.url, signature: asset.signature };
  }

  const manifest = { version, platforms };
  const manifestPath = join(outputDirectory, "latest.json");
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  const checksumNames = [
    ...preparedAssets.flatMap((asset) => [asset.archiveName, `${asset.archiveName}.sig`]),
    "latest.json",
  ].sort();
  const checksums = checksumNames
    .map((name) => {
      const digest = createHash("sha256")
        .update(readFileSync(join(outputDirectory, name)))
        .digest("hex");
      return `${digest}  ${name}`;
    })
    .join("\n");
  writeFileSync(join(outputDirectory, "SHA256SUMS"), `${checksums}\n`);
  return manifest;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [version, releaseBaseUrl, artifactsDirectory, outputDirectory, ...extra] =
    process.argv.slice(2);
  if (!version || !releaseBaseUrl || !artifactsDirectory || !outputDirectory || extra.length > 0) {
    console.error(
      "usage: node create-desktop-update-manifest.mjs VERSION RELEASE_BASE_URL ARTIFACTS_DIR OUTPUT_DIR",
    );
    process.exitCode = 2;
  } else {
    try {
      createDesktopUpdateManifest({
        version,
        releaseBaseUrl,
        artifactsDirectory,
        outputDirectory,
      });
      console.log(`assembled Rackio ${version} updater assets in ${basename(outputDirectory)}`);
    } catch (error) {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    }
  }
}
