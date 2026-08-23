# Embedded Python packages

This example exercises three popular pure-Python dependency trees:

- `click` for command-line parsing;
- `requests` for HTTP request construction;
- `rich` for terminal rendering.

`main.py` prepares a request but does not send it, so the example itself does
not require network access after its dependencies have been staged.

The frozen-lock workflow is:

```text
poly sync --frozen
poly main.py --name Poly
```

The checked-in `uv.lock` is generated with the same pinned uv revision that
Poly embeds, so the example can be synchronized without first invoking a
separate package manager.

The embedded uv integration validates `pyproject.toml` and `uv.lock`, evaluates
the selected project's marker-aware dependency graph, and accepts only generic
Python 3 `none-any` wheels with locked SHA-256 hashes. `poly sync --frozen`
downloads and caches those wheels in-process, then transactionally replaces
`.poly/python`. Python entry points and TypeScript-to-Python imports discover
that directory automatically. Resolver-driven lock generation remains a later
step, so `poly sync` without `--frozen` currently reports an error.
