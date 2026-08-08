# Embedded Python packages

This example exercises three popular pure-Python dependency trees:

- `click` for command-line parsing;
- `requests` for HTTP request construction;
- `rich` for terminal rendering.

`main.py` prepares a request but does not send it, so the example itself does
not require network access after its dependencies have been staged.

The intended workflow is:

```text
poly sync
poly main.py --name Poly
```

The embedded uv integration currently validates `pyproject.toml` and
`uv.lock`, builds a conservative install plan containing only generic Python 3
`none-any` wheels, verifies downloaded archives against the locked SHA-256,
and stages local wheel archives into `.poly/python`. Resolver-driven lock
generation and in-process download/cache wiring are the remaining steps before
the workflow above is executable end to end.
