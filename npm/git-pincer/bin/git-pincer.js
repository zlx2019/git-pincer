#!/usr/bin/env node
"use strict";

// git-pincer 的 npm 启动垫片:
// 按当前平台定位 optionalDependencies 里对应的二进制子包, 并把参数原样转发给它。

const { spawnSync } = require("node:child_process");
const path = require("node:path");

// 判断 Linux 是否为 musl 环境(运行时报告中没有 glibc 版本信息即视为 musl)
function isMusl() {
  try {
    return !process.report.getReport().header.glibcVersionRuntime;
  } catch {
    return false;
  }
}

// 根据平台与架构推导对应的二进制子包名, 不支持的平台返回 null
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

// 定位子包内的可执行文件绝对路径, 失败时输出指引并退出
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
  // 被信号终止时对自身重放同一信号, 让退出码符合 shell 约定(128+N)
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 1);
