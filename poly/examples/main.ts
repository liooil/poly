import { python } from "../sdk/typescript/poly.ts";
import { fileURLToPath } from "node:url";

console.log("Hello from Bun");

const modulePath = fileURLToPath(new URL("./math_tools.py", import.meta.url));
const sum = await python.call(modulePath, "add", [20, 22]);
const runtime = await python.call(modulePath, "describe_runtime");

console.log("[typescript] result:", sum);
console.log("[typescript] python runtime:", runtime);
