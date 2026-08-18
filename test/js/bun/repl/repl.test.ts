// Tests for Bun REPL
import { describe, expect, test } from "bun:test";
import { bunEnv, bunExe, isWindows, tempDir } from "harness";
import { chmodSync, statSync } from "node:fs";
import path from "path";

// Helper to run REPL with piped stdin (non-TTY mode) and capture output
async function runRepl(
  input: string | string[],
  options: {
    env?: Record<string, string>;
    args?: string[];
  } = {},
): Promise<{ stdout: string; stderr: string; exitCode: number }> {
  const inputStr = Array.isArray(input) ? input.join("\n") + "\n" : input;
  const { env = {}, args = [] } = options;

  await using proc = Bun.spawn({
    cmd: [bunExe(), "repl", ...args],
    stdin: Buffer.from(inputStr),
    stdout: "pipe",
    stderr: "pipe",
    env: {
      ...bunEnv,
      TERM: "dumb",
      NO_COLOR: "1",
      ...env,
    },
  });

  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);

  return { stdout, stderr, exitCode };
}

const stripAnsi = Bun.stripANSI;

// Helper to run REPL in a PTY and interact with it
async function withTerminalRepl(
  fn: (helpers: {
    terminal: Bun.Terminal;
    proc: Bun.ChildProcess;
    send: (text: string) => void;
    waitFor: (pattern: string | RegExp, timeoutMs?: number) => Promise<string>;
    allOutput: () => string;
    rawOutput: () => string;
  }) => Promise<void>,
  options: {
    args?: string[];
    env?: Record<string, string | undefined>;
    startupTimeoutMs?: number;
    cols?: number;
  } = {},
) {
  const { args = [], env = {}, startupTimeoutMs = 5000, cols = 120 } = options;
  const received: string[] = [];
  let cursor = 0;
  let resolveWaiter: (() => void) | null = null;

  await using terminal = new Bun.Terminal({
    cols,
    rows: 40,
    data(_term, data) {
      const str = Buffer.from(data).toString();
      received.push(str);
      if (resolveWaiter) {
        resolveWaiter();
        resolveWaiter = null;
      }
    },
  });

  await using proc = Bun.spawn({
    cmd: [bunExe(), "repl", ...args],
    terminal,
    env: {
      ...bunEnv,
      TERM: "xterm-256color",
      ...env,
    },
  });

  const send = (text: string) => terminal.write(text);

  const waitFor = async (pattern: string | RegExp, timeoutMs = 5000): Promise<string> => {
    const deadline = Date.now() + timeoutMs;
    while (true) {
      const all = received.join("");
      const recent = all.slice(cursor);
      const matched = typeof pattern === "string" ? recent.includes(pattern) : pattern.test(recent);
      if (matched) {
        cursor = all.length;
        return recent;
      }
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        throw new Error(
          `Timed out waiting for pattern: ${pattern}\nReceived so far:\n${stripAnsi(received.join("").slice(cursor))}`,
        );
      }
      // Wait for the next chunk of terminal data (or time out).

      await new Promise<void>(resolve => {
        resolveWaiter = resolve;
      });
      resolveWaiter = null;
    }
  };

  const allOutput = () => stripAnsi(received.join(""));
  const rawOutput = () => received.join("");

  await waitFor(/\u276f|> /, startupTimeoutMs); // Wait for prompt

  await fn({ terminal, proc, send, waitFor, allOutput, rawOutput });

  // Clean exit
  send(".exit\n");
  await Promise.race([proc.exited, Bun.sleep(2000)]);
  if (!proc.killed) proc.kill();
}

