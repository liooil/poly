type JsonPrimitive = null | boolean | number | string;
export type JsonValue =
  | JsonPrimitive
  | JsonValue[]
  | { [key: string]: JsonValue };

interface PythonCallResponse {
  ok: boolean;
  value?: JsonValue;
  stdout?: string;
  error?: string;
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

async function callPython(
  module: string,
  functionName: string,
  args: JsonValue[] = [],
): Promise<JsonValue> {
  const request = JSON.stringify({
    module,
    function: functionName,
    args,
  });

  let response: PythonCallResponse;
  try {
    const nativeBun = Bun as typeof Bun & {
      polyPythonCall(requestJson: string): string;
    };
    response = JSON.parse(nativeBun.polyPythonCall(request)) as PythonCallResponse;
  } catch (error) {
    throw new Error(`In-process Python bridge failed: ${String(error)}`);
  }

  if (response.stdout) {
    process.stdout.write(response.stdout);
  }
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
