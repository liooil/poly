def add(left, right):
    print(f"[python] adding {left} and {right}")
    return left + right


def describe_runtime():
    return {
        "language": "python",
        "engine": "rustpython",
        "interop": "in-process-json",
    }
