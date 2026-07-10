// 组装 npm 发布产物: 从 GitHub Release 的压缩包中解出各平台二进制,
// 生成 6 个平台子包和 1 个主包到 npm/dist/, 供 CI 依次 npm publish。
//
// 用法: node npm/build.mjs <tag> <assets-dir>
// 例如: node npm/build.mjs v0.1.2 /tmp/assets

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SCOPE = "@zero9501";
const BIN = "git-pincer";

// Rust 构建目标 → npm 子包的平台映射(与 release.yml 的 build matrix 一一对应)
const TARGETS = [
  { target: "aarch64-apple-darwin", suffix: "darwin-arm64", os: "darwin", cpu: "arm64" },
  { target: "x86_64-apple-darwin", suffix: "darwin-x64", os: "darwin", cpu: "x64" },
  { target: "aarch64-unknown-linux-gnu", suffix: "linux-arm64-gnu", os: "linux", cpu: "arm64", libc: "glibc" },
  { target: "x86_64-unknown-linux-gnu", suffix: "linux-x64-gnu", os: "linux", cpu: "x64", libc: "glibc" },
  { target: "x86_64-unknown-linux-musl", suffix: "linux-x64-musl", os: "linux", cpu: "x64", libc: "musl" },
  { target: "x86_64-pc-windows-msvc", suffix: "win32-x64", os: "win32", cpu: "x64" },
];

const [tag, assetsDir] = process.argv.slice(2);
if (!tag || !assetsDir) {
  console.error("usage: node npm/build.mjs <tag> <assets-dir>");
  process.exit(1);
}
const version = tag.replace(/^v/, "");

const npmDir = path.dirname(fileURLToPath(import.meta.url));
const repoDir = path.dirname(npmDir);
const distDir = path.join(npmDir, "dist");
fs.rmSync(distDir, { recursive: true, force: true });
fs.mkdirSync(distDir, { recursive: true });

// 解压 release 压缩包(带前导目录), 返回其中二进制文件的路径
function extractBinary(t, workDir) {
  const stem = `${BIN}-${tag}-${t.target}`;
  const isWin = t.os === "win32";
  const archive = path.join(assetsDir, `${stem}.${isWin ? "zip" : "tar.gz"}`);
  if (!fs.existsSync(archive)) {
    console.error(`missing release asset: ${archive}`);
    process.exit(1);
  }
  if (isWin) {
    execFileSync("unzip", ["-q", archive, "-d", workDir]);
  } else {
    execFileSync("tar", ["-xzf", archive, "-C", workDir]);
  }
  return path.join(workDir, stem, isWin ? `${BIN}.exe` : BIN);
}

// 生成一个平台子包: package.json + bin/ 下的二进制
function buildPlatformPackage(t, index) {
  const name = `${SCOPE}/${BIN}-${t.suffix}`;
  console.log(`[${index + 1}/${TARGETS.length}] assembling ${name}@${version}`);

  const workDir = fs.mkdtempSync(path.join(distDir, "extract-"));
  const binary = extractBinary(t, workDir);

  const pkgDir = path.join(distDir, SCOPE, `${BIN}-${t.suffix}`);
  const binDir = path.join(pkgDir, "bin");
  fs.mkdirSync(binDir, { recursive: true });
  fs.copyFileSync(binary, path.join(binDir, path.basename(binary)));
  fs.chmodSync(path.join(binDir, path.basename(binary)), 0o755);
  fs.rmSync(workDir, { recursive: true, force: true });

  const manifest = {
    name,
    version,
    description: `${BIN} binary for ${t.suffix}`,
    homepage: "https://github.com/zlx2019/git-pincer",
    repository: { type: "git", url: "git+https://github.com/zlx2019/git-pincer.git" },
    license: "MIT",
    os: [t.os],
    cpu: [t.cpu],
    ...(t.libc ? { libc: [t.libc] } : {}),
    files: ["bin"],
  };
  fs.writeFileSync(path.join(pkgDir, "package.json"), JSON.stringify(manifest, null, 2) + "\n");
  return name;
}

// 生成主包: 拷贝垫片, 重写版本号并把 optionalDependencies 固定到同版本
function buildMainPackage(platformNames) {
  console.log(`assembling ${BIN}@${version}`);
  const srcDir = path.join(npmDir, BIN);
  const pkgDir = path.join(distDir, BIN);
  fs.cpSync(path.join(srcDir, "bin"), path.join(pkgDir, "bin"), { recursive: true });
  fs.copyFileSync(path.join(repoDir, "README.md"), path.join(pkgDir, "README.md"));
  fs.copyFileSync(path.join(repoDir, "LICENSE"), path.join(pkgDir, "LICENSE"));

  const manifest = JSON.parse(fs.readFileSync(path.join(srcDir, "package.json"), "utf8"));
  manifest.version = version;
  manifest.optionalDependencies = Object.fromEntries(platformNames.map((name) => [name, version]));
  fs.writeFileSync(path.join(pkgDir, "package.json"), JSON.stringify(manifest, null, 2) + "\n");
}

const names = TARGETS.map(buildPlatformPackage);
buildMainPackage(names);
console.log(`done: ${TARGETS.length + 1} packages in ${distDir}`);
