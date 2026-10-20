// Runtime validation for the generated JSON Schema subset. No QML objects or eval.
function schemaError(schema, value, root, path) {
    root = root || schema;
    path = path || "$";
    if (schema === true) return "";
    if (schema === false) return path + " is forbidden";
    if (schema.$ref) {
        if (schema.$ref.indexOf("#/") !== 0) return path + " has an unsupported schema reference";
        var target = root;
        var parts = schema.$ref.slice(2).split("/");
        for (var r = 0; r < parts.length; r++) {
            target = target && target[parts[r].replace(/~1/g, "/").replace(/~0/g, "~")];
        }
        if (target === undefined) return path + " has an unresolved schema reference";
        var referenceError = schemaError(target, value, root, path);
        if (referenceError) return referenceError;
    }
    if (schema.const !== undefined && JSON.stringify(value) !== JSON.stringify(schema.const)) return path + " differs from constant";
    if (schema.enum && !schema.enum.some(function (item) { return JSON.stringify(item) === JSON.stringify(value); })) return path + " is not a registered value";
    for (var group of ["allOf", "anyOf", "oneOf"]) {
        if (!schema[group]) continue;
        var matches = schema[group].filter(function (part) { return !schemaError(part, value, root, path); }).length;
        if ((group === "allOf" && matches !== schema[group].length) || (group === "anyOf" && matches === 0) || (group === "oneOf" && matches !== 1)) return path + " does not match " + group;
    }
    if (schema.not && !schemaError(schema.not, value, root, path)) return path + " matches forbidden schema";
    var type = value === null ? "null" : Array.isArray(value) ? "array" : typeof value;
    var types = schema.type === undefined ? null : Array.isArray(schema.type) ? schema.type : [schema.type];
    if (types && !types.some(function (expected) {
        return expected === "integer" ? typeof value === "number" && Number.isSafeInteger(value) : expected === type;
    })) return path + " has wrong type";
    if (type === "number") {
        if (!Number.isFinite(value)) return path + " is not finite";
        if (schema.minimum !== undefined && value < schema.minimum) return path + " is below minimum";
        if (schema.maximum !== undefined && value > schema.maximum) return path + " is above maximum";
        if (schema.exclusiveMinimum !== undefined && value <= schema.exclusiveMinimum) return path + " is below exclusive minimum";
        if (schema.exclusiveMaximum !== undefined && value >= schema.exclusiveMaximum) return path + " is above exclusive maximum";
    }
    if (type === "string") {
        if (schema.minLength !== undefined && Array.from(value).length < schema.minLength) return path + " is too short";
        if (schema.maxLength !== undefined && Array.from(value).length > schema.maxLength) return path + " is too long";
        if (schema.pattern && !(new RegExp(schema.pattern)).test(value)) return path + " has invalid format";
    }
    if (type === "array") {
        if (schema.minItems !== undefined && value.length < schema.minItems) return path + " has too few items";
        if (schema.maxItems !== undefined && value.length > schema.maxItems) return path + " has too many items";
        for (var i = 0; i < value.length; i++) {
            var itemError = schemaError(schema.items === undefined ? true : schema.items, value[i], root, path + "[" + i + "]");
            if (itemError) return itemError;
        }
    }
    if (type === "object") {
        var required = schema.required || [];
        for (var k = 0; k < required.length; k++) if (!Object.prototype.hasOwnProperty.call(value, required[k])) return path + "." + required[k] + " is required";
        var properties = schema.properties || {};
        for (var key of Object.keys(value)) {
            var rule = Object.prototype.hasOwnProperty.call(properties, key) ? properties[key] : schema.additionalProperties;
            if (rule === undefined) continue;
            var fieldError = schemaError(rule, value[key], root, path + "." + key);
            if (fieldError) return fieldError;
        }
    }
    return "";
}

function validateEvent(eventContracts, envelope) {
    var schema = eventContracts[envelope.stream + ":" + envelope.event];
    return schema ? schemaError(schema, envelope.data) : "";
}

function validateRequest(contracts, method, params) {
    var contract = contracts[method];
    return contract ? schemaError(contract.params_schema, params) : "";
}
function validateResponse(contracts, method, envelope) {
    var contract = contracts[method];
    if (!contract || !envelope || envelope.ok !== true) return "";
    if (!envelope.data || !Object.prototype.hasOwnProperty.call(envelope.data, contract.response_key)) return "Missing " + contract.response_key + " result";
    return schemaError(contract.result_schema, envelope.data[contract.response_key]);
}
