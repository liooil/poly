import { describe, expect, test } from "bun:test";
import { isWindows, tempDir } from "harness";
import { chmodSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { resolveLlvmToolchain } from "../../../scripts/build/tools.ts";

const requiredTools = ["clang", "clang++", "llvm-ar", "llvm-ranlib", "llvm-strip"];

const rollingLlvmFiles = [
  ".buildkite/Dockerfile",
  ".github/workflows/bun-integration.yml",
  "CONTRIBUTING.md",
  "docs/project/building-windows.mdx",
  "docs/project/contributing.mdx",
  "flake.nix",
  "poly/scripts/setup-bun-windows-ci.ps1",
  "scripts/bootstrap.ps1",
  "scripts/bootstrap.sh",
  "scripts/build/config.ts",
  "scripts/build/tools.ts",
  "scripts/lldb-inline-tool.cpp",
  "scripts/run-clang-format.sh",
  "shell.nix",
  "src/crash_handler/lib.rs",
  "test/cli/run/run-crash-handler.test.ts",
  "test/napi/node-napi-tests/harness.ts",
];

function toolFiles(suffix: string, version: string): Record<string, string> {
  const script = `#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "${version}"
elif [ "$1" = "-print-resource-dir" ]; then
  echo "/fake/clang-resource-dir"
fi
`;
  return Object.fromEntries(requiredTools.map(name => [`${name}${suffix}`, script]));
}

function makeExecutable(dir: string, suffix: string): void {
  for (const name of requiredTools) chmodSync(join(dir, `${name}${suffix}`), 0o755);
}

function withToolPath<T>(dir: string, callback: () => T): T {
  const previousPath = process.env.PATH;
  const previousCargoHome = process.env.CARGO_HOME;
  process.env.PATH = dir;
  process.env.CARGO_HOME = join(dir, "missing-cargo-home");
  try {
    return callback();
  } finally {
    if (previousPath === undefined) delete process.env.PATH;
    else process.env.PATH = previousPath;
    if (previousCargoHome === undefined) delete process.env.CARGO_HOME;
    else process.env.CARGO_HOME = previousCargoHome;
  }
}

describe.skipIf(isWindows)("LLVM toolchain discovery", () => {
  test("uses the unversioned LLVM toolchain without pinning its release", () => {
    using dir = tempDir("llvm-system-default", toolFiles("", "clang version 23.1.0"));
    makeExecutable(String(dir), "");

    const toolchain = withToolPath(String(dir), () => resolveLlvmToolchain("freebsd", "x64"));

    expect(toolchain.cc).toBe(join(String(dir), "clang"));
    expect(toolchain.cxx).toBe(join(String(dir), "clang++"));
    expect(toolchain.clangVersion).toBe("23.1.0");
  });

  test("does not fall back to version-suffixed LLVM tools", () => {
    using dir = tempDir("llvm-versioned-only", toolFiles("-21", "clang version 21.1.8"));
    makeExecutable(String(dir), "-21");

    expect(() => withToolPath(String(dir), () => resolveLlvmToolchain("freebsd", "x64"))).toThrow(
      "Could not find clang",
    );
  });

  test("continues to reject Apple Clang", () => {
    using dir = tempDir("apple-clang", {
      ...toolFiles("", "Apple clang version 18.0.0"),
      brew: "#!/bin/sh\necho /nonexistent\n",
      dsymutil: "#!/bin/sh\n",
    });
    makeExecutable(String(dir), "");
    chmodSync(join(String(dir), "brew"), 0o755);
    chmodSync(join(String(dir), "dsymutil"), 0o755);

    expect(() => withToolPath(String(dir), () => resolveLlvmToolchain("darwin", "x64"))).toThrow(
      "Apple Clang is not supported",
    );
  });
});

test("active setup does not pin an LLVM release line", () => {
  const root = join(import.meta.dir, "../../..");
  const versionedTool = /\b(?:clang(?:\+\+|-cl|-format)?|llvm(?:-symbolizer)?|lld)(?:@|[-_])?\d+(?:\.\d+)*\b/gi;
  const versionVariable = /\b(?:REPORTED_)?LLVM_VERSION(?:_MAJOR)?\b/g;
  const pins = rollingLlvmFiles.flatMap(file => {
    const source = readFileSync(join(root, file), "utf8");
    return [...source.matchAll(versionedTool), ...source.matchAll(versionVariable)].map(match => ({
      file,
      value: match[0],
    }));
  });

  expect(pins).toEqual([]);
});
