import { spawnSync } from "bun";
import { describe, expect, test } from "bun:test";
import { bunEnv, bunExe, isPosix, tempDir } from "harness";
import fs from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const publicTopLevelCommands = [
  "add",
  "audit",
  "build",
  "ci",
  "completions",
  "create",
  "exec",
  "feedback",
  "help",
  "info",
  "init",
  "install",
  "link",
  "outdated",
  "patch",
  "patch-commit",
  "pm",
  "poly",
  "publish",
  "remove",
  "repl",
  "run",
  "test",
  "unlink",
  "update",
  "upgrade",
  "why",
  "x",
];

describe("bun", () => {
  async function runHelp(args: string[]) {
    await using proc = Bun.spawn({
      cmd: [bunExe(), ...args],
      env: bunEnv,
      stdout: "pipe",
      stderr: "pipe",
    });
    const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
    const output = stdout + stderr;

    expect(output).not.toBeEmpty();
    expect(exitCode).toBe(0);
    return output;
  }

  describe("help", () => {
    test("short root help wraps long flags instead of panicking", async () => {
      const output = await runHelp(["-h"]);
      const lines = output.split(/\r?\n/);
      const flagLine = lines.findIndex(line => line.includes("--preserve-symlinks-main"));

      expect(output).toContain("Usage: poly <command>");
      expect(output).toContain("Flags:");
      expect(flagLine).toBeGreaterThan(-1);
      expect(lines[flagLine]!.trim()).toBe("--preserve-symlinks-main");
      expect(lines[flagLine + 1]!.trim()).toBe("Preserve symlinks when resolving the main entry point");
      expect(lines[flagLine + 1]!.indexOf("Preserve")).toBe(30);
    });

    test("test help preserves value names and aliases", async () => {
      const output = await runHelp(["test", "--help"]);

      expect(output).toContain("--rerun-each=<NUMBER>");
      expect(output).toContain("Re-run each test file the specified number of times");
      expect(output).toContain("--bail=<NUMBER>?");
      expect(output).toContain("Exit the test suite after the specified number of failures");
      expect(output).toContain("--test-name-pattern/--grep=<STR>");
      expect(output).not.toContain("Re-run each test file  times");
      expect(output).not.toContain("after  failures");
    });

    test("completions help documents generation and installation", async () => {
      const output = await runHelp(["completions", "--help"]);

      expect(output).toContain("poly completions [<shell>]");
      expect(output).toContain("poly completions install [<shell>] [--dir <path>]");
      expect(output).toContain("bash, zsh, or fish");
      expect(output).toContain("Defaults to the current shell from $SHELL");
      expect(output).toContain("Install into an existing directory instead of auto-detecting one");
      expect(output).toContain("poly completions bash > ~/.local/share/bash-completion/completions/poly.bash");
      expect(output).toContain("poly completions install zsh --dir ~/.zfunc");
    });

    test("exec help uses Poly spelling", async () => {
      const execHelp = await runHelp(["exec", "--help"]);

      expect(execHelp).toContain("Usage: poly exec");
      expect(execHelp).toContain('poly exec "echo hi"');
      expect(execHelp).not.toContain("bun exec");
    });

    test("create help uses Poly spelling", async () => {
      const createHelp = await runHelp(["create", "--help"]);

      expect(createHelp).toContain("poly create <MyReactComponent");
      expect(createHelp).toContain("polyx create-<template>");
      expect(createHelp).not.toContain("bun create <MyReactComponent");
      expect(createHelp).not.toContain("bunx create-<template>");
    });

    test("pm help uses Poly spelling", async () => {
      const [pmHelp, implicitPmHelp, pmPackHelp, pmScanHelp] = await Promise.all([
        runHelp(["pm", "--help"]),
        runHelp(["pm"]),
        runHelp(["pm", "pack", "--help"]),
        runHelp(["pm", "scan", "--help"]),
      ]);

      expect(pmHelp).toContain("Usage: poly pm");
      expect(pmHelp).toContain("[<command>]");
      expect(pmHelp).toContain("poly pm why <pkg>");
      expect(pmHelp).toContain("poly list");
      expect(pmHelp).toContain("use `poly info` instead");
      expect(pmHelp).not.toContain("Usage: bun pm");
      expect(pmHelp).not.toContain("bun list");

      expect(implicitPmHelp).toBe(pmHelp);

      expect(pmPackHelp).toContain("Usage: poly pm pack");
      expect(pmPackHelp).toContain("poly pm pack");
      expect(pmPackHelp).not.toContain("bun pm pack");

      expect(pmScanHelp).toContain("Usage: poly pm scan");
      expect(pmScanHelp).toContain("poly pm scan");
      expect(pmScanHelp).not.toContain("bun pm scan");
    });

    test("feedback help uses Poly spelling", async () => {
      const feedbackHelp = await runHelp(["feedback", "--help"]);

      expect(feedbackHelp).toContain("poly feedback");
      expect(feedbackHelp).not.toContain("bun feedback");
    });

    test("x help uses Poly spelling", async () => {
      const xHelp = await runHelp(["x", "--help"]);

      expect(xHelp).toContain("Usage: poly x");
      expect(xHelp).not.toContain("Usage: bun x");
    });

    test("package command help uses Poly spelling", async () => {
      const packageCommands = [
        "install",
        "update",
        "patch",
        "patch-commit",
        "add",
        "remove",
        "link",
        "unlink",
        "outdated",
        "publish",
        "audit",
        "info",
        "why",
      ];
      const packageHelp = await Promise.all(packageCommands.map(command => runHelp([command, "--help"])));

      for (const [index, command] of packageCommands.entries()) {
        const output = packageHelp[index]!;
        expect(output).toContain(`Usage: poly ${command}`);
        expect(output).not.toContain(`Usage: bun ${command}`);
        expect(output).not.toContain(`bun ${command}`);
        expect(output).not.toContain("https://bun.com/docs");
        expect(output).toContain("https://github.com/liooil/poly");
      }

      expect(packageHelp[10]).toContain("at or above the selected severity");
      expect(packageHelp[10]).not.toContain("severity greater than or equal to  (");

      expect(packageHelp[12]).toContain("Usage: poly why [flags] <package>");
      expect(packageHelp[12]).toContain("--depth=<NUM>");
      expect(packageHelp[12]).not.toContain("<@version>");
      expect(packageHelp[12]).not.toContain("[property path]");
    });
  });

  describe("NO_COLOR", () => {
    for (const value of ["1", "0", "foo", " "]) {
      test(`respects NO_COLOR=${JSON.stringify(value)} to disable color`, () => {
        const { stdout } = spawnSync({
          cmd: [bunExe()],
          env: {
            NO_COLOR: value,
          },
        });
        expect(stdout.toString()).not.toMatch(/\u001b\[\d+m/);
      });
    }
    for (const value of ["", undefined]) {
      // TODO: need a way to fake a tty in order to test this,
      // and cannot use FORCE_COLOR since that will always override NO_COLOR.
      test.todo(`respects NO_COLOR=${JSON.stringify(value)} to enable color`, () => {
        const { stdout } = spawnSync({
          cmd: [bunExe()],
          env:
            value === undefined
              ? {}
              : {
                  NO_COLOR: value,
                },
        });
        expect(stdout.toString()).toMatch(/\u001b\[\d+m/);
      });
    }
  });

  describe("revision", () => {
    test("revision generates version numbers correctly", () => {
      var { stdout, exitCode } = Bun.spawnSync({
        cmd: [bunExe(), "--version"],
        env: bunEnv,
        stderr: "inherit",
      });
      var version = stdout.toString().trim();

      var { stdout, exitCode } = Bun.spawnSync({
        cmd: [bunExe(), "--revision"],
        env: bunEnv,
        stderr: "inherit",
      });
      var revision = stdout.toString().trim();

      expect(exitCode).toBe(0);
      expect(revision).toStartWith(version);
      // https://semver.org/#is-there-a-suggested-regular-expression-regex-to-check-a-semver-string
      expect(revision).toMatch(
        /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/,
      );
    });
  });
  describe("getcompletes", () => {
    test("getcompletes includes every public top-level command", () => {
      const { stdout, exitCode } = spawnSync({
        cmd: [bunExe(), "getcompletes"],
        env: bunEnv,
      });
      const completions = stdout.toString().split(/\s+/).filter(Boolean);

      expect(completions).toEqual(expect.arrayContaining(publicTopLevelCommands));
      expect(completions).not.toContain("discord");
      expect(exitCode).toBe(0);
    });

    // https://github.com/oven-sh/bun/issues/30086
    test("getcompletes keeps scripts whose names start with 'pre'/'post' when no sibling script exists", () => {
      using dir = tempDir("getcompletes-pre-post", {
        "package.json": JSON.stringify({
          name: "test",
          scripts: {
            // standalone scripts — nothing named `ttier`, `pare-release`, `gres`, `css`, `view`
            "prettier": "echo prettier",
            "prettier:fix": "echo prettier:fix",
            "prepare-release": "echo prepare-release",
            "postgres": "echo postgres",
            "postcss": "echo postcss",
            "preview": "echo preview",
            // plain scripts
            "build": "echo build",
            "dev": "echo dev",
            "lint": "echo lint",
            "lint:fix": "echo lint:fix",
            "fix": "echo fix",
            "test": "echo test",
            // real lifecycle hooks — these SHOULD be hidden (sibling exists)
            "prebuild": "echo prebuild",
            "postbuild": "echo postbuild",
            "pretest": "echo pretest",
          },
        }),
      });

      for (const filter of ["s", "i", "r", "g", "z"]) {
        const { stdout, exitCode } = spawnSync({
          cmd: [bunExe(), "getcompletes", filter],
          env: bunEnv,
          cwd: String(dir),
        });
        const lines = stdout
          .toString()
          .split(/\s+/)
          .map(l => l.split("\t")[0]) // "z" filter emits "name\tdescription"
          .filter(Boolean);

        // standalone pre/post-prefixed scripts must be present
        expect(lines).toContain("prettier");
        expect(lines).toContain("prettier:fix");
        expect(lines).toContain("prepare-release");
        expect(lines).toContain("postgres");
        expect(lines).toContain("postcss");
        expect(lines).toContain("preview");

        // real npm lifecycle hooks (sibling `build`/`test` exists) must still be hidden
        expect(lines).not.toContain("prebuild");
        expect(lines).not.toContain("postbuild");
        expect(lines).not.toContain("pretest");

        expect(exitCode).toBe(0);
      }
    });
  });

  describe("completion scripts", () => {
    function createPolyxStubs(dir: string) {
      for (const name of ["polyx", "polyx-debug"]) {
        const executable = join(dir, name);
        fs.writeFileSync(executable, "");
        fs.chmodSync(executable, 0o755);
      }
    }

    async function writeCompletionScript(shell: string, dir: string) {
      createPolyxStubs(dir);

      await using proc = Bun.spawn({
        cmd: [bunExe(), "completions", shell],
        cwd: dir,
        env: { ...bunEnv, PATH: `${dir}:${bunEnv.PATH ?? ""}`, SHELL: "/unsupported-shell" },
        stdout: "pipe",
        stderr: "pipe",
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);

      expect(stderr).toBe("");
      expect(stdout).not.toBeEmpty();
      expect(exitCode).toBe(0);

      const completionPath = join(dir, `poly.${shell}`);
      fs.writeFileSync(completionPath, stdout);
      return completionPath;
    }

    test.concurrent.skipIf(!isPosix)("generates or installs completions explicitly", async () => {
      using dir = tempDir("poly-completion-command", {
        "package.json": JSON.stringify({ name: "completion-test" }),
      });
      const root = String(dir);
      const installDir = join(root, "installed");
      fs.mkdirSync(installDir);
      createPolyxStubs(root);

      await using generated = Bun.spawn({
        cmd: [bunExe(), "completions"],
        cwd: root,
        env: { ...bunEnv, PATH: `${root}:${bunEnv.PATH ?? ""}`, SHELL: "/bin/fish" },
        stdout: "pipe",
        stderr: "pipe",
      });
      const [generatedStdout, generatedStderr, generatedExitCode] = await Promise.all([
        generated.stdout.text(),
        generated.stderr.text(),
        generated.exited,
      ]);
      expect({
        stdoutContainsFishRegistration: generatedStdout.includes("complete -c poly"),
        stderr: generatedStderr,
        exitCode: generatedExitCode,
      }).toEqual({ stdoutContainsFishRegistration: true, stderr: "", exitCode: 0 });
      expect(fs.readdirSync(installDir)).toEqual([]);

      await using installed = Bun.spawn({
        cmd: [bunExe(), "completions", "install", "bash", "--dir", installDir],
        cwd: root,
        env: { ...bunEnv, PATH: `${root}:${bunEnv.PATH ?? ""}`, SHELL: "/unsupported-shell" },
        stdout: "pipe",
        stderr: "pipe",
      });
      const [installedStdout, installedStderr, installedExitCode] = await Promise.all([
        installed.stdout.text(),
        installed.stderr.text(),
        installed.exited,
      ]);
      expect({ stdout: installedStdout, exitCode: installedExitCode }).toEqual({ stdout: "", exitCode: 0 });
      expect(installedStderr).toContain(`Installed completions to ${installDir}/poly.bash`);
      expect(fs.readFileSync(join(installDir, "poly.bash"), "utf8")).toContain("complete -F _poly_completions poly");
      expect(fs.existsSync(join(installDir, "poly.completion.bash"))).toBe(false);

      const autoDataDir = join(root, "auto-data");
      const autoInstallDir = join(autoDataDir, "bash-completion", "completions");
      fs.mkdirSync(autoInstallDir, { recursive: true });
      await using autoInstalled = Bun.spawn({
        cmd: [bunExe(), "completions"],
        cwd: root,
        env: {
          ...bunEnv,
          IS_BUN_AUTO_UPDATE: "1",
          PATH: `${root}:${bunEnv.PATH ?? ""}`,
          SHELL: "/bin/bash",
          XDG_DATA_HOME: autoDataDir,
        },
        stdout: "pipe",
        stderr: "pipe",
      });
      const [autoStdout, autoStderr, autoExitCode] = await Promise.all([
        autoInstalled.stdout.text(),
        autoInstalled.stderr.text(),
        autoInstalled.exited,
      ]);
      expect({ stdout: autoStdout, exitCode: autoExitCode }).toEqual({ stdout: "", exitCode: 0 });
      expect(autoStderr).toContain(`Installed completions to ${autoInstallDir}/poly.bash`);
      expect(fs.readFileSync(join(autoInstallDir, "poly.bash"), "utf8")).toContain(
        "complete -F _poly_completions poly",
      );
    });

    test.concurrent.skipIf(!isPosix)("rejects unsupported completion arguments", async () => {
      using dir = tempDir("poly-completion-errors", {});
      const root = String(dir);

      const run = async (args: string[]) => {
        await using proc = Bun.spawn({
          cmd: [bunExe(), "completions", ...args],
          cwd: root,
          env: { ...bunEnv, SHELL: "/bin/bash" },
          stdout: "pipe",
          stderr: "pipe",
        });
        const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
        return { stdout, stderr, exitCode };
      };

      const unsupported = await run(["nu"]);
      expect(unsupported.stdout).toBe("");
      expect(unsupported.stderr).toContain("Unsupported shell 'nu'. Expected bash, zsh, or fish.");
      expect(unsupported.exitCode).toBe(1);

      const generateWithDir = await run(["bash", "--dir", root]);
      expect(generateWithDir.stdout).toBe("");
      expect(generateWithDir.stderr).toContain("--dir can only be used with 'poly completions install'.");
      expect(generateWithDir.exitCode).toBe(1);
    });

    const bash = Bun.which("bash");
    test.concurrent.skipIf(!isPosix || !bash)("bash registers poly and completes every public command", async () => {
      using dir = tempDir("poly-completion-bash", {
        "package.json": JSON.stringify({ name: "completion-test" }),
      });
      const completionPath = await writeCompletionScript("bash", String(dir));
      const driver = [
        'poly() { "$POLY_EXE" "$@"; }',
        'source "$COMPLETION_SCRIPT"',
        "complete -p poly",
        'COMP_WORDS=(poly "")',
        "COMP_CWORD=1",
        "_poly_completions",
        `printf 'candidate=%s\\n' "\${COMPREPLY[@]}"`,
        'COMP_WORDS=(poly completions "")',
        "COMP_CWORD=2",
        "_poly_completions",
        `printf 'completion_candidate=%s\\n' "\${COMPREPLY[@]}"`,
      ].join("\n");
      await using proc = Bun.spawn({
        cmd: [bash!, "--noprofile", "--norc", "-c", driver],
        cwd: String(dir),
        env: { ...bunEnv, COMPLETION_SCRIPT: completionPath, POLY_EXE: bunExe() },
        stdout: "pipe",
        stderr: "pipe",
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      const candidates = stdout
        .split(/\r?\n/)
        .filter(line => line.startsWith("candidate="))
        .map(line => line.slice("candidate=".length));
      const completionCandidates = stdout
        .split(/\r?\n/)
        .filter(line => line.startsWith("completion_candidate="))
        .map(line => line.slice("completion_candidate=".length));

      expect(stderr).toBe("");
      expect(stdout).toContain("complete -F _poly_completions poly");
      expect(candidates).toEqual(expect.arrayContaining(publicTopLevelCommands));
      expect(candidates).not.toContain("bun");
      expect(candidates).not.toContain("dev");
      expect(candidates).not.toContain("discord");
      expect(completionCandidates).toEqual(expect.arrayContaining(["bash", "zsh", "fish", "install"]));
      expect(exitCode).toBe(0);
    });

    const zsh = Bun.which("zsh");
    test.concurrent.skipIf(!isPosix || !zsh)("zsh registers poly and completes every public command", async () => {
      using dir = tempDir("poly-completion-zsh", {
        "package.json": JSON.stringify({ name: "completion-test" }),
      });
      const completionPath = await writeCompletionScript("zsh", String(dir));
      const driver = [
        "autoload -Uz compinit",
        "compinit -D",
        'poly() { "$POLY_EXE" "$@"; }',
        'bun() { print -u2 -- "stale bun completion call"; return 127; }',
        'source "$COMPLETION_SCRIPT"',
        'print -r -- "registration=${_comps[poly]-missing}"',
        "_arguments() { state=cmd; return 0; }",
        "_alternative() {",
        "  local item",
        "  for item in $main_commands; do",
        '    print -r -- "candidate=$item"',
        "  done",
        "}",
        "_poly",
        "_arguments() { state=completion-action; line=(); return 0; }",
        "_values() {",
        "  shift",
        "  local item",
        '  for item in "$@"; do',
        '    print -r -- "completion_candidate=${item%%\\[*}"',
        "  done",
        "}",
        "_poly_completions_completion",
      ].join("\n");
      await using proc = Bun.spawn({
        cmd: [zsh!, "-f", "-c", driver],
        cwd: String(dir),
        env: { ...bunEnv, COMPLETION_SCRIPT: completionPath, POLY_EXE: bunExe() },
        stdout: "pipe",
        stderr: "pipe",
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      const candidates = stdout
        .split(/\r?\n/)
        .filter(line => line.startsWith("candidate="))
        .map(line => line.slice("candidate=".length).split("\\:")[0]);
      const completionCandidates = stdout
        .split(/\r?\n/)
        .filter(line => line.startsWith("completion_candidate="))
        .map(line => line.slice("completion_candidate=".length));

      expect(stderr).toBe("");
      expect(stdout).toContain("registration=_poly");
      expect(candidates).toEqual(expect.arrayContaining(publicTopLevelCommands));
      expect(candidates).not.toContain("bun");
      expect(candidates).not.toContain("dev");
      expect(candidates).not.toContain("discord");
      expect(completionCandidates).toEqual(expect.arrayContaining(["bash", "zsh", "fish", "install"]));
      expect(exitCode).toBe(0);
    });

    const fish = Bun.which("fish");
    test.concurrent.skipIf(!isPosix || !fish)("fish registers poly and completes every public command", async () => {
      using dir = tempDir("poly-completion-fish", {
        "package.json": JSON.stringify({ name: "completion-test" }),
      });
      const completionPath = await writeCompletionScript("fish", String(dir));
      const driver = [
        "function poly",
        '  "$POLY_EXE" $argv',
        "end",
        'source "$COMPLETION_SCRIPT"',
        `complete -C "poly " | string replace -r '\\t.*$' '' | string replace -r '^' 'candidate='`,
        `complete -C "poly completions " | string replace -r '\\t.*$' '' | string replace -r '^' 'completion_candidate='`,
      ].join("\n");
      await using proc = Bun.spawn({
        cmd: [fish!, "--no-config", "-c", driver],
        cwd: String(dir),
        env: { ...bunEnv, COMPLETION_SCRIPT: completionPath, POLY_EXE: bunExe() },
        stdout: "pipe",
        stderr: "pipe",
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      const candidates = stdout
        .split(/\r?\n/)
        .filter(line => line.startsWith("candidate="))
        .map(line => line.slice("candidate=".length));
      const completionCandidates = stdout
        .split(/\r?\n/)
        .filter(line => line.startsWith("completion_candidate="))
        .map(line => line.slice("completion_candidate=".length));

      expect(stderr).toBe("");
      expect(candidates).toEqual(expect.arrayContaining(publicTopLevelCommands));
      expect(candidates).not.toContain("bun");
      expect(candidates).not.toContain("dev");
      expect(candidates).not.toContain("discord");
      expect(completionCandidates).toEqual(expect.arrayContaining(["bash", "zsh", "fish", "install"]));
      expect(exitCode).toBe(0);
    });
  });

  describe("test command line arguments", () => {
    test("test --config, issue #4128", () => {
      const path = `${tmpdir()}/bunfig-${Date.now()}.toml`;
      fs.writeFileSync(path, "[debug]");

      const p = Bun.spawnSync({
        cmd: [bunExe(), "--config=" + path],
        env: {},
        stderr: "inherit",
      });
      try {
        expect(p.exitCode).toBe(0);
      } finally {
        fs.unlinkSync(path);
      }
    });
  });
});
