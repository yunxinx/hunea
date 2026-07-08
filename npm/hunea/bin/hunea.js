#!/usr/bin/env node
import { spawn } from "node:child_process";
import { existsSync, realpathSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const require = createRequire(import.meta.url);

const TARGET_BY_PLATFORM = {
  "linux:x64": {
    packageName: "hunea-linux-x64",
    targetTriple: "x86_64-unknown-linux-musl",
  },
  "linux:arm64": {
    packageName: "hunea-linux-arm64",
    targetTriple: "aarch64-unknown-linux-musl",
  },
  "darwin:x64": {
    packageName: "hunea-darwin-x64",
    targetTriple: "x86_64-apple-darwin",
  },
  "darwin:arm64": {
    packageName: "hunea-darwin-arm64",
    targetTriple: "aarch64-apple-darwin",
  },
  "win32:x64": {
    packageName: "hunea-win32-x64",
    targetTriple: "x86_64-pc-windows-msvc",
  },
};

const platformKey = `${process.platform}:${process.arch}`;
const platformTarget = TARGET_BY_PLATFORM[platformKey];

if (!platformTarget) {
  throw new Error(
    `Unsupported platform: ${process.platform} (${process.arch})`,
  );
}

function detectPackageManager() {
  const userAgent = process.env.npm_config_user_agent || "";
  if (/\bbun\//.test(userAgent)) {
    return "bun";
  }

  const execPath = process.env.npm_execpath || "";
  if (execPath.includes("bun")) {
    return "bun";
  }

  return userAgent ? "npm" : null;
}

function reinstallHint() {
  return detectPackageManager() === "bun"
    ? "bun install -g hunea@latest"
    : "npm install -g hunea@latest";
}

function vendorRoots() {
  const roots = [];

  try {
    const packageJsonPath = require.resolve(
      `${platformTarget.packageName}/package.json`,
    );
    roots.push(path.join(path.dirname(packageJsonPath), "vendor"));
  } catch {
    // npm may omit optionalDependencies for unsupported platforms; report a
    // useful reinstall hint below if the staged fallback is also missing.
  }

  // Allows `npm pack` staging checks before platform packages are published.
  roots.push(path.join(__dirname, "..", "vendor"));
  return roots;
}

function findExecutable() {
  const executableName = process.platform === "win32" ? "hunea.exe" : "hunea";
  for (const vendorRoot of vendorRoots()) {
    const candidate = path.join(
      vendorRoot,
      platformTarget.targetTriple,
      "bin",
      executableName,
    );
    if (existsSync(candidate)) {
      return candidate;
    }
  }

  throw new Error(
    `Missing optional dependency ${platformTarget.packageName}. Reinstall hunea: ${reinstallHint()}`,
  );
}

const binaryPath = findExecutable();
const packageManagerEnvVar =
  detectPackageManager() === "bun"
    ? "HUNEA_MANAGED_BY_BUN"
    : "HUNEA_MANAGED_BY_NPM";

const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: "inherit",
  env: {
    ...process.env,
    [packageManagerEnvVar]: "1",
    HUNEA_MANAGED_PACKAGE_ROOT: realpathSync(path.join(__dirname, "..")),
  },
});

child.on("error", (error) => {
  console.error(error);
  process.exit(1);
});

const forwardSignal = (signal) => {
  if (child.killed) {
    return;
  }
  try {
    child.kill(signal);
  } catch {
    // The child may have already exited.
  }
};

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(signal, () => forwardSignal(signal));
}

const childResult = await new Promise((resolve) => {
  child.on("exit", (code, signal) => {
    if (signal) {
      resolve({ type: "signal", signal });
    } else {
      resolve({ type: "code", exitCode: code ?? 1 });
    }
  });
});

if (childResult.type === "signal") {
  process.kill(process.pid, childResult.signal);
} else {
  process.exit(childResult.exitCode);
}
