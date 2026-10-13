#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");
const path = require("node:path");

function isMusl() {
  try {
    return !process.report.getReport().header.glibcVersionRuntime;
  } catch {
    return false;
  }
}

function platformPackage() {
  const { platform, arch } = process;
  if (platform === "darwin" && arch === "arm64") return "@zero9501/git-pincer-darwin-arm64";
  if (platform === "darwin" && arch === "x64") return "@zero9501/git-pincer-darwin-x64";
  if (platform === "linux" && arch === "arm64") return "@zero9501/git-pincer-linux-arm64-gnu";
  if (platform === "linux" && arch === "x64") {
    return isMusl() ? "@zero9501/git-pincer-linux-x64-musl" : "@zero9501/git-pincer-linux-x64-gnu";
  }
  if (platform === "win32" && arch === "x64") return "@zero9501/git-pincer-win32-x64";
  return null;
}

function binaryPath() {
  const pkg = platformPackage();
  if (!pkg) {
    console.error(`[git-pincer] unsupported platform: ${process.platform}-${process.arch}`);
    console.error("[git-pincer] see https://github.com/zlx2019/git-pincer#installation for other install options");
    process.exit(1);
  }
  const exe = process.platform === "win32" ? "git-pincer.exe" : "git-pincer";
  try {
    return path.join(path.dirname(require.resolve(`${pkg}/package.json`)), "bin", exe);
  } catch {
    console.error(`[git-pincer] platform package ${pkg} is missing; try reinstalling git-pincer`);
    process.exit(1);
  }
}

const result = spawnSync(binaryPath(), process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`[git-pincer] failed to launch binary: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 1);
