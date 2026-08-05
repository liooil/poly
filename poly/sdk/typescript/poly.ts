type JsonPrimitive = null | boolean | number | string;
export type JsonValue =
  | JsonPrimitive
  | JsonValue[]
  | { [key: string]: JsonValue };

interface BridgeResponse {
  ok: boolean;
  value?: JsonValue;
  exports?: {
    names: string[];
    constants: Record<string, JsonValue>;
    callables: string[];
  };
  error?: string;
  error_kind?: string;
  traceback?: string;
}

export class PythonCallError extends Error {
  readonly pythonTraceback?: string;

  constructor(message: string, pythonTraceback?: string) {
    super(message);
    this.name = "PythonCallError";
    this.pythonTraceback = pythonTraceback;
  }
}

function bridge(request: unknown): BridgeResponse {
  const nativeBun = Bun as typeof Bun & {
    polyPythonCall(requestJson: string): string;
  };
  return JSON.parse(nativeBun.polyPythonCall(JSON.stringify(request))) as BridgeResponse;
}

async function callPython(
  module: string,
  functionName: string,
  args: JsonValue[] = [],
): Promise<JsonValue> {
  // The runtime registry keeps modules loaded per process; `load` is
  // idempotent and must precede `call` (v1 protocol).
  const load = bridge({ kind: "load", module, function: "", args: [] });
  if (!load.ok) {
    throw new PythonCallError(load.error ?? "Python module failed to load", load.traceback);
  }

  const response = bridge({ kind: "call", module, function: functionName, args });
  if (!response.ok) {
    throw new PythonCallError(
      response.error ?? "Python call failed",
      response.traceback,
    );
  }

  return response.value ?? null;
}

export const python = {
  call: callPython,
};
