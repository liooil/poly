import { spawnSync } from "bun";
import { dlopen, FFIType } from "bun:ffi";
import { describe, expect, test } from "bun:test";
import { bunEnv, bunExe, isDebug, isMusl, isPosix, isWindows, tempDir } from "harness";
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
  "dedupe",
  "exec",
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
  "prune",
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
      expect(output).toContain("Re-run each test file <NUMBER> times");
      expect(output).toContain("--bail=<NUMBER>?");
      expect(output).toContain("Exit the test suite after <NUMBER> failures");
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
        "dedupe",
        "remove",
        "link",
        "unlink",
        "outdated",
        "prune",
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

      const helpByCommand = new Map(packageCommands.map((command, index) => [command, packageHelp[index]!]));

      expect(helpByCommand.get("audit")).toContain("severity greater than or equal to <level>");
      expect(helpByCommand.get("audit")).not.toContain("severity greater than or equal to  (");

      expect(helpByCommand.get("why")).toContain("Usage: poly why [flags] <package>");
      expect(helpByCommand.get("why")).toContain("--depth=<NUM>");
      expect(helpByCommand.get("why")).not.toContain("<@version>");
      expect(helpByCommand.get("why")).not.toContain("[property path]");
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

  // #39762: a piped stream must not get ANSI codes because the other stream
  // is a TTY. `bun test | pbcopy` copied raw escape codes to the clipboard
  // since stderr (a TTY) forced colors onto the piped stdout.
  //
  // openpty via bun:ffi so one stdio fd can be a real TTY while the other is
  // a pipe. glibc keeps openpty in libutil; musl and macOS keep everything in
  // libc. Same pattern as test/js/bun/terminal/terminal-spawn.test.ts.
  describe.skipIf(isWindows)("per-stream color detection", () => {
    const colorEnv = {
      ...bunEnv,
      NO_COLOR: undefined,
      FORCE_COLOR: undefined,
      TERM: "xterm-256color",
    };

    const openptyDecl = {
      openpty: {
        args: [FFIType.ptr, FFIType.ptr, FFIType.ptr, FFIType.ptr, FFIType.ptr],
        returns: FFIType.i32,
      },
    } as const;
    const closeDecl = {
      close: { args: [FFIType.i32], returns: FFIType.i32 },
    } as const;

    function openPty(): { slave: number; close(): void } {
      const lib =
        process.platform === "darwin"
          ? dlopen("libc.dylib", { ...openptyDecl, ...closeDecl })
          : isMusl
            ? dlopen(process.arch === "arm64" ? "libc.musl-aarch64.so.1" : "libc.musl-x86_64.so.1", {
                ...openptyDecl,
                ...closeDecl,
              })
            : dlopen("libutil.so.1", openptyDecl);
      const libc = process.platform === "darwin" || isMusl ? lib : dlopen("libc.so.6", closeDecl);

      const masterBuf = new Int32Array(1);
      const slaveBuf = new Int32Array(1);
      expect((lib.symbols as any).openpty(masterBuf, slaveBuf, null, null, null)).toBe(0);
      return {
        slave: slaveBuf[0],
        close() {
          (libc.symbols as any).close(masterBuf[0]);
          (libc.symbols as any).close(slaveBuf[0]);
        },
      };
    }

    test.concurrent("piped stdout stays plain when stderr is a tty", async () => {
      const pty = openPty();
      try {
        await using proc = Bun.spawn({
          cmd: [bunExe(), "-e", "console.log({ a: 1 })"],
          env: colorEnv,
          stdin: "ignore",
          stdout: "pipe",
          stderr: pty.slave,
        });
        const [stdout, exitCode] = await Promise.all([proc.stdout.text(), proc.exited]);
        expect(stdout).not.toMatch(/\u001b\[/);
        expect(stdout).toContain("a: 1");
        expect(exitCode).toBe(0);
      } finally {
        pty.close();
      }
    });

    test.concurrent("piped stderr stays plain when stdout is a tty", async () => {
      const pty = openPty();
      try {
        await using proc = Bun.spawn({
          cmd: [bunExe(), "-e", "console.error({ a: 1 })"],
          env: colorEnv,
          stdin: "ignore",
          stdout: pty.slave,
          stderr: "pipe",
        });
        const [stderr, exitCode] = await Promise.all([proc.stderr.text(), proc.exited]);
        expect(stderr).not.toMatch(/\u001b\[/);
        expect(stderr).toContain("a: 1");
        expect(exitCode).toBe(0);
      } finally {
        pty.close();
      }
    });

    // Guard against overcorrection: a stream that is itself a TTY keeps
    // colors.
    test.concurrent("a tty stdout still gets colors", async () => {
      let output = "";
      const decoder = new TextDecoder();
      await using terminal = new Bun.Terminal({
        data(_t, chunk: Uint8Array) {
          output += decoder.decode(chunk, { stream: true });
        },
      });
      const proc = Bun.spawn({
        cmd: [bunExe(), "-e", "console.log({ a: 1 })"],
        env: colorEnv,
        terminal,
      });
      await proc.exited;
      // PTY data can still be in flight after waitpid. Poll with a deadline
      // (below the 5s test timeout) so a regression fails here with the
      // captured output, not by timeout.
      const deadline = Date.now() + 2_000;
      while (!/\u001b\[\d+m/.test(output) && Date.now() < deadline) {
        await Bun.sleep(10);
      }
      expect(output).toMatch(/\u001b\[\d+m/);
    });
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

  // On Windows `poly completions` installs polyx as a hardlink (or a .cmd shim) instead of a symlink.
  describe.skipIf(isWindows)("completions", () => {
    const polyxName = isDebug ? "polyx-debug" : "polyx";

    test("installs a polyx symlink to the executable, falling back through the install directories", async () => {
      using dir = tempDir("completions-polyx", {
        "bin": {},
        "completions": {},
        "empty-path": {},
        "install": { bin: {} },
        "home-empty": {},
        "home-poly": { ".bun": { bin: {} } },
        "home-local": { ".local": { bin: {} } },
      });
      // Run a private copy of the executable so that the first candidate location, the executable's
      // own directory, is inside the temporary directory. A hardlink avoids copying the binary; fall
      // back to a copy when the temporary directory is on another filesystem.
      const exe = join(String(dir), "bin", "poly");
      try {
        fs.linkSync(fs.realpathSync(bunExe()), exe);
      } catch {
        fs.copyFileSync(bunExe(), exe);
      }
      // The link is created against the resolved executable path.
      const exeRealpath = fs.realpathSync(exe);

      async function installPolyx(env: Record<string, string | undefined>) {
        await using proc = Bun.spawn({
          cmd: [exe, "completions", "install", "bash", "--dir", join(String(dir), "completions")],
          env: {
            ...bunEnv,
            PATH: join(String(dir), "empty-path"),
            IS_BUN_AUTO_UPDATE: "1",
            BUN_INSTALL: undefined,
            ...env,
          },
          stdout: "pipe",
          stderr: "pipe",
        });
        const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
        expect(stdout).toBe("");
        expect(stderr).toContain("Installed completions");
        expect(exitCode).toBe(0);
      }

      // 1. Next to the executable.
      await installPolyx({ HOME: join(String(dir), "home-empty") });
      expect(fs.readlinkSync(join(String(dir), "bin", polyxName))).toBe(exeRealpath);

      // That link now exists, so every following run falls through to the next location.
      // 2. $BUN_INSTALL/bin
      await installPolyx({ HOME: join(String(dir), "home-empty"), BUN_INSTALL: join(String(dir), "install") });
      expect(fs.readlinkSync(join(String(dir), "install", "bin", polyxName))).toBe(exeRealpath);

      // 3. $HOME/.bun/bin
      await installPolyx({ HOME: join(String(dir), "home-poly") });
      expect(fs.readlinkSync(join(String(dir), "home-poly", ".bun", "bin", polyxName))).toBe(exeRealpath);

      // 4. $HOME/.local/bin, once $HOME/.bun/bin does not exist.
      await installPolyx({ HOME: join(String(dir), "home-local") });
      expect(fs.readlinkSync(join(String(dir), "home-local", ".local", "bin", polyxName))).toBe(exeRealpath);
    });
  });
  describe("--help preserves <placeholder> text", () => {
    const env = { ...bunEnv, NO_COLOR: "1" };
    const usage: [string, string][] = [
      ["install", "poly install [flags] <name>@<version>"],
      ["add", "poly add [flags] <package><@version>"],
      ["remove", "poly remove [flags] [<packages>]"],
      ["update", "poly update [flags] <name>@<version>"],
      ["link", "poly link [flags] [<packages>]"],
      ["patch", "poly patch [flags or options] <package>@<version>"],
      ["patch-commit", "poly patch-commit [flags or options] <directory>"],
      ["info", "poly info [flags] <package>[@<version>]"],
    ];
    test.concurrent.each(usage)("poly %s --help usage line", async (cmd, expected) => {
      await using proc = Bun.spawn({ cmd: [bunExe(), cmd, "--help"], env, stderr: "pipe" });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      const line = (stdout + stderr).split(/\r?\n/).find(l => l.startsWith("Usage:")) ?? "";
      expect(line).toBe(`Usage: ${expected}`);
      expect(exitCode).toBe(0);
    });

    const flags: [string, string, string][] = [
      ["audit", "--audit-level", "greater than or equal to <level> (low,"],
      ["test", "--rerun-each", "Re-run each test file <NUMBER> times"],
      ["test", "--bail", "Exit the test suite after <NUMBER> failures"],
      ["build", "--allow-unresolved", "Use '<empty>' for opaque specifiers"],
      ["add", "-F, --filter", "Add the package(s) to the matching workspaces instead of the current package"],
      ["add", "--catalog", 'depend on it as "catalog:" (use --catalog=NAME for a named catalog)'],
      ["remove", "-F, --filter", "Remove the package(s) from the matching workspaces instead of the current package"],
      ["install", "-F, --filter", "Install packages for the matching workspaces"],
      ["install", "--catalog", 'depend on it as "catalog:" (use --catalog=NAME for a named catalog)'],
      ["audit", "--ignore", "Ignore advisories by GHSA or numeric advisory ID (repeatable)"],
      ["audit", "-L, --latest", "Also apply fixes your declared ranges exclude, rewriting package.json"],
      ["update", "-p, --production", "Only update dependencies and optionalDependencies (alias: --prod)"],
      ["install", "-p, --production", "Don't install devDependencies"],
    ];
    test.concurrent.each(flags)("poly %s --help keeps placeholder in %s description", async (cmd, flag, expected) => {
      await using proc = Bun.spawn({ cmd: [bunExe(), cmd, "--help"], env, stderr: "pipe" });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      const line = (stdout + stderr).split(/\r?\n/).find(l => l.includes(flag)) ?? "";
      expect(line).toContain(expected);
      expect(exitCode).toBe(0);
    });

    test("poly add --help usage line is intact with FORCE_COLOR=1", async () => {
      await using proc = Bun.spawn({
        cmd: [bunExe(), "add", "--help"],
        env: { ...bunEnv, NO_COLOR: undefined, FORCE_COLOR: "1" },
        stderr: "pipe",
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      const out = stdout + stderr;
      // <blue>\<package\><r> renders to \x1b[34m<package>\x1b[0m, not \x1b[34m\x1b[0m
      expect(out).toContain("\x1b[34m<package>\x1b[0m");
      // raw tag markup must not leak through
      expect(out).not.toContain("<blue>");
      expect(exitCode).toBe(0);
    });
  });

  describe("--help lists package manager commands and examples", () => {
    const env = { ...bunEnv, NO_COLOR: "1" };
    const lines: [string, string[], RegExp[]][] = [
      [
        "poly --help",
        [],
        [
          /^ {2}dedupe +Remove duplicate versions from the lockfile$/m,
          /^ {2}prune +Remove packages that are not in the lockfile from node_modules$/m,
        ],
      ],
      [
        "poly pm --help",
        ["pm"],
        [
          /^ {2}poly list +list the dependency tree according to the current lockfile$/m,
          /^ {2}poly pm licenses +list installed packages grouped by license$/m,
        ],
      ],
      [
        "poly add --help",
        ["add"],
        [
          /^ {2}Add a dependency to a specific workspace in a monorepo\n {2}poly add zod --filter api$/m,
          /^ {2}Add to the workspace catalog instead of pinning a version\n {2}poly add --catalog react\n {2}poly add --catalog=testing vitest$/m,
        ],
      ],
      [
        "poly audit --help",
        ["audit"],
        [
          /^ {2}poly audit fix upgrades vulnerable packages to the lowest safe version that still satisfies every dependent's range\.$/m,
          /^ {2}Upgrade vulnerable packages in bun\.lock and node_modules; package\.json is only changed when an exact pin has to be bumped\.\n {2}poly audit fix$/m,
          /^ {2}Show what poly audit fix would change without changing anything\.\n {2}poly audit fix --dry-run$/m,
          /^ {2}Also apply fixes that your package\.json ranges exclude, rewriting those ranges\.\n {2}poly audit fix --latest$/m,
        ],
      ],
    ];
    test.concurrent.each(lines)("%s", async (_, args, expected) => {
      await using proc = Bun.spawn({ cmd: [bunExe(), ...args, "--help"], env, stderr: "pipe" });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      const out = (stdout + stderr).replaceAll("\r\n", "\n");
      for (const re of expected) expect(out).toMatch(re);
      expect(out).not.toContain("bun list ");
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
