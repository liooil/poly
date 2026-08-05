"""Math tools fixture for Poly interop tests."""

import sys

PI = 3.1416
__all__ = ["add", "greet", "sum_list", "PI", "get_call_count"]

_call_count = 0


def add(left, right):
    """Add two numbers."""
    print(f"[python] adding {left} and {right}")
    return left + right


def greet(name):
    """Greet someone."""
    return f"hello {name}"


def sum_list(items):
    """Sum a list of numbers."""
    return sum(items)


def get_call_count():
    """Return how many times we've been called."""
    global _call_count
    _call_count += 1
    return _call_count


def _internal_helper():
    """Should not be exported."""
    return "internal"


def make_object():
    """Return a custom object (unsupported in v1)."""
    return object()