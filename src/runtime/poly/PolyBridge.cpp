// PolyBridge.cpp — synchronous JS module bridge for the v1 polyglot
// interop (Python -> JS direction).
//
// The Rust host callback (poly_python::HostCallback) resolves and loads a JS
// module on the JS thread, then uses these two helpers to snapshot exports
// as JSON and to call a named export with JSON arguments:
//
//   PolyGetModuleExportsJson(global, key)  -> { values: {name: json}, functions: [name] }
//   PolyCallJsFunction(global, key, name, argsJson) -> JSON result string
//
// Modules must already be loaded and evaluated (registryEntry present); the
// Rust side drives loadAndEvaluateModule + waitForPromise before calling in.

// clang-format off
#include "root.h"
#include <JavaScriptCore/CallData.h>
#include <JavaScriptCore/Identifier.h>
#include <JavaScriptCore/JSCJSValue.h>
#include <JavaScriptCore/JSCast.h>
#include <JavaScriptCore/JSLock.h>
#include <JavaScriptCore/JSModuleLoader.h>
#include <JavaScriptCore/ModuleRegistryEntry.h>
#include <JavaScriptCore/JSModuleRecord.h>
#include <JavaScriptCore/JSModuleNamespaceObject.h>
#include <JavaScriptCore/JSString.h>
#include <JavaScriptCore/PropertyNameArray.h>
#include <JavaScriptCore/JSONObject.h>
#include <wtf/text/MakeString.h>
#include <wtf/text/StringConcatenate.h>

