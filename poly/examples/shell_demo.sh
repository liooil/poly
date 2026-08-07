#!/usr/bin/env sh
# Poly Shell demo — runs in-process via Bun Shell (bash-like subset).
# Builtins execute inside the runtime; external commands spawn per Shell
# semantics. Control flow: if/else, && / ||, pipes, redirects.

echo "Hello from Bun Shell"
if true; then
  echo "control flow works"
fi
echo "one two three" | tr ' ' '\n' | wc -l
exit 0