describe.concurrent("Bun REPL", () => {
  test("repl --help shows REPL-specific help", async () => {
    await using proc = Bun.spawn({
      cmd: [bunExe(), "repl", "--help"],
      stdout: "pipe",
      stderr: "pipe",
      env: { ...bunEnv, NO_COLOR: "1" },
    });
    const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);

    expect(stderr).toBe("");
    expect(stdout).toMatchInlineSnapshot(`
      "Usage: poly repl [options] [mode]

      Start an interactive REPL in JavaScript, Python, Shell, SQL, or C mode.

      Arguments:
        mode                        js, python (py), shell (sh), sql, or c. Defaults to js.

      Options:
        -e, --eval=<STR>           Evaluate argument as a script
        -p, --print=<STR>          Evaluate argument as a script and print the result
        -r, --preload=<STR>...     Import a module before other modules are loaded
            --require=<STR>...     Alias of --preload, for Node.js compatibility
            --import=<STR>...      Alias of --preload, for Node.js compatibility
            --cwd=<STR>            Absolute path to resolve files & entry points from. This just changes the process' cwd.
            --env-file=<STR>...    Load environment variables from the specified file(s)
            --no-env-file          Disable automatic loading of .env files
            --inspect=<STR>?       Activate Poly's debugger
            --smol                 Use less memory, but run garbage collection more often
        -c, --config=<PATH>?       Specify path to bunfig.toml. Default $cwd/bunfig.toml
        -h, --help                 Display this menu and exit

      --eval and --print are only supported in JavaScript mode.

      Examples:
        poly repl
        poly repl python
        poly repl shell
        poly repl --preload ./setup.ts
        poly repl -p 'const x: number = 21; x * 2'
        poly repl --inspect

      Inside the REPL, use .help to list interactive commands.

      Learn more about Poly at https://github.com/liooil/poly
      "
    `);
    expect(exitCode).toBe(0);
  });

  test.each([
    ["js", "JavaScript"],
    ["python", "Python (embedded RustPython)"],
    ["py", "Python (embedded RustPython)"],
    ["shell", "Shell (Bun Shell)"],
    ["sh", "Shell (Bun Shell)"],
    ["sql", "SQL (in-memory SQLite)"],
    ["c", "C (embedded TinyCC)"],
  ])("repl %s starts directly in the requested mode", async (mode, expectedMode) => {
    const { stdout, stderr, exitCode } = await runRepl([".mode", ".exit"], { args: [mode] });
    const output = stripAnsi(stdout);

    expect(output).toContain(`Current mode: ${expectedMode}`);
    expect(stderr).toBe("");
    expect(exitCode).toBe(0);
  });

  test("repl rejects an unknown mode", async () => {
    const { stdout, stderr, exitCode } = await runRepl("", { args: ["ruby"] });

    expect(stdout).toBe("");
    expect(stripAnsi(stderr)).toBe('error: Unknown REPL mode "ruby". Expected one of: js, python, shell, sql, c.\n');
    expect(exitCode).toBe(1);
  });

  test("repl rejects arguments after the mode", async () => {
    const { stdout, stderr, exitCode } = await runRepl("", { args: ["python", "extra"] });

    expect(stdout).toBe("");
    expect(stripAnsi(stderr)).toBe('error: Unexpected argument "extra" after REPL mode.\n');
    expect(exitCode).toBe(1);
  });

  test.each(["--eval", "--print"])("repl rejects %s outside JavaScript mode", async flag => {
    const { stdout, stderr, exitCode } = await runRepl("", { args: ["python", flag, "1 + 1"] });

    expect(stdout).toBe("");
    expect(stripAnsi(stderr)).toBe("error: --eval and --print are only supported in JavaScript REPL mode.\n");
    expect(exitCode).toBe(1);
  });

  test("merges legacy per-language history into one tagged history file", async () => {
    using dir = tempDir("repl-merged-history", {
      ".bun_repl_history": "40 + 2\n",
      ".bun_repl_history.py": "py_value = 42\n",
      ".bun_repl_history.sh": "echo shell-history\n",
      ".bun_repl_history.sql": "SELECT 42;\n",
      ".bun_repl_history.c": "sizeof(int)\n",
    });
    const home = String(dir);
    const homeEnv = { HOME: home, USERPROFILE: home };

    const first = await runRepl([".history", ".exit"], {
      args: ["python"],
      env: homeEnv,
    });
    const output = stripAnsi(first.stdout);
    expect(output).toContain(".js 40 + 2");
    expect(output).toContain(".py py_value = 42");
    expect(output).toContain(".sh echo shell-history");
    expect(output).toContain(".sql SELECT 42;");
    expect(output).toContain(".c sizeof(int)");
    expect(first.stderr).toBe("");
    expect(first.exitCode).toBe(0);

    const historyPath = path.join(home, ".bun_repl_history");
    const expected = [
      ".js 40 + 2",
      ".py py_value = 42",
      ".sh echo shell-history",
      ".sql SELECT 42;",
      ".c sizeof(int)",
      "",
    ].join("\n");
    expect(await Bun.file(historyPath).text()).toBe(expected);

    const second = await runRepl(".exit\n", { env: homeEnv });
    expect(second.stderr).toBe("");
    expect(second.exitCode).toBe(0);
    expect(await Bun.file(historyPath).text()).toBe(expected);
  });

  test("persists entries from different modes in one tagged history file", async () => {
    using dir = tempDir("repl-unified-history", {});
    const home = String(dir);
    const historyPath = path.join(home, ".bun_repl_history");

    const result = await runRepl(["40 + 2", ".py", "py_value = 42", "def answer():", "    return 42", "", ".exit"], {
      env: { HOME: home, USERPROFILE: home, NODE_REPL_HISTORY: historyPath },
    });
    expect(result.stderr).toBe("");
    expect(result.exitCode).toBe(0);
    expect(await Bun.file(historyPath).text()).toBe(
      ".js 40 + 2\n.py py_value = 42\n.py def answer():\n.py|     return 42\n",
    );
  }, 10_000);

  test("honors NODE_REPL_HISTORY and NODE_REPL_HISTORY_SIZE", async () => {
    using dir = tempDir("repl-configured-history", {});
    const historyPath = path.join(String(dir), "custom.history");
    const result = await runRepl(["1", "2", "3", ".exit"], {
      env: {
        HOME: String(dir),
        NODE_REPL_HISTORY: historyPath,
        NODE_REPL_HISTORY_SIZE: "2",
      },
    });

    expect(result.stderr).toBe("");
    expect(result.exitCode).toBe(0);
    expect(await Bun.file(historyPath).text()).toBe(".js 2\n.js 3\n");
    expect(await Bun.file(path.join(String(dir), ".bun_repl_history")).exists()).toBe(false);
  });

  test("an empty NODE_REPL_HISTORY disables persistence", async () => {
    using dir = tempDir("repl-disabled-history", {});
    const result = await runRepl(["40 + 2", ".exit"], {
      env: { HOME: String(dir), NODE_REPL_HISTORY: "" },
    });

    expect(result.exitCode).toBe(0);
    expect(await Bun.file(path.join(String(dir), ".bun_repl_history")).exists()).toBe(false);
  });

  describe("basic evaluation", () => {
    test("evaluates simple expression", async () => {
      const { stdout, exitCode } = await runRepl(["1 + 1", ".exit"]);
      expect(stripAnsi(stdout)).toContain("2");
      expect(exitCode).toBe(0);
    });

    test("evaluates multiple expressions", async () => {
      const { stdout, exitCode } = await runRepl(["1 + 1", "2 * 3", "Math.sqrt(16)", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("2");
      expect(output).toContain("6");
      expect(output).toContain("4");
      expect(exitCode).toBe(0);
    });

    test("evaluates string expressions", async () => {
      const { stdout, exitCode } = await runRepl(["'hello'.toUpperCase()", ".exit"]);
      expect(stripAnsi(stdout)).toContain("HELLO");
      expect(exitCode).toBe(0);
    });

    test("evaluates object literals", async () => {
      const { stdout, exitCode } = await runRepl(["({ a: 1, b: 2 })", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("a");
      expect(output).toContain("b");
      expect(exitCode).toBe(0);
    });

    test("evaluates array expressions", async () => {
      const { stdout, exitCode } = await runRepl(["[1, 2, 3].map(x => x * 2)", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("2");
      expect(output).toContain("4");
      expect(output).toContain("6");
      expect(exitCode).toBe(0);
    });
  });

  describe("special variables", () => {
    test("_ contains last result", async () => {
      const { stdout, exitCode } = await runRepl(["42", "_", ".exit"]);
      const output = stripAnsi(stdout);
      // 42 should appear at least twice: once for the eval, once for _
      expect(output.split("42").length - 1).toBeGreaterThanOrEqual(2);
      expect(exitCode).toBe(0);
    });

    test("_ updates with each result", async () => {
      const { stdout, exitCode } = await runRepl(["10", "_ * 2", "_ + 5", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("10");
      expect(output).toContain("20");
      expect(output).toContain("25");
      expect(exitCode).toBe(0);
    });

    test("_error contains last error", async () => {
      const { stdout, exitCode } = await runRepl(["throw new Error('test error')", "_error.message", ".exit"]);
      expect(stripAnsi(stdout)).toContain("test error");
      expect(exitCode).toBe(0);
    });

    test("assigning _ disables automatic last-result updates", async () => {
      const { stdout, exitCode } = await runRepl(["_ = 7", "40 + 2", "_", ".exit"]);
      const output = stripAnsi(stdout);

      expect(output).toContain("Expression assignment to _ now disabled.");
      expect(output.match(/Expression assignment to _ now disabled\./g)).toHaveLength(1);
      expect(output).toMatch(/> _\r+\n7\n/);
      expect(exitCode).toBe(0);
    });

    test("assigning _error preserves the user value across later errors", async () => {
      const { stdout, stderr, exitCode } = await runRepl([
        "_error = new Error('mine')",
        "throw new Error('other')",
        "_error.message",
        ".exit",
      ]);
      const output = stripAnsi(stdout + stderr);

      expect(output).toContain("Expression assignment to _error now disabled.");
      expect(output).toContain("other");
      expect(output).toContain("mine");
      expect(exitCode).toBe(0);
    });

    test("regular expressions containing assignments do not disable special variables", async () => {
      const { stdout, exitCode } = await runRepl([
        "1",
        "/_=/.test('_=') && /_error\\s*=/.test('_error =')",
        "2",
        "_",
        ".exit",
      ]);
      const output = stripAnsi(stdout);

      expect(output).not.toContain("Expression assignment to _ now disabled.");
      expect(output).not.toContain("Expression assignment to _error now disabled.");
      expect(output).toMatch(/> _\r+\n2\n/);
      expect(exitCode).toBe(0);
    });

    test("assignments inside template substitutions disable automatic last-result updates", async () => {
      const { stdout, exitCode } = await runRepl(["`${(_ = 9)}`", "40 + 2", "_", ".exit"]);
      const output = stripAnsi(stdout);

      expect(output).toContain("Expression assignment to _ now disabled.");
      expect(output).toMatch(/> _\r+\n9\n/);
      expect(exitCode).toBe(0);
    });
  });

  describe("strict mode", () => {
    test("NODE_REPL_MODE=strict rejects implicit globals", async () => {
      const { stdout, stderr, exitCode } = await runRepl(["strictLeak = 42", ".exit"], {
        env: { NODE_REPL_MODE: "strict" },
      });

      expect(stripAnsi(stdout + stderr)).toMatch(/strictLeak is not defined|ReferenceError/);
      expect(exitCode).toBe(0);
    });

    test("NODE_REPL_MODE=sloppy keeps implicit globals", async () => {
      const { stdout, stderr, exitCode } = await runRepl(["sloppyLeak = 42", "sloppyLeak", ".exit"], {
        env: { NODE_REPL_MODE: "sloppy" },
      });

      expect(stderr).toBe("");
      expect(stripAnsi(stdout)).toContain("42");
      expect(exitCode).toBe(0);
    });
  });

  describe("REPL commands", () => {
    test(".exit exits the REPL", async () => {
      const { exitCode } = await runRepl([".exit"]);
      expect(exitCode).toBe(0);
    });

    test(".help shows help message", async () => {
      const { stdout, exitCode } = await runRepl([".help", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain(".help");
      expect(output).toContain(".exit");
      expect(output).toContain(".load");
      expect(output).toContain(".save");
      expect(exitCode).toBe(0);
    });

    test(".load loads and evaluates a file", async () => {
      using dir = tempDir("repl-load-test", {
        "test.js": "var loadedVar = 42;\n",
      });
      const filePath = path.join(String(dir), "test.js");
      const { stdout, exitCode } = await runRepl([`.load ${filePath}`, "loadedVar", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("42");
      expect(exitCode).toBe(0);
    });

    test(".load with nonexistent file shows error", async () => {
      // Use a relative path so Windows doesn't choke on forward-slash absolute paths (EINVAL).
      const { stdout, stderr, exitCode } = await runRepl([
        ".load definitely-does-not-exist-repl-test.js",
        "1 + 1",
        ".exit",
      ]);
      const allOutput = stripAnsi(stdout + stderr);
      expect(allOutput.toLowerCase()).toMatch(/error|not found|no such file|enoent|invalid argument/i);
      // REPL should continue after failed load
      expect(allOutput).toContain("2");
      expect(exitCode).toBe(0);
    });

    test(".load without filename shows usage", async () => {
      const { stdout, exitCode } = await runRepl([".load", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output.toLowerCase()).toMatch(/usage|filename/i);
      expect(exitCode).toBe(0);
    });

    test(".save saves history to file", async () => {
      using dir = tempDir("repl-save-test", {});
      const filePath = path.join(String(dir), "saved.js");
      const { exitCode } = await runRepl(["const x = 1", "const y = 2", `.save ${filePath}`, ".exit"]);
      expect(exitCode).toBe(0);
      const content = await Bun.file(filePath).text();
      expect(content).toContain("const x = 1");
      expect(content).toContain("const y = 2");
    });

    test(".save without filename shows usage", async () => {
      const { stdout, exitCode } = await runRepl([".save", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output.toLowerCase()).toMatch(/usage|filename/i);
      expect(exitCode).toBe(0);
    });

    test("unknown command shows error", async () => {
      const { stdout, exitCode } = await runRepl([".nonexistent", "1 + 1", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output.toLowerCase()).toContain("unknown");
      // REPL should continue
      expect(output).toContain("2");
      expect(exitCode).toBe(0);
    });

    test(".history shows command history", async () => {
      const { stdout, exitCode } = await runRepl(["1 + 1", "2 + 2", ".history", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("1 + 1");
      expect(output).toContain("2 + 2");
      expect(exitCode).toBe(0);
    });

    test(".break cancels multiline input", async () => {
      const { stdout, exitCode } = await runRepl([
        "function foo() {", // opens multiline
        ".break", // cancels it
        "1 + 1", // should eval normally
        ".exit",
      ]);
      const output = stripAnsi(stdout);
      expect(output).toContain("2");
      // foo should NOT be defined since we broke out
      expect(exitCode).toBe(0);
    });

    test(".break on empty multiline recovers prompt", async () => {
      const { stdout, exitCode } = await runRepl(["{", ".break", "99", ".exit"]);
      expect(stripAnsi(stdout)).toContain("99");
      expect(exitCode).toBe(0);
    });

    test(".clear cancels pending input without clearing the terminal", async () => {
      const { stdout, exitCode } = await runRepl(["{", ".clear", "40 + 2", ".exit"]);

      expect(stripAnsi(stdout)).toContain("42");
      expect(stdout).not.toContain("\x1b[2J");
      expect(exitCode).toBe(0);
    });

    test("command prefix matching (.e -> .exit)", async () => {
      // ReplCommand.find allows prefix matching when name.len > 1
      const { exitCode } = await runRepl([".ex"]);
      expect(exitCode).toBe(0);
    });
  });

  describe(".copy command", () => {
    test(".copy with no args copies last result", async () => {
      const { stdout, exitCode } = await runRepl(["42", ".copy", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("Copied");
      expect(output).toContain("clipboard");
      expect(exitCode).toBe(0);
    });

    test(".copy with expression evaluates and copies", async () => {
      const { stdout, exitCode } = await runRepl([".copy 1 + 1", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("Copied");
      expect(output).toContain("clipboard");
      expect(exitCode).toBe(0);
    });

    test(".copy still sets _ variable", async () => {
      const { stdout, exitCode } = await runRepl([".copy 'hello'", "_", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("hello");
      expect(exitCode).toBe(0);
    });
  });

  describe("error handling", () => {
    test("handles syntax errors gracefully", async () => {
      const { stdout, stderr, exitCode } = await runRepl(["(1 + ))", "1 + 1", ".exit"]);
      const allOutput = stripAnsi(stdout + stderr);
      expect(allOutput.toLowerCase()).toContain("error");
      // REPL should continue working after syntax error
      expect(allOutput).toContain("2");
      expect(exitCode).toBe(0);
    });

    test("handles runtime errors gracefully", async () => {
      const { stdout, stderr, exitCode } = await runRepl(["undefinedVariable", "1 + 1", ".exit"]);
      const allOutput = stripAnsi(stdout + stderr);
      expect(allOutput).toMatch(/not defined|ReferenceError/);
      expect(exitCode).toBe(0);
    });

    test("handles thrown string errors", async () => {
      const { stdout, stderr, exitCode } = await runRepl(["throw 'custom error'", "1 + 1", ".exit"]);
      const allOutput = stripAnsi(stdout + stderr);
      expect(allOutput).toContain("custom error");
      // REPL should continue after thrown error
      expect(allOutput).toContain("2");
      expect(exitCode).toBe(0);
    });

    test("handles thrown Error objects", async () => {
      const { stdout, stderr, exitCode } = await runRepl(["throw new Error('boom')", ".exit"]);
      const allOutput = stripAnsi(stdout + stderr);
      expect(allOutput).toContain("boom");
      expect(exitCode).toBe(0);
    });

    test("shows system error properties", async () => {
      const { stdout, stderr, exitCode } = await runRepl(["fs.readFileSync('/nonexistent/path/file.txt')", ".exit"]);
      const allOutput = stripAnsi(stdout + stderr);
      expect(allOutput).toMatch(/ENOENT|no such file/);
      expect(exitCode).toBe(0);
    });

    test("throwing custom inspect doesn't crash the loop", async () => {
      // format2 catches custom-inspect throws internally, but we verify no exception
      // leaks (BUN_JSC_validateExceptionChecks in CI) and the loop continues.
      const { stdout, stderr, exitCode } = await runRepl([
        `globalThis.__bad = { [Symbol.for("nodejs.util.inspect.custom")]() { throw new Error("boom"); } }; __bad`,
        "7 * 6",
        ".exit",
      ]);
      const allOutput = stripAnsi(stdout + stderr);
      // REPL must keep working after the inspection failure.
      // Use a product that won't appear in echoed input (7*6=42).
      expect(allOutput).toContain("42");
      expect(exitCode).toBe(0);
    });

    test("throwing Proxy ownKeys trap doesn't crash the loop", async () => {
      const { stdout, stderr, exitCode } = await runRepl([
        `new Proxy({}, { ownKeys() { throw new Error("boom"); } })`,
        "100 + 23",
        ".exit",
      ]);
      const allOutput = stripAnsi(stdout + stderr);
      expect(allOutput).toContain("123");
      expect(exitCode).toBe(0);
    });
  });

  describe("import statements", () => {
    test("import default from builtin module", async () => {
      const { stdout, exitCode } = await runRepl(["import path from 'path'", "typeof path.join", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("function");
      expect(exitCode).toBe(0);
    });

    test("import named exports from builtin module", async () => {
      const { stdout, exitCode } = await runRepl(["import { join, resolve } from 'path'", "typeof join", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("function");
      expect(exitCode).toBe(0);
    });

    test("import namespace from builtin module", async () => {
      const { stdout, exitCode } = await runRepl(["import * as os from 'os'", "typeof os.cpus", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("function");
      expect(exitCode).toBe(0);
    });

    test("import used across lines", async () => {
      // Use path.posix.join so the output is identical on Windows (otherwise: "\\tmp\\test").
      const { stdout, exitCode } = await runRepl([
        "import path from 'path'",
        "path.posix.join('/tmp', 'test')",
        ".exit",
      ]);
      const output = stripAnsi(stdout);
      expect(output).toContain("/tmp/test");
      expect(exitCode).toBe(0);
    });

    test("import nonexistent module shows error", async () => {
      const { stdout, stderr, exitCode } = await runRepl(["import _ from 'nonexistent-module-xyz'", "1 + 1", ".exit"]);
      const allOutput = stripAnsi(stdout + stderr);
      // Should show an error about the module not being found
      expect(allOutput.toLowerCase()).toMatch(/error|not found|cannot find|resolve/);
      // REPL should continue after failed import
      expect(allOutput).toContain("2");
      expect(exitCode).toBe(0);
    });

    test("import default and named together", async () => {
      // Combined form: import X, { a, b } from 'mod'
      // Verifies both default and named bindings are set correctly.
      using dir = tempDir("repl-import-combined", {
        "mod.mjs": `
          export const named1 = "first";
          export const named2 = "second";
          export default "the-default";
        `,
      });
      const filePath = Bun.pathToFileURL(path.join(String(dir), "mod.mjs")).href;
      const { stdout, stderr, exitCode } = await runRepl([
        `import def, { named1, named2 } from ${JSON.stringify(filePath)}`,
        "JSON.stringify([def, named1, named2])",
        ".exit",
      ]);
      const output = stripAnsi(stdout + stderr);
      expect(output).toContain(`the-default`);
      expect(output).toContain(`first`);
      expect(output).toContain(`second`);
      // Ensure the array is fully populated (no undefineds).
      expect(output).not.toMatch(/\bnull\b|\bundefined\b/);
      expect(exitCode).toBe(0);
    });

    test("import with alias uses source export name", async () => {
      // Regression: `import { foo as bar }` was reading __ns.bar instead of __ns.foo.
      using dir = tempDir("repl-import-alias", {
        "mod.mjs": `export const foo = "correct"; export const bar = "wrong";`,
      });
      const filePath = Bun.pathToFileURL(path.join(String(dir), "mod.mjs")).href;
      const { stdout, exitCode } = await runRepl([
        `import { foo as bar } from ${JSON.stringify(filePath)}`,
        "bar",
        ".exit",
      ]);
      const output = stripAnsi(stdout);
      expect(output).toContain("correct");
      expect(output).not.toContain("wrong");
      expect(exitCode).toBe(0);
    });

    test("import with multiple aliases", async () => {
      using dir = tempDir("repl-import-multi-alias", {
        "mod.mjs": `export const a = 1; export const b = 2; export const c = 3;`,
      });
      const filePath = Bun.pathToFileURL(path.join(String(dir), "mod.mjs")).href;
      const { stdout, exitCode } = await runRepl([
        `import { a as x, b as y, c } from ${JSON.stringify(filePath)}`,
        "JSON.stringify([x, y, c])",
        ".exit",
      ]);
      const output = stripAnsi(stdout);
      expect(output).toContain("[1,2,3]");
      expect(exitCode).toBe(0);
    });

    test("side-effect-only import", async () => {
      using dir = tempDir("repl-import-side-effect", {
        "side.mjs": `globalThis.__sideEffectRan = true;`,
      });
      const filePath = Bun.pathToFileURL(path.join(String(dir), "side.mjs")).href;
      const { stdout, exitCode } = await runRepl([
        `import ${JSON.stringify(filePath)}`,
        "globalThis.__sideEffectRan",
        ".exit",
      ]);
      expect(stripAnsi(stdout)).toContain("true");
      expect(exitCode).toBe(0);
    });
  });

  describe("require", () => {
    test("require is defined", async () => {
      const { stdout, exitCode } = await runRepl(["typeof require", ".exit"]);
      expect(stripAnsi(stdout)).toContain("function");
      expect(exitCode).toBe(0);
    });

    test("require builtin module", async () => {
      const { stdout, exitCode } = await runRepl(["const path = require('path')", "typeof path.join", ".exit"]);
      expect(stripAnsi(stdout)).toContain("function");
      expect(exitCode).toBe(0);
    });

    test("require.resolve works", async () => {
      const { stdout, exitCode } = await runRepl(["typeof require.resolve", ".exit"]);
      expect(stripAnsi(stdout)).toContain("function");
      expect(exitCode).toBe(0);
    });

    test("require resolves local files relative to cwd", async () => {
      // Verifies module.filename is set correctly so require("./x") resolves.
      using dir = tempDir("repl-require-local", {
        "local.js": `module.exports = { value: "from-local-file" };`,
      });
      await using proc = Bun.spawn({
        cmd: [bunExe(), "repl"],
        stdin: Buffer.from(`require("./local").value\n.exit\n`),
        stdout: "pipe",
        stderr: "pipe",
        cwd: String(dir),
        env: { ...bunEnv, TERM: "dumb", NO_COLOR: "1" },
      });
      const [stdout, exitCode] = await Promise.all([proc.stdout.text(), proc.exited]);
      expect(stripAnsi(stdout)).toContain("from-local-file");
      expect(exitCode).toBe(0);
    });

    test("module.filename has correct path separator", async () => {
      // Regression: was producing `/cwd[repl]` instead of `/cwd/[repl]`.
      const { stdout, exitCode } = await runRepl(["module.filename", ".exit"]);
      const output = stripAnsi(stdout);
      // Must contain a separator before [repl] — matches / or \ followed by [repl]
      expect(output).toMatch(/[\/\\]\[repl\]/);
      // Must NOT have the cwd smashed against [repl] without a separator
      expect(output).not.toMatch(/[a-zA-Z0-9]\[repl\]/);
      expect(exitCode).toBe(0);
    });
  });

  describe("global objects", () => {
    test("has access to Bun globals", async () => {
      const { stdout, exitCode } = await runRepl(["typeof Bun.version", ".exit"]);
      expect(stripAnsi(stdout)).toContain("string");
      expect(exitCode).toBe(0);
    });

    test("has access to console", async () => {
      const { stdout, exitCode } = await runRepl(["console.log('hello from repl')", ".exit"]);
      expect(stripAnsi(stdout)).toContain("hello from repl");
      expect(exitCode).toBe(0);
    });

    test("has access to Buffer", async () => {
      const { stdout, exitCode } = await runRepl(["Buffer.from('hello').length", ".exit"]);
      expect(stripAnsi(stdout)).toContain("5");
      expect(exitCode).toBe(0);
    });

    test("has access to process", async () => {
      const { stdout, exitCode } = await runRepl(["typeof process.version", ".exit"]);
      expect(stripAnsi(stdout)).toContain("string");
      expect(exitCode).toBe(0);
    });

    test("has __dirname and __filename", async () => {
      const { stdout, exitCode } = await runRepl(["typeof __dirname", "typeof __filename", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("string");
      expect(exitCode).toBe(0);
    });

    test("has module object", async () => {
      const { stdout, exitCode } = await runRepl(["typeof module", ".exit"]);
      expect(stripAnsi(stdout)).toContain("object");
      expect(exitCode).toBe(0);
    });
  });

  describe("variable persistence", () => {
    test("variables persist across evaluations", async () => {
      const { stdout, exitCode } = await runRepl(["const x = 10", "const y = 20", "x + y", ".exit"]);
      expect(stripAnsi(stdout)).toContain("30");
      expect(exitCode).toBe(0);
    });

    test("let variables can be reassigned", async () => {
      const { stdout, exitCode } = await runRepl(["let counter = 0", "counter++", "counter++", "counter", ".exit"]);
      expect(stripAnsi(stdout)).toContain("2");
      expect(exitCode).toBe(0);
    });

    test("functions persist", async () => {
      const { stdout, exitCode } = await runRepl(["function add(a, b) { return a + b; }", "add(5, 3)", ".exit"]);
      expect(stripAnsi(stdout)).toContain("8");
      expect(exitCode).toBe(0);
    });

    test("classes persist across evaluations", async () => {
      const { stdout, exitCode } = await runRepl([
        "class Point { constructor(x, y) { this.x = x; this.y = y; } sum() { return this.x + this.y; } }",
        "new Point(3, 4).sum()",
        ".exit",
      ]);
      expect(stripAnsi(stdout)).toContain("7");
      expect(exitCode).toBe(0);
    });

    test("const can be redeclared across lines", async () => {
      // REPL hoists const -> var so redeclaration works like Node's REPL.
      const { stdout, stderr, exitCode } = await runRepl(["const x = 1", "const x = 2", "x", ".exit"]);
      const output = stripAnsi(stdout + stderr);
      expect(output).not.toMatch(/already.*declared|redeclar/i);
      expect(output).toContain("2");
      expect(exitCode).toBe(0);
    });

    test("array destructuring persists", async () => {
      const { stdout, exitCode } = await runRepl(["const [a, b, c] = [10, 20, 30]", "a + b + c", ".exit"]);
      expect(stripAnsi(stdout)).toContain("60");
      expect(exitCode).toBe(0);
    });

    test("object destructuring persists", async () => {
      const { stdout, exitCode } = await runRepl(["const { px, py } = { px: 5, py: 7 }", "px * py", ".exit"]);
      expect(stripAnsi(stdout)).toContain("35");
      expect(exitCode).toBe(0);
    });

    test("object destructuring with rename persists", async () => {
      const { stdout, exitCode } = await runRepl(["const { a: renamed } = { a: 99 }", "renamed", ".exit"]);
      expect(stripAnsi(stdout)).toContain("99");
      expect(exitCode).toBe(0);
    });

    test("destructuring with defaults persists", async () => {
      const { stdout, exitCode } = await runRepl(["const { missing = 42 } = {}", "missing", ".exit"]);
      expect(stripAnsi(stdout)).toContain("42");
      expect(exitCode).toBe(0);
    });

    test("array rest destructuring persists", async () => {
      const { stdout, exitCode } = await runRepl(["const [first, ...rest] = [1, 2, 3, 4]", "rest", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("2");
      expect(output).toContain("3");
      expect(output).toContain("4");
      expect(exitCode).toBe(0);
    });

    test("object rest destructuring persists", async () => {
      const { stdout, exitCode } = await runRepl([
        "const { keep, ...others } = { keep: 1, x: 2, y: 3 }",
        "Object.keys(others).sort().join(',')",
        ".exit",
      ]);
      expect(stripAnsi(stdout)).toContain("x,y");
      expect(exitCode).toBe(0);
    });

    test("nested destructuring persists", async () => {
      const { stdout, exitCode } = await runRepl([
        "const { outer: { inner } } = { outer: { inner: 'deep' } }",
        "inner",
        ".exit",
      ]);
      expect(stripAnsi(stdout)).toContain("deep");
      expect(exitCode).toBe(0);
    });
  });

  describe("multiline input", () => {
    test("handles multiline function definition", async () => {
      const { stdout, exitCode } = await runRepl([
        "function greet(name) {",
        "  return 'hi ' + name",
        "}",
        "greet('world')",
        ".exit",
      ]);
      expect(stripAnsi(stdout)).toContain("hi world");
      expect(exitCode).toBe(0);
    });

    test("handles multiline object", async () => {
      const { stdout, exitCode } = await runRepl(["({", "  x: 1,", "  y: 2", "})", ".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("x");
      expect(output).toContain("y");
      expect(exitCode).toBe(0);
    });

    test("lone string directive returns the string", async () => {
      // REPL treats directives (string literal statements) as expressions.
      const { stdout, exitCode } = await runRepl([`"use strict"`, ".exit"]);
      expect(stripAnsi(stdout)).toContain("use strict");
      expect(exitCode).toBe(0);
    });
  });

  describe("async evaluation", () => {
    test("await expressions", async () => {
      const { stdout, exitCode } = await runRepl(["await Promise.resolve(42)", ".exit"]);
      expect(stripAnsi(stdout)).toContain("42");
      expect(exitCode).toBe(0);
    });

    test("await rejected promise shows error", async () => {
      const { stdout, stderr, exitCode } = await runRepl([
        "await Promise.reject(new Error('async fail'))",
        "1 + 1",
        ".exit",
      ]);
      const allOutput = stripAnsi(stdout + stderr);
      expect(allOutput).toContain("async fail");
      // REPL should continue after rejected promise
      expect(allOutput).toContain("2");
      expect(exitCode).toBe(0);
    });

    test("async functions", async () => {
      const { stdout, exitCode } = await runRepl([
        "async function getValue() { return 123; }",
        "await getValue()",
        ".exit",
      ]);
      expect(stripAnsi(stdout)).toContain("123");
      expect(exitCode).toBe(0);
    });
  });

  describe("TypeScript support", () => {
    test("type annotations are stripped", async () => {
      const { stdout, exitCode } = await runRepl(["const x: number = 42", "x", ".exit"]);
      expect(stripAnsi(stdout)).toContain("42");
      expect(exitCode).toBe(0);
    });

    test("interface declarations work", async () => {
      const { stdout, exitCode } = await runRepl([
        "interface User { name: string }",
        "const u: User = { name: 'test' }",
        "u.name",
        ".exit",
      ]);
      expect(stripAnsi(stdout)).toContain("test");
      expect(exitCode).toBe(0);
    });
  });

  describe("welcome message", () => {
    test("shows welcome message with version", async () => {
      const { stdout, exitCode } = await runRepl([".exit"]);
      const output = stripAnsi(stdout);
      expect(output).toContain("Welcome to Poly");
      expect(output).toMatch(/Poly v\d+\.\d+\.\d+/);
      expect(exitCode).toBe(0);
    });
  });

  describe("-e / --eval and -p / --print", () => {
    async function runReplWith(args: string[]) {
      await using proc = Bun.spawn({
        cmd: [bunExe(), "repl", ...args],
        stdout: "pipe",
        stderr: "pipe",
        env: { ...bunEnv, NO_COLOR: "1" },
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      return { stdout: stripAnsi(stdout), stderr: stripAnsi(stderr), exitCode };
    }

    test("-e evaluates and exits without printing result", async () => {
      const { stdout, exitCode } = await runReplWith(["-e", "1 + 1"]);
      expect(stdout).toBe("");
      expect(exitCode).toBe(0);
    });

    test("-e does not show welcome message", async () => {
      const { stdout, exitCode } = await runReplWith(["-e", "1 + 1"]);
      expect(stdout).not.toContain("Welcome to Poly");
      expect(exitCode).toBe(0);
    });

    test("-e console.log output is shown", async () => {
      const { stdout, exitCode } = await runReplWith(["-e", "console.log('hello from eval')"]);
      expect(stdout).toBe("hello from eval\n");
      expect(exitCode).toBe(0);
    });

    test("--eval works like -e", async () => {
      const { stdout, exitCode } = await runReplWith(["--eval", "console.log(2 + 2)"]);
      expect(stdout).toBe("4\n");
      expect(exitCode).toBe(0);
    });

    test("-p prints the result and exits", async () => {
      const { stdout, exitCode } = await runReplWith(["-p", "1 + 1"]);
      expect(stdout).toBe("2\n");
      expect(exitCode).toBe(0);
    });

    test("--print works like -p", async () => {
      const { stdout, exitCode } = await runReplWith(["--print", "2 * 3"]);
      expect(stdout).toBe("6\n");
      expect(exitCode).toBe(0);
    });

    test("-p prints undefined for void expressions", async () => {
      const { stdout, exitCode } = await runReplWith(["-p", "void 0"]);
      expect(stdout).toBe("undefined\n");
      expect(exitCode).toBe(0);
    });

    test("-p with empty script prints undefined and exits", async () => {
      const { stdout, exitCode } = await runReplWith(["-p", ""]);
      expect(stdout).toBe("undefined\n");
      expect(exitCode).toBe(0);
    });

    test("-e supports TypeScript", async () => {
      const { stdout, exitCode } = await runReplWith(["-p", "const x: number = 42; x * 2"]);
      expect(stdout).toBe("84\n");
      expect(exitCode).toBe(0);
    });

    test("-e supports top-level await", async () => {
      const { stdout, exitCode } = await runReplWith(["-p", "await Promise.resolve(123)"]);
      expect(stdout).toBe("123\n");
      expect(exitCode).toBe(0);
    });

    test("-p wraps object literals", async () => {
      const { stdout, exitCode } = await runReplWith(["-p", "{ a: 1, b: 2 }"]);
      expect(stdout).toContain("a");
      expect(stdout).toContain("1");
      expect(stdout).toContain("b");
      expect(stdout).toContain("2");
      expect(exitCode).toBe(0);
    });

    test("-e with thrown error writes to stderr and exits with code 1", async () => {
      const { stdout, stderr, exitCode } = await runReplWith(["-e", "throw new Error('boom')"]);
      expect(stdout).toBe("");
      expect(stderr).toContain("boom");
      expect(exitCode).toBe(1);
    });

    test("-e with syntax error writes to stderr and exits with code 1", async () => {
      const { stdout, stderr, exitCode } = await runReplWith(["-e", "const ="]);
      expect(stdout).toBe("");
      expect(stderr.toLowerCase()).toContain("syntaxerror");
      expect(exitCode).toBe(1);
    });

    test("-e with rejected top-level await writes to stderr and exits with code 1", async () => {
      const { stdout, stderr, exitCode } = await runReplWith(["-e", "await Promise.reject(new Error('async fail'))"]);
      expect(stdout).toBe("");
      expect(stderr).toContain("async fail");
      expect(exitCode).toBe(1);
    });

    test("-e preserves process.exitCode set by the script", async () => {
      const { exitCode } = await runReplWith(["-e", "process.exitCode = 42"]);
      expect(exitCode).toBe(42);
    });

    test("-e fires process.on('beforeExit')", async () => {
      const { stdout, exitCode } = await runReplWith([
        "-e",
        "process.on('beforeExit', () => console.log('beforeExit fired'))",
      ]);
      expect(stdout).toBe("beforeExit fired\n");
      expect(exitCode).toBe(0);
    });

    test("-e drains event loop (timers fire before exit)", async () => {
      const { stdout, exitCode } = await runReplWith(["-e", "setTimeout(() => console.log('from timer'), 50)"]);
      expect(stdout).toBe("from timer\n");
      expect(exitCode).toBe(0);
    });

    test("-p drains event loop before printing", async () => {
      // Result should be printed after the timer output, since we drain
      // the event loop before printing the final result.
      const { stdout, exitCode } = await runReplWith(["-p", "setTimeout(() => console.log('timer'), 50); 'result'"]);
      expect(stdout).toBe('timer\n"result"\n');
      expect(exitCode).toBe(0);
    });

    test("-e supports require()", async () => {
      const { stdout, exitCode } = await runReplWith(["-p", "require('path').posix.join('/a', 'b')"]);
      expect(stdout).toBe('"/a/b"\n');
      expect(exitCode).toBe(0);
    });

    test("-e supports import statements", async () => {
      const { stdout, exitCode } = await runReplWith(["-e", "import path from 'path'; console.log(typeof path.join)"]);
      expect(stdout).toBe("function\n");
      expect(exitCode).toBe(0);
    });

    test("-e has access to __dirname and __filename", async () => {
      const { stdout, exitCode } = await runReplWith(["-e", "console.log(typeof __dirname, typeof __filename)"]);
      expect(stdout).toBe("string string\n");
      expect(exitCode).toBe(0);
    });

    // https://github.com/oven-sh/bun/issues/31225
    test("bare top-level `this` does not throw (issue #31225)", async () => {
      // Before the fix this threw `ReferenceError: exports is not defined`
      // because the parser rewrote top-level `this` to `exports`, and the REPL
      // IIFE has no `exports` binding.
      const { stdout, stderr, exitCode } = await runReplWith(["-e", "this"]);
      expect(stderr).toBe("");
      expect(stdout).toBe("");
      expect(exitCode).toBe(0);
    });

    test("top-level `this` evaluates to globalThis (issue #31225)", async () => {
      const { stdout, stderr, exitCode } = await runReplWith(["-e", "console.log(typeof this, this === globalThis)"]);
      expect(stderr).toBe("");
      expect(stdout).toBe("object true\n");
      expect(exitCode).toBe(0);
    });

    test("member access on top-level `this` hits the global (issue #31225)", async () => {
      // `Math` lives on the global, so `this.Math` should be the same object.
      const { stdout, stderr, exitCode } = await runReplWith(["-e", "console.log(this.Math === Math)"]);
      expect(stderr).toBe("");
      expect(stdout).toBe("true\n");
      expect(exitCode).toBe(0);
    });
  });
});

// Interactive terminal-based REPL tests
describe.todoIf(isWindows)("Bun REPL (Terminal)", () => {
  test("shows welcome message and prompt", async () => {
    await withTerminalRepl(async ({ allOutput }) => {
      const output = allOutput();
      expect(output).toContain("Welcome to Poly");
      expect(output).toMatch(/\u276f|> /);
    });
  });

  test.each([
    [
      "JavaScript",
      [],
      'const answer = 42; const text = "ok"; // note',
      "\x1b[35mconst\x1b[0m",
      "\x1b[33m42\x1b[0m",
      '\x1b[32m"ok"\x1b[0m',
      "\x1b[2m// note\x1b[0m",
    ],
    [
      "Python",
      ["python"],
      '@cache async def answer(value: int = 0x2a) -> str: return f"""{value}""" # note',
      "\x1b[36m@cache\x1b[0m",
      "\x1b[35masync\x1b[0m",
      '\x1b[32mf"""{value}"""\x1b[0m',
      "\x1b[2m# note\x1b[0m",
    ],
    [
      "Shell",
      ["shell"],
      'if test -n "${HOME}"; then echo "$(pwd)" 2>>log # note',
      "\x1b[35mif\x1b[0m",
      "\x1b[36m${HOME}\x1b[0m",
      "\x1b[33m2>>\x1b[0m",
      "\x1b[2m# note\x1b[0m",
    ],
    [
      "SQL",
      ["sql"],
      'select count(*) from "users" where id = :id -- note',
      "\x1b[35mselect\x1b[0m",
      "\x1b[34mcount\x1b[0m",
      "\x1b[36m:id\x1b[0m",
      "\x1b[2m-- note\x1b[0m",
    ],
    [
      "C",
      ["c"],
      '#define VALUE ((const char *)u8"ok") // note',
      "\x1b[35m#define\x1b[0m",
      "\x1b[34mchar\x1b[0m",
      '\x1b[32mu8"ok"\x1b[0m',
      "\x1b[2m// note\x1b[0m",
    ],
  ])(
    "%s mode applies language-specific syntax highlighting",
    async (_language, args, input, firstToken, secondToken, thirdToken, commentToken) => {
      using dir = tempDir("repl-syntax-highlighting", {});
      await withTerminalRepl(
        async ({ send, waitFor, rawOutput }) => {
          send(input);
          await waitFor("note");

          const output = rawOutput();
          expect(stripAnsi(output)).toContain(input);
          expect(output).toContain(firstToken);
          expect(output).toContain(secondToken);
          expect(output).toContain(thirdToken);
          expect(output).toContain(commentToken);

          send("\x03");
          await waitFor(/\u276f|> /);
        },
        { args, env: { HOME: String(dir), NO_COLOR: undefined } },
      );
    },
  );

  test("syntax highlighting remains enabled for input longer than 2048 bytes", async () => {
    const input = `return ${Buffer.alloc(2049, "x").toString()}`;
    expect(input.length).toBeGreaterThan(2048);
    using dir = tempDir("repl-long-syntax-highlighting", {
      ".bun_repl_history": `.py ${input}\n`,
    });

    await withTerminalRepl(
      async ({ send, waitFor }) => {
        send("\x1b[A");
        const output = await waitFor("\x1b[35mreturn\x1b[0m");
        expect(output).toContain("\x1b[35mreturn\x1b[0m");

        send("\x03");
        await waitFor(/\u276f|> /);
      },
      {
        args: ["python"],
        env: { HOME: String(dir), NO_COLOR: undefined },
      },
    );
  });

  test("evaluates expression and shows result", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send("40 + 2\n");
      const output = await waitFor("42");
      expect(stripAnsi(output)).toContain("42");
    });
  });

  test("error shows in terminal", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send("throw new Error('order test')\n");
      const output = await waitFor("order test");
      expect(stripAnsi(output)).toContain("order test");
    });
  });

  test("console.log shows in terminal", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send("console.log('side effect')\n");
      const output = await waitFor("side effect");
      expect(stripAnsi(output)).toContain("side effect");
    });
  });

  test("Ctrl+C cancels current input", async () => {
    await withTerminalRepl(async ({ send, waitFor, allOutput }) => {
      send("some partial input");
      await waitFor("some partial input");
      send("\x03"); // Ctrl+C
      await waitFor(/\u276f|> /);
      // Should be back at a clean prompt
      send("1 + 1\n");
      await waitFor("2");
    });
  });

  test("Ctrl+D exits on empty line", async () => {
    await withTerminalRepl(async ({ terminal, proc }) => {
      terminal.write("\x04"); // Ctrl+D
      const exitCode = await Promise.race([proc.exited, Bun.sleep(3000).then(() => -1)]);
      expect(exitCode).toBe(0);
    });
  });

  test("require works in terminal", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send("typeof require\n");
      const output = await waitFor("function");
      expect(stripAnsi(output)).toContain("function");
    });
  });

  test("import statement works in terminal", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send("import path from 'path'\n");
      // Wait for the import to complete
      await waitFor(/\u276f|> /);
      send("path.sep\n");
      await waitFor("/");
    });
  });

  test("up arrow recalls previous command", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send("111 + 222\n");
      await waitFor("333");
      // Press up arrow to recall previous command
      send("\x1b[A"); // Up arrow escape sequence
      await Bun.sleep(100);
      send("\n");
      // Should evaluate the same expression again
      await waitFor("333");
    });
  });

  test("up arrow renders another language's history as an inline command", async () => {
    using dir = tempDir("repl-cross-language-history", {
      ".bun_repl_history": ".js 40 + 2\n",
    });

    await withTerminalRepl(
      async ({ send, waitFor }) => {
        send("\x1b[A");
        const recalled = await waitFor(".js 40 + 2");
        expect(stripAnsi(recalled)).toContain(".js 40 + 2");

        send("\n");
        const evaluated = await waitFor(/\n\s*42\b/);
        expect(stripAnsi(evaluated)).toMatch(/\n\s*42\b/);
      },
      {
        args: ["python"],
        env: { HOME: String(dir) },
      },
    );
  });

  test("down arrow restores temp line after history", async () => {
    using dir = tempDir("repl-history-temp-line", {});
    await withTerminalRepl(
      async ({ send, waitFor }) => {
        send("partialValue = 777\n");
        await waitFor("777");

        send("partial");
        await waitFor("partial");
        send("\x1b[A");
        await waitFor("partialValue = 777");
        send("\x1b[B");
        await waitFor("partial");

        send("\x03");
        await waitFor(/\u276f|> /);
        send("1 + 1\n");
        await waitFor("2");
      },
      { env: { HOME: String(dir) } },
    );
  });

  test("Ctrl+Y yanks text deleted by Ctrl+U", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send("123 + 456");
      await waitFor("123 + 456");
      send("\x15");
      await waitFor(/\u276f|> /);
      send("\x19");
      await waitFor("123 + 456");
      send("\n");
      await waitFor(/\n\s*579\b/);
    });
  });

  test("up and down arrows search history by the current prefix", async () => {
    using dir = tempDir("repl-prefix-history", {
      ".bun_repl_history": '.js "alpha old"\n.js "beta"\n.js "alpha new"\n',
    });

    await withTerminalRepl(
      async ({ send, waitFor }) => {
        send('"alpha');
        await waitFor('"alpha');
        send("\x1b[A");
        await waitFor('"alpha new"');
        send("\x1b[A");
        await waitFor('"alpha old"');
        send("\x1b[B");
        await waitFor('"alpha new"');
        send("\x03");
        await waitFor(/\u276f|> /);
      },
      { env: { HOME: String(dir) } },
    );
  });

  test("Ctrl+R and Ctrl+S search backward and forward through history", async () => {
    using dir = tempDir("repl-reverse-history", {
      ".bun_repl_history": '.js "older needle"\n.js "newer needle"\n',
    });

    await withTerminalRepl(
      async ({ send, waitFor }) => {
        send("\x12");
        await waitFor("bck-i-search:");
        send("needle");
        await waitFor("newer needle");
        send("\x12");
        await waitFor("older needle");
        send("\x13");
        await waitFor("newer needle");
        send("\n");
        await waitFor(/\u276f|> /);
      },
      { env: { HOME: String(dir) } },
    );
  });

  test("recalls a persisted multiline entry as one editable command", async () => {
    using dir = tempDir("repl-multiline-history", {
      ".bun_repl_history": ".js let persisted = (\n.js| 41\n.js| )\n",
    });

    await withTerminalRepl(
      async ({ send, waitFor, allOutput }) => {
        send("\x1b[A");
        await waitFor("41");
        expect(allOutput()).toContain("... 41");

        send("\x1b[A");
        send("\x1b[3~");
        send("2");
        await waitFor("42");
        send("\n");
        await waitFor(/\u276f|> /);
        send("persisted\n");
        await waitFor(/\n\s*42\b/);
      },
      { env: { HOME: String(dir) } },
    );
  });

  test("keeps wrapped and exact-width input editable", async () => {
    await withTerminalRepl(
      async ({ send, waitFor }) => {
        send("1234567890");
        await waitFor("1234567890");
        send("\x7f");
        const exactWidthEdit = await waitFor("123456789");
        expect(exactWidthEdit).not.toContain("\x1b[1A");
        send("0\n");
        await waitFor(/\n\s*1234567890\b/);

        send("1000000000 + 1");
        await waitFor("1000000000 + 1");
        send("\x01");
        const movedAcrossRows = await waitFor("1000000000 + 1");
        expect(movedAcrossRows).toContain("\x1b[1A");
        send("\x1b[3~2\x05\n");
        await waitFor(/\n\s*2000000001\b/);
      },
      { cols: 12 },
    );
  });

  test("reflows the current input after a terminal resize", async () => {
    await withTerminalRepl(async ({ terminal, send, waitFor }) => {
      send('"123456789012345678901"');
      await waitFor('"123456789012345678901"');

      terminal.resize(12, 40);
      send("\x01");
      const resized = await waitFor(/\x1b\[2A/);
      expect(resized).toContain("\x1b[2A");
      send("\x05\n");
      await waitFor("123456789012345678901");
    });
  });

  test("tab completes REPL commands", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send(".he");
      await waitFor(".he");
      send("\t"); // Tab — should complete to .help (only match)
      await waitFor(".help");
    });
  });

  test("tab completes JavaScript properties and expands their common prefix", async () => {
    await withTerminalRepl(async ({ send, waitFor, allOutput }) => {
      send("const completionTarget = { alphaUnique: 1, alphabetSoup: 2 }\n");
      await waitFor(/\u276f|> /);

      send("completionTarget.alp\t");
      await waitFor("completionTarget.alpha");
      send("\t");
      await waitFor("alphaUnique");
      expect(allOutput()).toContain("alphabetSoup");

      send("Unique\n");
      await waitFor(/\n\s*1\b/);
    });
  });

  test("tab completion keeps common prefixes on UTF-8 boundaries", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send("const unicodeTarget = { 'a\\u00e9': 1, 'a\\u00ea': 2 }\n");
      await waitFor(/\u276f|> /);

      send("unicodeTarget.a\t");
      await waitFor("a\u00e9");
      send("\u00e9\n");
      await waitFor(/\n\s*1\b/);
    });
  });

  test("shows an inline completion preview and accepts it with Enter", async () => {
    await withTerminalRepl(
      async ({ send, waitFor }) => {
        send("const ghostTarget = { previewValue: 42 }; 6000 + 7\n");
        await waitFor("6007", 3000);
        await waitFor(/\u276f|> /, 3000);

        send("ghostTarget.previewV");
        await waitFor("\x1b[2malue\x1b[0m", 3000);
        send("\n");
        await waitFor("42", 3000);
      },
      { env: { NO_COLOR: undefined } },
    );
  }, 15_000);

  test("inline completion does not invoke getters or Proxy traps", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send(
        "let getterReads = 0, proxyReads = 0; const holder = {}; " +
          "Object.defineProperty(holder, 'target', { get() { getterReads++; return { previewValue: 1 } } }); " +
          "const proxyTarget = new Proxy({ previewValue: 1 }, { get(target, key, receiver) { proxyReads++; return Reflect.get(target, key, receiver) } });\n",
      );
      await waitFor(/\u276f|> /);

      send("holder.target.previewV");
      await waitFor("holder.target.previewV");
      send("\x03");
      await waitFor(/\u276f|> /);
      send("proxyTarget.previewV");
      await waitFor("proxyTarget.previewV");
      send("\x03");
      await waitFor(/\u276f|> /);
      send("getterReads + proxyReads\n");
      await waitFor(/\n\s*0\b/);
    });
  });

  test("shows side-effect-free result previews without updating the last result", async () => {
    await withTerminalRepl(
      async ({ send, waitFor }) => {
        send("10\n");
        await waitFor("10", 3000);
        await waitFor(/\u276f|> /, 3000);
        send("1 + 1");
        await waitFor(/\r\n\x1b\[2m2\x1b\[0m/, 3000);
        send("\x03");
        await waitFor(/\u276f|> /, 3000);
        send("_\n");
        await waitFor("10", 3000);
      },
      { env: { NO_COLOR: undefined } },
    );
  }, 15_000);

  test("tab completes attributes from the live Python object", async () => {
    await withTerminalRepl(
      async ({ send, waitFor }) => {
        send('import math; print("python-ready")\n');
        await waitFor("python-ready");
        await waitFor(">>> ");
        send("math.sq\t");
        await waitFor("math.sqrt");
        send("(81)\n");
        await waitFor(/\n\s*9(?:\.0)?\b/);
      },
      { args: ["python"], startupTimeoutMs: 10_000 },
    );
  }, 15_000);

  test(".editor mode collects lines until Ctrl+D", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send(".editor\n");
      await waitFor(/editor mode/i);
      send("let __editorResult = 100\n");
      send("__editorResult + 23\n");
      send("\x04"); // Ctrl+D to finish editor mode
      await waitFor("123");
    });
  });

  test(".editor is ignored while a multiline input is pending", async () => {
    await withTerminalRepl(async ({ send, waitFor, allOutput }) => {
      send("function __pending() {\n");
      await waitFor("...");
      send(".editor\n");
      await waitFor("...");
      // `.editor` must not switch modes here: the next line still feeds the
      // pending function body, which then completes and is callable.
      send("return 42 }\n");
      await waitFor(/\u276f|> /);
      send("__pending()\n");
      await waitFor("42");
      expect(allOutput()).not.toMatch(/editor mode/i);
    });
  });

  test("multiline input with open brace", async () => {
    await withTerminalRepl(async ({ send, waitFor }) => {
      send("function test() {\n");
      await waitFor("..."); // multiline prompt
      send("  return 99\n");
      send("}\n");
      // Wait for function to be defined
      await waitFor(/\u276f|> /);
      send("test()\n");
      await waitFor("99");
    });
  });

  // Regression for #31871: the line editor read input one byte at a time and
  // dropped every byte >= 0x80, so multi-byte UTF-8 (Korean, emoji, etc.) was
  // silently discarded and the evaluated expression differed from what was typed.
  test("keeps multi-byte UTF-8 characters typed into the line editor", async () => {
    await withTerminalRepl(async ({ send, waitFor, allOutput }) => {
      send('"a한b".length\n');
      // "a한b".length is 3; before the fix the 한 was dropped and it was 2.
      await waitFor(/\n\s*3\b/);
      // The echoed input line must still contain the Korean character.
      expect(allOutput()).toContain("한");
    });
  });

  test("keeps a full Korean string typed into the line editor", async () => {
    await withTerminalRepl(async ({ send, waitFor, allOutput }) => {
      send('"안녕하세요 영재님".length\n');
      // 9 characters; before the fix only the ASCII space survived -> 1.
      await waitFor(/\n\s*9\b/);
      expect(allOutput()).toContain("안녕하세요 영재님");
    });
  });

  test("backspace deletes a whole multi-byte character", async () => {
    await withTerminalRepl(async ({ send, waitFor, allOutput }) => {
      // Type a Korean char then backspace over it; byte-wise stepping would
      // leave a truncated UTF-8 sequence behind.
      send("한");
      await waitFor("한");
      send("\x7f"); // Backspace
      send('"ok".length\n');
      await waitFor(/\n\s*2\b/);
      // The dangling Korean char must be gone, leaving a clean expression.
      expect(allOutput()).toContain('"ok".length');
    });
  });

  test("left-arrow navigation steps over a whole multi-byte character", async () => {
    await withTerminalRepl(async ({ send, waitFor, allOutput }) => {
      // Type `"한"`, then left-arrow twice to land between the opening quote
      // and 한 (the second move must skip all 3 bytes of 한, not land mid-char),
      // and insert `b`. The buffer becomes `"b한"`, whose length is 2.
      send('"한"');
      await waitFor("한");
      send("\x1b[D"); // left: between 한 and the closing quote
      send("\x1b[D"); // left: between the opening quote and 한 (skips 3 bytes)
      send("b");
      send("\x05"); // Ctrl+E: move to end of line
      send(".length\n");
      await waitFor(/\n\s*2\b/);
      expect(allOutput()).toContain('"b한"');
    });
  });

  test("Ctrl+T transposes whole multi-byte characters", async () => {
    await withTerminalRepl(async ({ send, waitFor, allOutput }) => {
      // Type `"한b"`, move the cursor between 한 and b, then Ctrl+T. Byte-wise
      // transposition would split 한; it must swap the two whole codepoints so
      // the buffer becomes `"b한"` (length 2), not corrupted UTF-8.
      send('"한b"');
      await waitFor("한b");
      send("\x1b[D"); // left: between b and the closing quote
      send("\x1b[D"); // left: between 한 and b
      send("\x14"); // Ctrl+T: transpose 한 and b -> "b한"
      send("\x05"); // Ctrl+E: move to end of line
      send(".length\n");
      await waitFor(/\n\s*2\b/);
      expect(allOutput()).toContain('"b한"');
    });
  });

  test("drops a malformed UTF-8 sequence instead of corrupting the buffer", async () => {
    await withTerminalRepl(async ({ terminal, send, waitFor }) => {
      // ED A0 80 encodes the lone surrogate U+D800: it has a valid lead byte
      // and continuation-byte shape but is not valid UTF-8. It must be dropped,
      // leaving a clean "ab" (length 2), not fed into the buffer.
      send('"a');
      await waitFor("a");
      terminal.write(new Uint8Array([0xed, 0xa0, 0x80]));
      send('b".length\n');
      await waitFor(/\n\s*2\b/);
    });
  });

  test("a stray lead byte does not swallow the next keystroke", async () => {
    await withTerminalRepl(async ({ terminal, send, waitFor }) => {
      // 0xC2 is a 2-byte lead; the following byte (0x62 'b') is not a
      // continuation byte, so the lead is dropped. The 'b' must still be
      // processed, giving a clean "ab" (length 2), not "a" (length 1).
      send('"a');
      await waitFor("a");
      terminal.write(new Uint8Array([0xc2, 0x62])); // stray lead + 'b'
      send('".length\n');
      await waitFor(/\n\s*2\b/);
    });
  });
});

// History file written on REPL exit must be owner-only (0600), since it can
// contain pasted credentials. See src/runtime/cli/repl.rs History::save.
describe.skipIf(isWindows)("REPL history file permissions", () => {
  test("persists history readable only by the owner", async () => {
    using dir = tempDir("repl-history-perms", {});
    const home = String(dir);

    await using proc = Bun.spawn({
      cmd: [bunExe(), "repl"],
      stdin: Buffer.from(['const dbUrl = "postgres://user:hunter2@db.internal/prod"', ".exit", ""].join("\n")),
      stdout: "pipe",
      stderr: "pipe",
      env: {
        ...bunEnv,
        TERM: "dumb",
        NO_COLOR: "1",
        HOME: home,
      },
    });

    const [stdout, exitCode] = await Promise.all([proc.stdout.text(), proc.exited]);

    // Legitimate behavior still works: the typed line is persisted to
    // $HOME/.bun_repl_history on exit.
    const historyPath = path.join(home, ".bun_repl_history");
    const content = await Bun.file(historyPath).text();
    expect(content).toContain("const dbUrl");

    // The file must not be readable or writable by group/other, while the
    // owner keeps read/write access.
    const mode = statSync(historyPath).mode & 0o777;
    expect(mode & 0o077).toBe(0);
    expect(mode & 0o600).toBe(0o600);

    expect(stripAnsi(stdout)).toContain("Welcome to Poly");
    expect(exitCode).toBe(0);
  });

  test("tightens permissions on a pre-existing history file", async () => {
    using dir = tempDir("repl-history-perms-existing", {
      ".bun_repl_history": "1 + 1\n",
    });
    const home = String(dir);
    const historyPath = path.join(home, ".bun_repl_history");
    chmodSync(historyPath, 0o644);

    await using proc = Bun.spawn({
      cmd: [bunExe(), "repl"],
      stdin: Buffer.from(['const dbUrl = "postgres://user:hunter2@db.internal/prod"', ".exit", ""].join("\n")),
      stdout: "pipe",
      stderr: "pipe",
      env: {
        ...bunEnv,
        TERM: "dumb",
        NO_COLOR: "1",
        HOME: home,
      },
    });

    const [stdout, exitCode] = await Promise.all([proc.stdout.text(), proc.exited]);

    const content = await Bun.file(historyPath).text();
    expect(content).toContain("const dbUrl");

    const mode = statSync(historyPath).mode & 0o777;
    expect(mode & 0o077).toBe(0);
    expect(mode & 0o600).toBe(0o600);

    expect(stripAnsi(stdout)).toContain("Welcome to Poly");
    expect(exitCode).toBe(0);
  });
});

// `bun --interactive` boots the full node:repl + readline + acorn stack; on a
// debug+asan build that is ~4–5s per spawn, so the 5s default is too tight.
const interactiveTimeout = 20_000;

describe.concurrent("--interactive", () => {
  const env = { ...bunEnv, NO_COLOR: "1", NODE_REPL_HISTORY: "" };

  async function runInteractive(extra: string[], stdin: string, opts: { cwd?: string; env?: any } = {}) {
    await using proc = Bun.spawn({
      cmd: [bunExe(), "--interactive", ...extra],
      env: { ...env, ...opts.env },
      cwd: opts.cwd,
      // Closing stdin (EOF) exits the REPL; `.exit` adds latency on debug builds.
      stdin: Buffer.from(stdin),
      stdout: "pipe",
      stderr: "pipe",
    });
    const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
    return { stdout, stderr, exitCode };
  }

  test(
    "prints a Poly-branded banner, not 'Welcome to Node.js'",
    async () => {
      const { stdout, exitCode } = await runInteractive([], "");
      expect(stdout).toMatch(/^Welcome to Poly v\d+\.\d+\.\d+.*\(Node\.js-compatible REPL/);
      expect(stdout).not.toContain("Welcome to Node.js");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // `node -i -e 'code'`: -e runs as its own Script against globalThis, so
  // `var`/`function` declarations are visible from the REPL prompt.
  test(
    "-e var/function declarations are visible in the REPL",
    async () => {
      const { stdout, stderr, exitCode } = await runInteractive(
        ["-e", "var fromVar = 1; function f() { return 42 }"],
        "fromVar + f()\n",
      );
      expect(stdout).toContain("43");
      expect(stderr).not.toContain("error");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // `process._eval` carries the raw `-e` bytes, which are UTF-8. Decoding them
  // as Latin-1 turns every multi-byte character into mojibake, so both the
  // evaluated source and the reported `process._eval` must round-trip.
  test(
    "-e round-trips multi-byte UTF-8 through process._eval",
    async () => {
      const source = `console.log("한글-🎉-café")`;
      const { stdout, stderr, exitCode } = await runInteractive(["-e", source], "process._eval\n");
      // The -e script itself ran with its literal intact...
      expect(stdout).toContain("한글-🎉-café");
      // ...and process._eval reports the source verbatim, not re-encoded.
      expect(stdout).toContain(source);
      expect(stderr).not.toContain("error");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // `node -i -e '<bad>'`: Node exits 1 with a SyntaxError code frame at
  // [eval]:1 and never accepts REPL input; not caught by the REPL error handler.
  test(
    "-e with a syntax error is fatal and never enters the REPL",
    async () => {
      const { stdout, stderr, exitCode } = await runInteractive(["-e", "console.log(1"], "'stdin-ran'\n");
      expect(stdout).toContain("Welcome to Poly");
      // stdin was never evaluated:
      expect(stdout).not.toContain("stdin-ran");
      // The error is reported against the user's [eval] script, not the bootstrap.
      expect(stdout + stderr).toMatch(/SyntaxError/);
      expect(stdout + stderr).toContain("[eval]");
      expect(stdout + stderr).not.toMatch(/node-repl|createInternalRepl|__BUN_EVAL_SCRIPT__/);
      expect(exitCode).toBe(1);
    },
    interactiveTimeout,
  );

  test(
    "-e with a runtime error is fatal and never enters the REPL",
    async () => {
      const { stdout, stderr, exitCode } = await runInteractive(["-e", 'throw new Error("BOOM")'], "'stdin-ran'\n");
      expect(stdout).not.toContain("stdin-ran");
      expect(stdout + stderr).toContain("BOOM");
      expect(stdout + stderr).toContain("[eval]");
      expect(exitCode).toBe(1);
    },
    interactiveTimeout,
  );

  test.each(["/*", "const x=`foo"])(
    "-e with an unterminated template/comment cannot swallow the bootstrap (%j)",
    async bad => {
      const { stdout, stderr, exitCode } = await runInteractive(["-e", bad], "");
      expect(stdout).toContain("Welcome to Poly");
      expect(stdout + stderr).toMatch(/SyntaxError/);
      expect(exitCode).toBe(1);
    },
    interactiveTimeout,
  );

  // Node silently ignores `-i` when a script positional is present.
  test(
    "with a script positional runs the script and does not enter the REPL",
    async () => {
      using dir = tempDir("interactive-script", { "foo.js": `console.log("script-ran")` });
      const { stdout, stderr, exitCode } = await runInteractive(["foo.js"], "1+1\n", { cwd: String(dir) });
      expect(stdout).toContain("script-ran");
      expect(stdout).not.toContain("Welcome");
      expect(stdout).not.toContain("> ");
      expect(stderr).not.toContain("error");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // Documented "for now" deviation: `-p` wins over `--interactive`.
  test(
    "-p wins over --interactive (prints, no REPL)",
    async () => {
      const { stdout, stderr, exitCode } = await runInteractive(["-p", "1+1"], "999\n");
      expect(stdout.trim()).toBe("2");
      expect(stdout).not.toContain("Welcome");
      expect(stderr).not.toContain("error");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // exec_node_repl boots the bootstrap through the [eval] slot; process._eval
  // must still report the user's -e string (used by child_process.fork's
  // execArgv stripping), not the bootstrap.
  test(
    "process._eval reports the user's -e string, not the bootstrap",
    async () => {
      const eScript = 'console.log("EVAL=" + JSON.stringify(process._eval)); process.exit(0)';
      const { stdout, stderr, exitCode } = await runInteractive(["-e", eScript], "");
      expect(stdout).toContain(`EVAL=${JSON.stringify(eScript)}`);
      expect(stdout + stderr).not.toMatch(/__BUN_EVAL_SCRIPT__|createInternalRepl/);
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  test(
    "process._eval is undefined without -e",
    async () => {
      const { stdout, exitCode } = await runInteractive([], 'console.log("EVAL=" + process._eval)\n');
      expect(stdout).toContain("EVAL=undefined");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // The bootstrap runs -e via vm.runInThisContext (raw JS, matching
  // `node -i -e`); TypeScript syntax is a SyntaxError, not transpiled.
  test(
    "-e is raw JavaScript (not transpiled)",
    async () => {
      const { stdout, stderr, exitCode } = await runInteractive(["-e", "const x: number = 1"], "");
      expect(stdout + stderr).toMatch(/SyntaxError/);
      expect(exitCode).toBe(1);
    },
    interactiveTimeout,
  );

  // bun-as-node --interactive routes through exec_as_if_node, which used to
  // print "does not support a repl" and exit 1.
  test(
    "bun-as-node --interactive enters the REPL",
    async () => {
      await using proc = Bun.spawn({
        cmd: [bunExe(), "--interactive"],
        argv0: "node",
        env,
        stdin: Buffer.from("1+1\n"),
        stdout: "pipe",
        stderr: "pipe",
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      expect(stdout).toContain("Welcome to Poly");
      expect(stdout).toContain("2");
      expect(stderr).not.toContain("does not support a repl");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // node evaluates `-e` after createInternalRepl, via runScriptInContext, which
  // publishes the CJS bindings onto the global before running the body.
  test(
    "-e sees require/module/__filename/__dirname like `node -i -e`",
    async () => {
      const { stdout, exitCode } = await runInteractive(
        ["-e", "console.log(typeof require, typeof module, typeof __filename, typeof __dirname)"],
        "",
      );
      expect(stdout).toContain("function object string string");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // node's wrapper compiles as `[eval]-wrapper`, so __dirname is "." — NOT the
  // cwd — while module.filename stays the cwd-joined path.
  test(
    "-e exposes node's exact __dirname/__filename/module.filename",
    async () => {
      using dir = tempDir("repl-eval-dirname", {});
      const { stdout, exitCode } = await runInteractive(
        ["-e", "console.log(JSON.stringify({d: __dirname, f: __filename, m: module.filename}))"],
        "",
        { cwd: String(dir) },
      );
      const parsed = JSON.parse(stdout.slice(stdout.indexOf("{"), stdout.indexOf("}") + 1));
      expect({ d: parsed.d, f: parsed.f }).toEqual({ d: ".", f: "[eval]" });
      expect(parsed.m).toBe(path.join(String(dir), "[eval]"));
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  test(
    "-e can require() a builtin",
    async () => {
      const { stdout, exitCode } = await runInteractive(
        ["-e", 'console.log("plat:" + typeof require("os").platform)'],
        "",
      );
      expect(stdout).toContain("plat:function");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // Publishing those bindings must not move `var`/`function` off the global —
  // node runs the body in global scope, it does not CJS-wrap it.
  test(
    "-e declarations still land on the REPL's global",
    async () => {
      const { stdout, exitCode } = await runInteractive(["-e", "var x = 5; function f(){}"], "typeof x + typeof f\n");
      expect(stdout).toContain("numberfunction");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // node's `-i` is an alias for --interactive. Bun's own `-i` is
  // --install=fallback, which has no meaning under node emulation, so the node
  // meaning wins there; everywhere else `-i` stays --install=fallback.
  test(
    "bun-as-node: `node -i` enters the REPL",
    async () => {
      await using proc = Bun.spawn({
        cmd: [bunExe(), "-i"],
        argv0: "node",
        env,
        stdin: Buffer.from("1+1\n"),
        stdout: "pipe",
        stderr: "pipe",
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      expect(stdout).toContain("Welcome to Poly");
      expect(stdout).toContain("2");
      expect(stderr).not.toContain("Missing script to execute");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  test(
    "bun run --interactive is not a silent no-op",
    async () => {
      await using proc = Bun.spawn({
        cmd: [bunExe(), "run", "--interactive"],
        env,
        stdin: Buffer.from("1+1\n"),
        stdout: "pipe",
        stderr: "pipe",
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      expect(stdout).toContain("Welcome to Poly");
      expect(stdout).toContain("2");
      expect(stderr).not.toContain("error");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // The "run" subcommand word is a dispatch artifact, not user input: it must
  // not survive into the REPL's process.argv the way a script name would.
  test(
    "bun run --interactive keeps 'run' out of process.argv",
    async () => {
      await using proc = Bun.spawn({
        cmd: [bunExe(), "run", "--interactive"],
        env,
        // Tagged so the match can't be confused with the REPL's own echo.
        stdin: Buffer.from(`console.log("ARGV:" + JSON.stringify(process.argv.slice(1)))\n`),
        stdout: "pipe",
        stderr: "pipe",
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      const match = stdout.match(/ARGV:(\[.*\])/);
      expect(match).not.toBeNull();
      expect(JSON.parse(match![1])).toEqual([]);
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  test(
    "NODE_REPL_EXTERNAL_MODULE replaces the built-in REPL",
    async () => {
      using dir = tempDir("ext-repl", { "ext.js": `console.log("external-repl-42")` });
      const { stdout, stderr, exitCode } = await runInteractive([], "", {
        cwd: String(dir),
        env: { NODE_REPL_EXTERNAL_MODULE: "./ext.js" },
      });
      expect(stdout).toContain("external-repl-42");
      expect(stdout).not.toContain("Welcome");
      expect(stderr).not.toContain("error");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );
});

// ts-node does `require("repl")` at import time but only touches
// repl.start/repl.Recoverable inside createRepl(); those (plus the REPL_MODE
// symbols and isValidSyntax) are data properties so the destructure is free,
// and only calling start() or reading REPLServer/writer loads the body.
test("require('node:repl') is hollow until start() or REPLServer is used", async () => {
  await using proc = Bun.spawn({
    cmd: [
      bunExe(),
      "-e",
      `
        const repl = require("node:repl");
        const shape = n => "value" in Object.getOwnPropertyDescriptor(repl, n) ? "data" : "accessor";
        console.log(JSON.stringify({
          keys: Object.keys(repl).sort(),
          desc: {
            start: shape("start"),
            Recoverable: shape("Recoverable"),
            REPL_MODE_SLOPPY: shape("REPL_MODE_SLOPPY"),
            isValidSyntax: shape("isValidSyntax"),
            REPLServer: shape("REPLServer"),
            writer: shape("writer"),
          },
        }));
        // Reading the cheap five must not throw and must not require readline.
        const {start, Recoverable, REPL_MODE_SLOPPY, REPL_MODE_STRICT, isValidSyntax} = repl;
        console.log(JSON.stringify({
          start: typeof start,
          Recoverable: typeof Recoverable,
          REPL_MODE_SLOPPY: typeof REPL_MODE_SLOPPY,
          isValidSyntax: typeof isValidSyntax,
        }));
        console.log("recoverable-is-error=" + (new Recoverable(new SyntaxError("m")) instanceof SyntaxError));
        // Now force the full load and check REPLServer is real.
        console.log("REPLServer=" + typeof repl.REPLServer);
        // Recoverable identity: the one exposed before load is the one the impl uses.
        console.log("same-Recoverable=" + (repl.Recoverable === Recoverable));
        repl.repl = "x";
        console.log("repl.repl=" + repl.repl);
      `,
    ],
    env: bunEnv,
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
  expect(stderr).toBe("");
  const lines = stdout.trim().split("\n");
  expect(JSON.parse(lines[0])).toEqual({
    keys: [
      "REPLServer",
      "REPL_MODE_SLOPPY",
      "REPL_MODE_STRICT",
      "Recoverable",
      "isValidSyntax",
      "repl",
      "start",
      "writer",
    ],
    desc: {
      start: "data",
      Recoverable: "data",
      REPL_MODE_SLOPPY: "data",
      isValidSyntax: "data",
      REPLServer: "accessor",
      writer: "accessor",
    },
  });
  expect(JSON.parse(lines[1])).toEqual({
    start: "function",
    Recoverable: "function",
    REPL_MODE_SLOPPY: "symbol",
    isValidSyntax: "function",
  });
  expect(lines[2]).toBe("recoverable-is-error=true");
  expect(lines[3]).toBe("REPLServer=function");
  expect(lines[4]).toBe("same-Recoverable=true");
  expect(lines[5]).toBe("repl.repl=x");
  expect(exitCode).toBe(0);
});

describe.concurrent("node:repl process-global side effects", () => {
  const env = { ...bunEnv, NO_COLOR: "1" };

  // Known limitation until process.addUncaughtExceptionCaptureCallback is
  // implemented natively: the shim occupies the exclusive capture slot for the
  // process lifetime. It must NOT displace a user callback installed BEFORE the
  // first repl.start().
  test(
    "uncaught-exception capture shim defers to a pre-installed user callback",
    async () => {
      const script = `
      let userGot;
      process.setUncaughtExceptionCaptureCallback(e => { userGot = e.message; });
      const repl = require("node:repl");
      const { PassThrough } = require("node:stream");
      const inp = new PassThrough(), out = new PassThrough(); out.resume();
      const r = repl.start({ input: inp, output: out, terminal: false, prompt: "" });
      r.close();
      setImmediate(() => { throw new Error("boom"); });
      setImmediate(() => setImmediate(() => {
        console.log("userGot=" + userGot);
        // The user callback owns the slot; REPL didn't displace it.
        process.exit(0);
      }));
    `;
      await using proc = Bun.spawn({ cmd: [bunExe(), "-e", script], env, stdout: "pipe", stderr: "pipe" });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      expect(stdout).toContain("userGot=boom");
      expect(stderr).not.toContain("ALREADY_SET");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // Node filters slash-modules in addBuiltinLibsToObject (not
  // getBuiltinLibs), so `fs/promises` etc. never land on the REPL context
  // while repl.builtinModules and require-completion still list them.
  test(
    "addBuiltinLibsToObject does not install slash-modules on the REPL context",
    async () => {
      const script = `
      const repl = require("node:repl");
      const { PassThrough } = require("node:stream");
      const inp = new PassThrough(), out = new PassThrough(); out.resume();
      const r = repl.start({ input: inp, output: out, terminal: false, prompt: "" });
      const slash = Object.getOwnPropertyNames(r.context).filter(n => n.includes("/"));
      const listed = repl.builtinModules.filter(n => n.includes("/"));
      console.log("SLASH=" + JSON.stringify(slash) + " LISTED=" + (listed.includes("fs/promises")));
      r.close();
    `;
      await using proc = Bun.spawn({ cmd: [bunExe(), "-e", script], env, stdout: "pipe", stderr: "pipe" });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      expect(stdout).toContain("SLASH=[] LISTED=true");
      expect(stderr).not.toContain("error");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  // decorateErrorStack runs after user code, so a tampered String.prototype.split
  // must not stop the REPL from rendering the next error.
  test(
    "error rendering survives a tampered String.prototype.split",
    async () => {
      const script = `
      const repl = require("node:repl");
      const { PassThrough } = require("node:stream");
      const inp = new PassThrough(), out = new PassThrough();
      let buf = ""; out.on("data", d => buf += d);
      const r = repl.start({ input: inp, output: out, terminal: false, prompt: "> " });
      r.on("exit", () => { console.log(buf); process.exit(0); });
      inp.write("String.prototype.split = () => { throw 0 }\\n");
      inp.write("oops\\n");
      inp.write("1+1\\n");
      inp.end();
    `;
      await using proc = Bun.spawn({ cmd: [bunExe(), "-e", script], env, stdout: "pipe", stderr: "pipe" });
      const [stdout, , exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      expect(stdout).toContain("Uncaught ReferenceError");
      expect(stdout).toContain("> 2");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  test(
    "REPL survives a tampered RegExp.prototype[Symbol.split]",
    async () => {
      const script = `
      const repl = require("node:repl");
      const { PassThrough } = require("node:stream");
      const inp = new PassThrough(), out = new PassThrough();
      let buf = ""; out.on("data", d => buf += d);
      const r = repl.start({ input: inp, output: out, terminal: false, prompt: "> " });
      r.on("exit", () => { console.log(buf); process.exit(0); });
      inp.write("RegExp.prototype[Symbol.split] = () => { throw 0 }\\n");
      inp.write("oops\\n");
      inp.write("1+1\\n");
      inp.end();
    `;
      await using proc = Bun.spawn({ cmd: [bunExe(), "-e", script], env, stdout: "pipe", stderr: "pipe" });
      const [stdout, , exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      expect(stdout).toContain("Uncaught ReferenceError");
      expect(stdout).toContain("> 2");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );

  test(
    "REPL survives a tampered RegExp.prototype[Symbol.replace]",
    async () => {
      const script = `
      const repl = require("node:repl");
      const { PassThrough } = require("node:stream");
      const inp = new PassThrough(), out = new PassThrough();
      let buf = ""; out.on("data", d => buf += d);
      const r = repl.start({ input: inp, output: out, terminal: false, prompt: "> " });
      r.on("exit", () => { console.log(buf); process.exit(0); });
      inp.write("RegExp.prototype[Symbol.replace] = () => { throw 0 }\\n");
      inp.write("oops\\n");
      inp.write("1+1\\n");
      inp.end();
    `;
      await using proc = Bun.spawn({ cmd: [bunExe(), "-e", script], env, stdout: "pipe", stderr: "pipe" });
      const [stdout, , exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      expect(stdout).toContain("Uncaught ReferenceError");
      expect(stdout).toContain("> 2");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );
});

// JSC's Error#stack is an own data property (V8's is an accessor), so a frozen
// error makes _handleError's strict-mode `e.stack = …` rewrites throw; the Bun
// port guards those writes so the REPL prints the error and continues like Node.
describe.concurrent("node:repl prints a frozen thrown error and continues", () => {
  test.each([
    ["Error in sloppy mode", "SLOPPY", "throw Object.freeze(new Error('boom'))", "Uncaught Error: boom"],
    ["SyntaxError", "SLOPPY", "throw Object.freeze(new SyntaxError('boom'))", "Uncaught SyntaxError: boom"],
    ["Error in strict mode", "STRICT", "throw Object.freeze(new Error('boom'))", "Uncaught Error: boom"],
  ])(
    "%s",
    async (_name, mode, line, expectedFirstLine) => {
      const script = `
      const repl = require("repl");
      const { PassThrough } = require("stream");
      const inp = new PassThrough(), out = new PassThrough();
      let buf = "";
      out.on("data", d => buf += d);
      const r = repl.start({
        input: inp,
        output: out,
        terminal: false,
        prompt: "",
        useGlobal: true,
        replMode: repl.REPL_MODE_${mode},
      });
      r.on("exit", () => process.stdout.write(buf));
      inp.write(${JSON.stringify(line + "\n")});
      inp.end();
    `;
      await using proc = Bun.spawn({
        cmd: [bunExe(), "-e", script],
        env: { ...bunEnv, NO_COLOR: "1" },
        stdout: "pipe",
        stderr: "pipe",
      });
      const [stdout, stderr, exitCode] = await Promise.all([proc.stdout.text(), proc.stderr.text(), proc.exited]);
      // The frozen .stack can't be trimmed under JSC (eager materialization), so
      // assert only on the first line and that the REPL printed the next prompt.
      expect(stdout.split("\n")[0]).toBe(expectedFirstLine);
      expect(stdout).not.toContain("Attempted to assign to readonly property");
      expect(stderr).not.toContain("Attempted to assign to readonly property");
      expect(exitCode).toBe(0);
    },
    interactiveTimeout,
  );
});