namespace Poly {

/// Resolve a loaded module's namespace object by registry key.
JSC::JSModuleNamespaceObject* getModuleNamespace(JSC::JSGlobalObject* global, JSC::JSValue keyValue)
{
    JSC::JSString* key = JSC::asString(keyValue);
    auto& vm = JSC::getVM(global);
    auto keyIdent = JSC::Identifier::fromString(vm, key->value(global));
    auto* entry = global->moduleLoader()->registryEntry(keyIdent);
    if (!entry)
        return nullptr;
    auto* module = entry->record();
    if (!module)
        return nullptr;
    return global->moduleLoader()->getModuleNamespaceObject(global, module);
}

/// Snapshot a loaded module's exports: JSON-serializable values become
/// {values: {name: "<json>"}}, callables are listed in {functions: [name]}.
extern "C" JSC::EncodedJSValue PolyGetModuleExportsJson(JSC::JSGlobalObject* global, JSC::JSValue keyValue)
{
    auto& vm = JSC::getVM(global);
    auto scope = DECLARE_THROW_SCOPE(vm);

    auto* ns = getModuleNamespace(global, keyValue);
    if (!ns)
        return JSC::JSValue::encode(JSC::jsNull());

    JSC::JSObject* result = JSC::constructEmptyObject(global);
    JSC::JSObject* values = JSC::constructEmptyObject(global);
    JSC::JSArray* functions = JSC::constructEmptyArray(global, nullptr, 0);

    JSC::PropertyNameArrayBuilder builder(vm, JSC::PropertyNameMode::Strings, JSC::PrivateSymbolMode::Exclude);
    JSC::JSModuleNamespaceObject::getOwnPropertyNames(ns, global, builder, JSC::DontEnumPropertiesMode::Include);
    RETURN_IF_EXCEPTION(scope, {});

    for (auto it = builder.begin(); it != builder.end(); ++it) {
        JSC::Identifier name = *it;
        JSC::JSValue value = ns->get(global, name);
        RETURN_IF_EXCEPTION(scope, {});
        if (value.isCallable()) {
            functions->push(global, JSC::jsString(vm, name.string()));
            RETURN_IF_EXCEPTION(scope, {});
        } else {
            WTF::String json = JSONStringify(global, value, 0);
            RETURN_IF_EXCEPTION(scope, {});
            if (json.isEmpty())
                continue; // non-serializable (symbol, etc): skip
            // Store the parsed JSON *value*, not the JSON text.
            JSC::JSValue jsonObject = global->get(global, JSC::Identifier::fromString(vm, "JSON"_s));
            RETURN_IF_EXCEPTION(scope, {});
            JSC::JSValue parseFn = jsonObject.get(global, vm.propertyNames->parse);
            RETURN_IF_EXCEPTION(scope, {});
            JSC::CallData parseData = JSC::getCallData(parseFn);
            JSC::MarkedArgumentBuffer parseArgs;
            parseArgs.append(JSC::jsString(vm, json));
            JSC::JSValue parsed = JSC::call(global, parseFn, parseData, jsonObject, parseArgs);
            RETURN_IF_EXCEPTION(scope, {});
            values->putDirect(vm, name, parsed);
            RETURN_IF_EXCEPTION(scope, {});
        }
    }

    result->putDirect(vm, JSC::Identifier::fromString(vm, "values"_s), values);
    result->putDirect(vm, JSC::Identifier::fromString(vm, "functions"_s), functions);
    return JSC::JSValue::encode(result);
}

/// Call a named function export with JSON arguments; returns the JSON string
/// of the result (or "null" for undefined). JS exceptions propagate to the
/// caller (Rust clears + reads `global.has_exception()`).
extern "C" JSC::EncodedJSValue PolyCallJsFunction(
    JSC::JSGlobalObject* global,
    JSC::JSValue keyValue,
    JSC::JSValue functionNameValue,
    JSC::JSValue argsJsonValue)
{
    auto& vm = JSC::getVM(global);
    auto scope = DECLARE_THROW_SCOPE(vm);

    auto* ns = getModuleNamespace(global, keyValue);
    if (!ns)
        return JSC::JSValue::encode(JSC::jsNull());

    WTF::String functionName = JSC::asString(functionNameValue)->value(global);
    RETURN_IF_EXCEPTION(scope, {});
    JSC::JSValue fn = ns->get(global, JSC::Identifier::fromString(vm, functionName));
    RETURN_IF_EXCEPTION(scope, {});
    if (!fn.isCallable())
        return JSC::JSValue::encode(JSC::jsNull());

    // Parse the JSON argument array.
    JSC::JSValue jsonObject = global->get(global, JSC::Identifier::fromString(vm, "JSON"_s));
    RETURN_IF_EXCEPTION(scope, {});
    JSC::JSValue parseFn = jsonObject.get(global, vm.propertyNames->parse);
    RETURN_IF_EXCEPTION(scope, {});
    JSC::CallData parseData = JSC::getCallData(parseFn);
    JSC::MarkedArgumentBuffer parseArgs;
    parseArgs.append(JSC::asString(argsJsonValue));
    JSC::JSValue argsArray = JSC::call(global, parseFn, parseData, jsonObject, parseArgs);
    RETURN_IF_EXCEPTION(scope, {});
    if (!argsArray.isObject())
        return JSC::JSValue::encode(JSC::jsNull());

    JSC::CallData callData = JSC::getCallData(fn);
    JSC::MarkedArgumentBuffer callArgs;
    JSC::JSArray* arr = JSC::asArray(argsArray);
    unsigned length = arr->length();
    for (unsigned i = 0; i < length; i++) {
        callArgs.append(arr->getIndex(global, i));
        RETURN_IF_EXCEPTION(scope, {});
    }
    JSC::JSValue result = JSC::call(global, fn, callData, JSC::jsUndefined(), callArgs);
    RETURN_IF_EXCEPTION(scope, {});
    WTF::String nullJson = "null"_s;
    if (result.isUndefined())
        return JSC::JSValue::encode(JSC::jsString(vm, nullJson));

    WTF::String json = JSONStringify(global, result, 0);
    RETURN_IF_EXCEPTION(scope, {});
    if (json.isEmpty())
        return JSC::JSValue::encode(JSC::jsString(vm, nullJson));
    return JSC::JSValue::encode(JSC::jsString(vm, json));
}

/// Render a thrown exception as a readable string: Error objects become
/// "Name: message (stack)", anything else uses JS toString semantics.
/// Returns a JSString. Takes the JSC `Exception` directly and reads its
/// `value()` in C++ (Bun's `JSC__Exception__asJSValue` binding returns the
/// `Exception` cell itself, not the thrown value).
extern "C" JSC::EncodedJSValue PolyExceptionToString(JSC::JSGlobalObject* global, JSC::Exception* exception)
{
    auto& vm = JSC::getVM(global);
    auto scope = DECLARE_THROW_SCOPE(vm);
    JSC::JSValue value = exception->value();

    WTF::String text;
    if (value.isObject()) {
        JSC::JSObject* obj = JSC::asObject(value);
        JSC::JSValue name = obj->get(global, vm.propertyNames->name);
        JSC::JSValue message = obj->get(global, vm.propertyNames->message);
        JSC::JSValue stack = obj->get(global, JSC::Identifier::fromString(vm, "stack"_s));
        RETURN_IF_EXCEPTION(scope, {});
        WTF::String nameStr = name.toWTFString(global);
        WTF::String messageStr = message.toWTFString(global);
        WTF::String stackStr = stack.toWTFString(global);
        RETURN_IF_EXCEPTION(scope, {});
        if (!messageStr.isEmpty() && messageStr != "undefined"_s) {
            text = WTF::makeString(nameStr, ": "_s, messageStr);
            if (!stackStr.isEmpty() && stackStr != "undefined"_s)
                text = WTF::makeString(text, "\n"_s, stackStr);
        } else {
            text = value.toWTFString(global);
            RETURN_IF_EXCEPTION(scope, {});
        }
    } else {
        text = value.toWTFString(global);
        RETURN_IF_EXCEPTION(scope, {});
    }
    return JSC::JSValue::encode(JSC::jsString(vm, text));
}

} // namespace Poly
