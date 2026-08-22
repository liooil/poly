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

  // The REPL loads and saves $HOME/.bun_repl_history (USERPROFILE on Windows).
  using home = tempDir("repl-home", {});
  await using proc = Bun.spawn({
    cmd: [bunExe(), "repl", ...args],
    stdin: Buffer.from(inputStr),
    stdout: "pipe",
    stderr: "pipe",
    env: {
      ...bunEnv,
      TERM: "dumb",
      NO_COLOR: "1",
      HOME: String(home),
      USERPROFILE: String(home),
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

async function waitForExit(proc: { exited: Promise<number> }, timeoutMs: number): Promise<number | null> {
  const timeout = AbortSignal.timeout(timeoutMs);
  return await new Promise(resolve => {
    let settled = false;
    const finish = (exitCode: number | null) => {
      if (settled) return;
      settled = true;
      timeout.removeEventListener("abort", onTimeout);
      resolve(exitCode);
    };
    const onTimeout = () => finish(null);
    timeout.addEventListener("abort", onTimeout, { once: true });
    void proc.exited.then(finish);
  });
}

interface Screen {
  /** One string per row, holding exactly the cells that were written to it. */
  rows: string[];
  cursor: { row: number; col: number };
}

// A model of a `cols` wide terminal fed with the REPL's output: text auto-wraps
// (a wide glyph that does not fit in the last cell moves to the next row whole),
// and the cursor and erase controls the line editor uses are interpreted. The
// screen is unbounded in height, so rows never scroll off.
class ScreenModel {
  private readonly rows: string[][] = [[]];
  private row = 0;
  private col = 0;
  // A glyph that fills the row leaves the cursor on the last column; the wrap
  // happens when the next glyph arrives (as in xterm).
  private pendingWrap = false;
  // An escape sequence cut off at the end of the previous chunk.
  private unfinished = "";

  constructor(private cols: number) {}

  /** Rows drawn so far keep their cells, as in a terminal that does not reflow. */
  resize(cols: number): void {
    this.cols = cols;
  }

  private rowAt(index: number): string[] {
    while (this.rows.length <= index) this.rows.push([]);
    return this.rows[index];
  }

  feed(chunk: string): void {
    const output = this.unfinished + chunk;
    this.unfinished = "";
    let i = 0;
    while (i < output.length) {
      const codePoint = output.codePointAt(i)!;
      if (codePoint === 0x1b) {
        const rest = output.slice(i, i + 24);
        const seq = /^\x1b\[([\d;]*)([A-Za-z])/.exec(rest);
        if (seq === null) {
          if (/^\x1b(\[[\d;]*)?$/.test(rest)) {
            this.unfinished = output.slice(i);
            return;
          }
          throw new Error(`ScreenModel: unsupported escape sequence ${JSON.stringify(rest)}`);
        }
        i += seq[0].length;
        this.control(seq[2], seq[1], seq[0]);
        continue;
      }
      const ch = codePoint > 0xffff ? String.fromCodePoint(codePoint) : output[i];
      i += ch.length;
      if (ch === "\r") {
        this.col = 0;
        this.pendingWrap = false;
      } else if (ch === "\n") {
        // The pty translates "\n" to "\r\n", so the column was already reset.
        this.row++;
        this.pendingWrap = false;
      } else {
        this.put(ch, codePoint < 0x80 ? 1 : Bun.stringWidth(ch));
      }
    }
  }

  private control(command: string, params: string, sequence: string): void {
    if (command === "m") return; // colors
    // The line editor only ever sends a single parameter, and never one past a terminal's size.
    if (!/^\d{0,4}$/.test(params)) {
      throw new Error(`ScreenModel: unsupported escape sequence ${JSON.stringify(sequence)}`);
    }
    const n = params === "" ? undefined : Number(params);
    switch (command) {
      case "A":
        this.row = Math.max(0, this.row - (n ?? 1));
        break;
      case "B":
        this.row += n ?? 1;
        break;
      case "C":
        this.col = Math.min(this.cols - 1, this.col + (n ?? 1));
        break;
      case "D":
        this.col = Math.max(0, this.col - (n ?? 1));
        break;
      case "J": // erase from the cursor to the end of the screen
        if ((n ?? 0) !== 0) throw new Error(`ScreenModel: unsupported erase ${JSON.stringify(sequence)}`);
        this.rowAt(this.row).length = Math.min(this.rowAt(this.row).length, this.col);
        this.rows.length = this.row + 1;
        break;
      case "K": // erase to the end of the row (0) or the whole row (2)
        if (n === 2) this.rowAt(this.row).length = 0;
        else if ((n ?? 0) === 0) this.rowAt(this.row).length = Math.min(this.rowAt(this.row).length, this.col);
        else throw new Error(`ScreenModel: unsupported erase ${JSON.stringify(sequence)}`);
        break;
      default:
        throw new Error(`ScreenModel: unsupported escape sequence ${JSON.stringify(sequence)}`);
    }
    this.pendingWrap = false;
  }

  private put(ch: string, width: number): void {
    if (width === 0) return;
    if (this.pendingWrap || this.col + width > this.cols) {
      this.row++;
      this.col = 0;
      this.pendingWrap = false;
    }
    const cells = this.rowAt(this.row);
    while (cells.length < this.col) cells.push(" ");
    cells[this.col] = ch;
    // The second cell of a wide glyph holds nothing of its own.
    for (let extra = 1; extra < width; extra++) cells[this.col + extra] = "";
    if (this.col + width >= this.cols) {
      this.col = this.cols - 1;
      this.pendingWrap = true;
    } else {
      this.col += width;
    }
  }

  screen(): Screen {
    this.rowAt(this.row);
    return { rows: this.rows.map(cells => cells.join("")), cursor: { row: this.row, col: this.col } };
  }
}

function formatScreen({ rows, cursor }: Screen): string {
  return rows
    .map((text, row) => {
      if (row !== cursor.row) return text;
      // Find the character that starts at the cursor's column.
      let column = 0;
      let index = 0;
      while (index < text.length && column < cursor.col) {
        const ch = String.fromCodePoint(text.codePointAt(index)!);
        column += Bun.stringWidth(ch);
        index += ch.length;
      }
      return text.slice(0, index) + " ".repeat(Math.max(0, cursor.col - column)) + "|" + text.slice(index);
    })
    .join("\n");
}

// Helper to run REPL in a PTY and interact with it
async function withTerminalRepl(
  fn: (helpers: {
    terminal: Bun.Terminal;
    proc: Bun.ChildProcess;
    send: (text: string) => void;
    waitFor: (pattern: string | RegExp, timeoutMs?: number) => Promise<string>;
    /** Resolves with the rendered screen once `ready` accepts it. */
    waitForScreen: (ready: (screen: Screen) => boolean, timeoutMs?: number) => Promise<Screen>;
    /** Changes the width of the pty, and of the screen that `waitForScreen` renders. */
    resize: (cols: number) => void;
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
  const rows = 40;
  let cursor = 0;
  let resolveWaiter: (() => void) | null = null;
  // Streaming, so that a multi-byte character split across two reads stays intact.
  const decoder = new TextDecoder();

  // The REPL loads and saves $HOME/.bun_repl_history (USERPROFILE on Windows).
  using home = tempDir("repl-home", {});
  await using terminal = new Bun.Terminal({
    cols,
    rows,
    data(_term, data) {
      const str = decoder.decode(data, { stream: true });
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
      HOME: String(home),
      USERPROFILE: String(home),
      ...env,
    },
  });

  const send = (text: string) => terminal.write(text);

  const waitForOutput = async (observedChunks: number, deadline: number): Promise<void> => {
    const remaining = deadline - Date.now();
    if (remaining <= 0) return;
    await new Promise<void>(resolve => {
      const timeout = AbortSignal.timeout(remaining);
      let settled = false;
      const finish = () => {
        if (settled) return;
        settled = true;
        timeout.removeEventListener("abort", finish);
        if (resolveWaiter === finish) resolveWaiter = null;
        resolve();
      };
      timeout.addEventListener("abort", finish, { once: true });
      resolveWaiter = finish;
      if (received.length !== observedChunks) finish();
    });
  };

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
      await waitForOutput(received.length, deadline);
    }
  };

  // The REPL redraws within milliseconds of a keystroke; the deadline is well
  // below the test timeout so that a failure reports the screen it got stuck on.
  const model = new ScreenModel(cols);
  let modelled = 0;
  const renderReceived = () => {
    while (modelled < received.length) model.feed(received[modelled++]);
  };
  const waitForScreen = async (ready: (screen: Screen) => boolean, timeoutMs = 3000): Promise<Screen> => {
    const deadline = Date.now() + timeoutMs;
    while (true) {
      renderReceived();
      const screen = model.screen();
      if (ready(screen)) return screen;
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        throw new Error(`Timed out waiting for the screen. Last screen (| marks the cursor):\n${formatScreen(screen)}`);
      }
      await waitForOutput(received.length, deadline);
    }
  };

  // Call this while the REPL waits for input: what it wrote before is laid out
  // at the old width, and what it writes next at the new one.
  const resize = (newCols: number) => {
    renderReceived();
    terminal.resize(newCols, rows);
    model.resize(newCols);
  };

  const allOutput = () => stripAnsi(received.join(""));
  const rawOutput = () => received.join("");

  await waitFor(/\u276f|> /, startupTimeoutMs); // Wait for prompt

  await fn({ terminal, proc, send, waitFor, waitForScreen, resize, allOutput, rawOutput });

  // Clean exit. Ctrl+A then Ctrl+K first discards whatever the test left on
  // the line, wherever the cursor is, so `.exit` is not appended to it (which
  // would leave the REPL running until the kill below).
  send("\x01\x0b.exit\n");
  if ((await waitForExit(proc, 2000)) === null) proc.kill();
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
      expect(output).toMatch(/\n7\n> \n42\n> \n7\n> \n$/);
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
      expect(output).toMatch(/\ntrue\n> \n2\n> \n2\n> \n$/);
      expect(exitCode).toBe(0);
    });

    test("assignments inside template substitutions disable automatic last-result updates", async () => {
      const { stdout, exitCode } = await runRepl(["`${(_ = 9)}`", "40 + 2", "_", ".exit"]);
      const output = stripAnsi(stdout);

      expect(output).toContain("Expression assignment to _ now disabled.");
      expect(output).toMatch(/\n"9"\n> \n42\n> \n9\n> \n$/);
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
      const exitCode = await waitForExit(proc, 3000);
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
      await waitFor("111 + 222");
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
      async ({ send, waitFor, waitForScreen }) => {
        send("\x1b[A");
        const recalled = await waitForScreen(
          screen =>
            screen.rows.at(-3) === "> let persisted = (" && screen.rows.at(-2) === "41" && screen.rows.at(-1) === ")",
        );
        expect(recalled.rows.slice(-3)).toEqual(["> let persisted = (", "41", ")"]);

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
      async ({ send, waitForScreen }) => {
        const top = (await waitForScreen(screen => screen.rows.at(-1) === "> ")).rows.length - 1;

        send("1234567890");
        let screen = await waitForScreen(screen => screen.cursor.row === top + 1 && screen.cursor.col === 0);
        expect(screen.rows.slice(top)).toEqual(["> 1234567890", ""]);

        send("\x7f");
        screen = await waitForScreen(screen => screen.rows.at(-1) === "> 123456789" && screen.cursor.col === 11);
        expect(screen.rows.slice(top)).toEqual(["> 123456789"]);

        send("0\n");
        screen = await waitForScreen(screen => screen.rows.at(-1) === "> " && screen.rows.at(-2) === "1234567890");

        send("1000000000 + 1");
        const promptRow = screen.rows.length - 1;
        screen = await waitForScreen(
          screen => screen.rows[promptRow] === "> 1000000000" && screen.rows[promptRow + 1] === " + 1",
        );
        expect(screen.rows.slice(promptRow, promptRow + 2)).toEqual(["> 1000000000", " + 1"]);

        send("\x01");
        await waitForScreen(screen => screen.cursor.row === promptRow && screen.cursor.col === 2);

        send("\x1b[3~2\x05\n");
        await waitForScreen(screen => screen.rows.at(-1) === "> " && screen.rows.at(-2) === "2000000001");
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
      // Underscore-prefixed globals are skipped by JS-to-Python sync, so trap reads here can only come from completion.
      send(
        "let getterReads = 0, proxyReads = 0; const _holder = {}; " +
          "Object.defineProperty(_holder, 'target', { get() { getterReads++; return { previewValue: 1 } } }); " +
          "const _proxyTarget = new Proxy({ previewValue: 1 }, { get(target, key, receiver) { proxyReads++; return Reflect.get(target, key, receiver) } }); void 0;\n",
      );
      await waitFor(/\u276f|> /);

      send("_holder.target.previewV");
      await waitFor("_holder.target.previewV");
      send("\x03");
      await waitFor(/\u276f|> /);
      send("_proxyTarget.previewV");
      await waitFor("_proxyTarget.previewV");
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
        send("_ \n");
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

  describe("inline suggestions", () => {
    // Ghost text is rendered as: ESC[2m<remainder>ESC[0m after the typed text.
    // The feature requires colors, so override bunEnv's NO_COLOR for these.
    const DIM = "\x1b[2m";
    const colorEnv = { NO_COLOR: undefined, FORCE_COLOR: "1" };

    test("suggests global completion while typing", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          // "cons" should suggest "ole" (-> console). There are longer globals
          // like `constructor` on the prototype chain, but the REPL picks the
          // shortest match.
          send("cons");
          await waitFor(`${DIM}ole`);
        },
        { env: colorEnv },
      );
    });

    test("right arrow accepts the suggestion", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          send("JSO");
          await waitFor(`${DIM}N`);
          send("\x1b[C"); // Right arrow accepts the ghost text
          // After acceptance the input is `JSON`; extend it and evaluate.
          // The result "81" never appears in the echoed input, so this only
          // matches once the expression actually evaluates, proving the ghost
          // was accepted (otherwise `JSO.stringify` is a ReferenceError).
          send(".stringify(9*9)\n");
          await waitFor('"81"');
        },
        { env: colorEnv },
      );
    });

    test("suggests property completion after a dot", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          // Build up `console.` by accepting the global suggestion first.
          send("cons");
          await waitFor(`${DIM}ole`);
          send("\x1b[C"); // accept -> "console"
          send(".l");
          // console.l -> "log" is the shortest property starting with "l"
          await waitFor(`${DIM}og`);
        },
        { env: colorEnv },
      );
    });

    test("tab accepts the visible suggestion", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          send("JSON.str");
          await waitFor(`${DIM}ingify`, 10000);
          send("\t"); // Tab accepts the ghost suggestion -> `JSON.stringify`
          // Result "81" cannot occur in the echoed input, so it only matches
          // if Tab really completed to `stringify` and the call succeeded.
          send("(9*9)\n");
          await waitFor('"81"');
        },
        { env: colorEnv },
      );
    }, 15000);

    test("end key accepts the suggestion", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          send("Mat");
          await waitFor(`${DIM}h`);
          send("\x1b[F"); // End accepts the suggestion -> "Math"
          // Result 63 cannot occur in the echoed input; if End didn't accept,
          // `Mat.max(...)` would throw instead of producing it.
          send(".max(4,7)*9\n");
          await waitFor("63");
        },
        { env: colorEnv },
      );
    });

    test("suggests first property when prefix is empty after dot", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          // Define an object with a single distinctive property so the
          // suggestion is deterministic regardless of prototype ordering.
          send("globalThis.__sgObj = Object.create(null); __sgObj.onlyProp = 1\n");
          await waitFor("1");
          send("__sgObj.");
          await waitFor(`${DIM}onlyProp`);
        },
        { env: colorEnv },
      );
    });

    test("falls back to JS keywords when no global matches", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          // No global starts with "instan", but the keyword `instanceof` does.
          send("x instan");
          await waitFor(`${DIM}ceof`);
        },
        { env: colorEnv },
      );
    });

    test("no global or keyword suggestion for a property of an unresolvable expression", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          send('globalThis.__o = () => ({ th: "no" + "Ghost", this: "had" + "Ghost" }); "o" + "Ready"\n');
          await waitFor("oReady");
          // `.th` names a property of the call result, so the keyword fallback
          // (`this`) must not kick in. Right arrow would accept such a ghost,
          // turning the line into `__o().this`.
          send("__o().th\x1b[C\n");
          await waitFor("noGhost");
        },
        { env: colorEnv },
      );
    });

    test("suggests non-ASCII property names that are valid identifiers", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          // Both keys start with "caf" and have the same byte length, and `caf→`
          // comes first; only `cafés` can follow a `.`, so the ghost must be its
          // remainder.
          send(
            'globalThis.__u = { "caf\\u2192": 1, "caf\\u00e9s": 2 }; globalThis["caf\\u00e9"] = { latte: 1 }; "u" + "Ready"\n',
          );
          await waitFor("uReady");
          send("__u.caf");
          await waitFor(`${DIM}és`);
          // The typed prefix itself may contain non-ASCII characters.
          send("é");
          await waitFor(`${DIM}s`);
          // So may the object being completed: the chain segment is looked up as
          // UTF-8, not byte-per-character.
          send("\x15café.");
          await waitFor(`${DIM}latte`);
        },
        { env: colorEnv },
      );
    });

    test("resolves chain segments inherited from Object.prototype", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          // `constructor` comes from Object.prototype; it must resolve to `Object`
          // like a real property access would, not stop at the object's own keys.
          send('globalThis.__plain = {}; "plain" + "Ready"\n');
          await waitFor("plainReady");
          send("__plain.constructor.getOwnPropertyNa");
          await waitFor(`${DIM}mes`);
        },
        { env: colorEnv },
      );
    });

    test("resolves chains through primitive values", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          // `process.version` is a string and `.length` a number; both get boxed
          // the way a real property access would, ending on Number.prototype.
          send("process.version.length.toF");
          await waitFor(`${DIM}ixed`);
        },
        { env: colorEnv },
      );
    });

    test("spread dots do not turn the word into a property access", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          send("[...cons");
          await waitFor(`${DIM}ole`);
          // Nor do they swallow the chain that follows them.
          send("\x15[...console.l");
          await waitFor(`${DIM}og`);
        },
        { env: colorEnv },
      );
    });

    test("a chain starting with `this` completes against the global object", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          send("this.cons");
          await waitFor(`${DIM}ole`);
        },
        { env: colorEnv },
      );
    });

    test("no suggestions inside a string literal", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          // `st` would otherwise match globals such as `structuredClone`, and the
          // right arrow would accept that ghost, changing the evaluated string.
          send('"st\x1b[C" + "x"\n');
          await waitFor('"stx"');
        },
        { env: colorEnv },
      );
    });

    test("tab inside a string literal indents instead of completing", async () => {
      await withTerminalRepl(async ({ send, waitFor }) => {
        // `JSON.pars` has exactly one completion, which Tab would otherwise splice
        // into the string; indenting adds two spaces, so the length is 9 + 2.
        send('("JSON.pars\t").length\n');
        await waitFor(/\b11\b/);
      });
    });

    test("suggests inside a template hole but not in the template text around it", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          send("`${JSO");
          await waitFor(`${DIM}N`);
          // Back in template text after the `}`: "st" must get no ghost, or the
          // right arrow would accept it. `${JSON}` stringifies to the 13-char
          // "[object JSON]", plus " st", so the length is 16.
          send("N} st\x1b[C`.length === 16\n");
          await waitFor("true");
        },
        { env: colorEnv },
      );
    });

    test("tab on a continuation line of a template literal indents", async () => {
      await withTerminalRepl(async ({ send, waitFor }) => {
        send("globalThis.__tpl = `\n");
        await waitFor("...");
        // The backtick was opened on the previous line. Without that context
        // Tab would complete `JSON.pars` to `parse` inside the template.
        send("JSON.pars\t`\n");
        await waitFor(/\u276f|> /);
        // "\n" + "JSON.pars" + two spaces.
        send("__tpl.length\n");
        await waitFor(/\b12\b/);
      });
    });

    test("tab on a continuation line outside a string still completes", async () => {
      await withTerminalRepl(async ({ send, waitFor }) => {
        send("function __cont() {\n");
        await waitFor("...");
        send("return JSON.pars\t\n");
        send("}\n");
        await waitFor(/\u276f|> /);
        send("__cont() === JSON.parse\n");
        await waitFor("true");
      });
    });

    test("completion on a Proxy with a misbehaving getPrototypeOf trap does not hang", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          // The trap alternates between ending the chain and pointing back at
          // the proxy, so any completer that walks the chain itself never finishes.
          send(
            'globalThis.__flipN = 0; globalThis.__flip = new Proxy({}, { getPrototypeOf: () => (__flipN++ % 2 ? __flip : null) }); "flip" + "Ready"\n',
          );
          await waitFor("flipReady");
          // Typing the `.` computes completions for `__flip`; Ctrl+C then
          // discards the line and the REPL must still evaluate the next one.
          send("__flip.\x03");
          send('"still" + "Alive"\n');
          await waitFor("stillAlive");
        },
        { env: colorEnv },
      );
    });

    test("tab completes properties on an object (no ghost)", async () => {
      // Tab completion resolves `obj.prefix` chains even when ghost text is
      // disabled (NO_COLOR), so this covers parse_completion_context + resolve.
      await withTerminalRepl(async ({ send, waitFor }) => {
        // Store the marker as two halves so it never appears in the echoed
        // input; it only shows up once the completed property is evaluated.
        send("globalThis.__tcObj = { uniqueLongName: 'tcMAR' + 'KER' }; 0\n");
        await waitFor(/\b0\b/);
        send("__tcObj.uni");
        send("\t");
        // After tab, the full property should be in the input; evaluate it.
        send("\n");
        await waitFor("tcMARKER");
      });
    });

    test("enter accepts the visible suggestion before evaluating", async () => {
      await withTerminalRepl(
        async ({ send, waitFor }) => {
          send('globalThis.zzGhostMarker = "enter" + "Accepted"\n');
          await waitFor("enterAccepted");
          send("zz");
          await waitFor(`${DIM}GhostMarker`);
          send("\n");
          await waitFor("enterAccepted");
        },
        { env: colorEnv },
      );
    });
  });

  describe("input wider than the terminal", () => {
    // Every keystroke redraws the whole input. When the input spans more than
    // one terminal row, the redraw has to start on the prompt's row, otherwise
    // each keystroke leaves another stale copy of the first row behind.
    const cols = 30;
    // "> " plus 37 characters: the first 28 characters share the prompt's row.
    const line = "[111, 222, 333, 444, 555, 666].length";
    const split = cols - "> ".length;

    test("a wrapped line is redrawn in place and its result is printed below it", async () => {
      await withTerminalRepl(
        async ({ send, waitForScreen }) => {
          const top = (await waitForScreen(s => s.rows.at(-1) === "> ")).rows.length - 1;

          send(line);
          let screen = await waitForScreen(s => s.rows.at(-1) === line.slice(split));
          expect(screen.rows.slice(top)).toEqual([`> ${line.slice(0, split)}`, line.slice(split)]);
          await waitForScreen(s => s.cursor.row === top + 1 && s.cursor.col === line.length - split);

          // Ctrl+A, then insert at the start: the cursor is on the prompt's row
          // while the line still continues on the next one.
          send("\x01" + "0;");
          const edited = `0;${line}`;
          screen = await waitForScreen(s => s.rows.at(-1) === edited.slice(split));
          expect(screen.rows.slice(top)).toEqual([`> ${edited.slice(0, split)}`, edited.slice(split)]);
          await waitForScreen(s => s.cursor.row === top && s.cursor.col === "> 0;".length);

          send("\n");
          screen = await waitForScreen(s => s.rows.at(-1) === "> " && s.rows.at(-2) === "6");
          expect(screen.rows.slice(top)).toEqual([`> ${edited.slice(0, split)}`, edited.slice(split), "6", "> "]);
        },
        { cols },
      );
    });

    test("Ctrl+C is echoed after the end of a wrapped line", async () => {
      await withTerminalRepl(
        async ({ send, waitForScreen }) => {
          const top = (await waitForScreen(s => s.rows.at(-1) === "> ")).rows.length - 1;

          send(line);
          await waitForScreen(s => s.rows.at(-1) === line.slice(split));
          // Ctrl+A moves the cursor up to the prompt's row, then Ctrl+C.
          send("\x01\x03");
          const screen = await waitForScreen(s => s.rows.at(-1) === "> " && s.rows.at(-2) === `${line.slice(split)}^C`);
          expect(screen.rows.slice(top)).toEqual([`> ${line.slice(0, split)}`, `${line.slice(split)}^C`, "> "]);
        },
        { cols },
      );
    });

    test("recalling a shorter history entry clears the rows of a longer one", async () => {
      await withTerminalRepl(
        async ({ send, waitForScreen }) => {
          const top = (await waitForScreen(s => s.rows.at(-1) === "> ")).rows.length - 1;

          send(`${line}\n`);
          await waitForScreen(s => s.rows.at(-1) === "> " && s.rows.at(-2) === "6");
          send("7\n");
          await waitForScreen(s => s.rows.at(-1) === "> " && s.rows.at(-2) === "7");

          send("\x1b[A"); // Up: 7
          await waitForScreen(s => s.rows.at(-1) === "> 7");
          send("\x1b[A"); // Up: the wrapped line
          let screen = await waitForScreen(s => s.rows.at(-1) === line.slice(split));
          const history = [`> ${line.slice(0, split)}`, line.slice(split), "6", "> 7", "7"];
          expect(screen.rows.slice(top)).toEqual([...history, `> ${line.slice(0, split)}`, line.slice(split)]);

          send("\x1b[B"); // Down: back to 7, which needs one row less
          screen = await waitForScreen(s => s.rows.at(-1) === "> 7");
          expect(screen.rows.slice(top)).toEqual([...history, "> 7"]);
          await waitForScreen(s => s.cursor.row === screen.rows.length - 1 && s.cursor.col === "> 7".length);
        },
        { cols },
      );
    });

    test("ghost text that does not fit on the row wraps and is accepted on enter", async () => {
      // Colors enable ghost text; the prompt is then "❯ ".
      const cols = 40;
      // "❯ " plus 30 characters leaves room for 8 of the 14 ghost characters.
      const input = "[1, 2, 3, 4, 5, 6].length + zz";
      await withTerminalRepl(
        async ({ send, waitForScreen }) => {
          const top = (await waitForScreen(s => s.rows.at(-1) === "❯ ")).rows.length - 1;

          // `zz` itself is skipped as a suggestion for `zz`, so the ghost is
          // the remainder of zzLongSuffixName.
          send("var zz = 1, zzLongSuffixName = 5\n");
          await waitForScreen(s => s.rows.at(-1) === "❯ " && s.rows.at(-2) === "5");
          const setup = ["❯ var zz = 1, zzLongSuffixName = 5", "5"];

          send(input);
          let screen = await waitForScreen(s => s.rows.at(-1) === "ixName");
          expect(screen.rows.slice(top)).toEqual([...setup, `❯ ${input}LongSuff`, "ixName"]);
          // The cursor stays in front of the ghost, on the prompt's row.
          await waitForScreen(s => s.cursor.row === top + setup.length && s.cursor.col === `❯ ${input}`.length);

          send("\n");
          screen = await waitForScreen(s => s.rows.at(-1) === "❯ " && s.rows.at(-2) === "11");
          expect(screen.rows.slice(top)).toEqual([...setup, `❯ ${input}LongSuff`, "ixName", "11", "❯ "]);
          const evaluated = [...setup, `❯ ${input}LongSuff`, "ixName", "11"];

          // "❯ " plus 38 characters fills the row, so the whole ghost goes to the
          // next row, and the cursor with it.
          const filling = "[1, 2, 3, 4, 5, 6, 7, 888].length + zz";
          send(filling);
          screen = await waitForScreen(s => s.rows.at(-1) === "LongSuffixName");
          expect(screen.rows.slice(top)).toEqual([...evaluated, `❯ ${filling}`, "LongSuffixName"]);
          await waitForScreen(s => s.cursor.row === screen.rows.length - 1 && s.cursor.col === 0);

          // Enter accepts the ghost before evaluation.
          send("\n");
          screen = await waitForScreen(s => s.rows.at(-1) === "❯ " && s.rows.at(-2) === "13");
          expect(screen.rows.slice(top)).toEqual([...evaluated, `❯ ${filling}`, "LongSuffixName", "13", "❯ "]);
        },
        { cols, env: { NO_COLOR: undefined, FORCE_COLOR: "1" } },
      );
    });

    test("a line that fills the row exactly leaves no blank row behind", async () => {
      // "> " plus 28 characters is exactly 30 columns.
      const filling = `"${Buffer.alloc(19, "a")}".length`;
      await withTerminalRepl(
        async ({ send, waitForScreen }) => {
          const top = (await waitForScreen(s => s.rows.at(-1) === "> ")).rows.length - 1;

          send(filling);
          // The cursor has nowhere to go on the full row, so it waits on an empty one.
          let screen = await waitForScreen(s => s.cursor.row === top + 1 && s.cursor.col === 0);
          expect(screen.rows.slice(top)).toEqual([`> ${filling}`, ""]);

          send("\n");
          screen = await waitForScreen(s => s.rows.at(-1) === "> " && s.rows.at(-2) === "19");
          expect(screen.rows.slice(top)).toEqual([`> ${filling}`, "19", "> "]);

          send(filling);
          await waitForScreen(s => s.rows.at(-1) === "" && s.rows.at(-2) === `> ${filling}`);
          send("\x03");
          screen = await waitForScreen(s => s.rows.at(-1) === "> " && s.rows.at(-2) === "^C");
          expect(screen.rows.slice(top)).toEqual([`> ${filling}`, "19", `> ${filling}`, "^C", "> "]);
        },
        { cols },
      );
    });

    test("wide characters wrap the way the terminal wraps them", async () => {
      // 21 columns: a 2 column glyph never fits in the last cell of a row that
      // starts on an even column, so those rows hold one column less than the
      // width. `> "` and nine glyphs fill the first row; the second row holds ten
      // glyphs and a blank cell; the rest goes on the third row.
      const cols = 21;
      const glyphs = Array.from({ length: 25 }, () => "日");
      const input = `"${glyphs.join("")}".length`;
      await withTerminalRepl(
        async ({ send, waitForScreen }) => {
          const top = (await waitForScreen(s => s.rows.at(-1) === "> ")).rows.length - 1;

          send(input);
          const third = `${glyphs.slice(19).join("")}".length`;
          let screen = await waitForScreen(s => s.rows.at(-1) === third);
          expect(screen.rows.slice(top)).toEqual([
            `> "${glyphs.slice(0, 9).join("")}`,
            glyphs.slice(9, 19).join(""),
            third,
          ]);
          // Six glyphs and `".length` are 20 columns.
          await waitForScreen(s => s.cursor.row === top + 2 && s.cursor.col === 20);

          send("\n");
          screen = await waitForScreen(s => s.rows.at(-1) === "> " && s.rows.at(-2) === "25");
          expect(screen.rows.slice(top + 2)).toEqual([third, "25", "> "]);
        },
        { cols },
      );
    });

    test("the cursor is positioned past column 80 on a wide terminal", async () => {
      const cols = 120;
      const input = Buffer.alloc(90, "q").toString();
      await withTerminalRepl(
        async ({ send, waitFor, waitForScreen }) => {
          const top = (await waitForScreen(s => s.rows.at(-1) === "> ")).rows.length - 1;

          send(input);
          await waitFor(`> ${input}`);
          send("\x1b[D"); // Left
          await waitForScreen(s => s.cursor.row === top && s.cursor.col === `> ${input}`.length - 1);

          send("Z");
          const edited = `${input.slice(0, -1)}Zq`;
          await waitForScreen(s => s.rows.at(-1) === `> ${edited}` && s.cursor.col === `> ${edited}`.length - 1);
        },
        { cols },
      );
    });

    test("a terminal narrowed after startup wraps the input at its new width", async () => {
      // The pty starts out 120 columns wide, on which the line fits on one row.
      await withTerminalRepl(async ({ send, resize, waitForScreen }) => {
        const top = (await waitForScreen(s => s.rows.at(-1) === "> ")).rows.length - 1;
        resize(cols);

        send(line);
        let screen = await waitForScreen(s => s.rows.at(-1) === line.slice(split));
        expect(screen.rows.slice(top)).toEqual([`> ${line.slice(0, split)}`, line.slice(split)]);
        await waitForScreen(s => s.cursor.row === top + 1 && s.cursor.col === line.length - split);

        send("\n");
        screen = await waitForScreen(s => s.rows.at(-1) === "> " && s.rows.at(-2) === "6");
        expect(screen.rows.slice(top)).toEqual([`> ${line.slice(0, split)}`, line.slice(split), "6", "> "]);
      });
    });
  });

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
