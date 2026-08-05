import js

math = js.import_module("./js_math.ts")

print("[python] js_math.greeting:", math.greeting)
print("[python] js_math.answer:", math.answer)
print("[python] js_math.triple(7):", math.triple(7))
