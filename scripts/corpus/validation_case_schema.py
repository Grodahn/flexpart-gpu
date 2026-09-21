"""Dependency-free validator for the canonical validation-case v2 schema.

The repository intentionally has no Python package/dependency layer for corpus
scripts.  This module therefore implements the small JSON-Schema 2020-12
subset used by schemas/validation-case-v2.schema.json.  Unsupported schema
keywords fail closed so a future schema extension cannot silently weaken the
Python consumers.
"""

import json
import math
import re
from functools import lru_cache
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_SCHEMA = REPO / "schemas" / "validation-case-v2.schema.json"

_SUPPORTED_SCHEMA_KEYS = {
    "$ref", "$schema", "$id", "$defs", "$comment",
    "title", "description", "default", "examples",
    "type", "const", "enum", "oneOf",
    "properties", "required", "additionalProperties",
    "items", "minItems", "maxItems",
    "minLength", "maxLength", "pattern",
    "minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf",
}


class ValidationCaseSchemaError(ValueError):
    """Raised when a case is not valid under validation-case-v2.schema.json."""


@lru_cache(maxsize=4)
def _load_schema(path_text: str):
    path = Path(path_text)
    return json.loads(path.read_text(encoding="utf-8"))


def _json_type_name(value):
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, str):
        return "string"
    if isinstance(value, list):
        return "array"
    if isinstance(value, dict):
        return "object"
    if isinstance(value, int):
        return "integer"
    if isinstance(value, float):
        return "number"
    return type(value).__name__


def _type_matches(expected, value):
    if expected == "null":
        return value is None
    if expected == "boolean":
        return isinstance(value, bool)
    if expected == "string":
        return isinstance(value, str)
    if expected == "array":
        return isinstance(value, list)
    if expected == "object":
        return isinstance(value, dict)
    if expected == "integer":
        return (
            isinstance(value, int)
            and not isinstance(value, bool)
        ) or (
            isinstance(value, float)
            and math.isfinite(value)
            and value.is_integer()
        )
    if expected == "number":
        return (
            isinstance(value, (int, float))
            and not isinstance(value, bool)
            and (not isinstance(value, float) or math.isfinite(value))
        )
    raise ValidationCaseSchemaError(f"unsupported schema type {expected!r}")


def _json_equal(left, right):
    # JSON booleans are distinct from numbers even though bool subclasses int
    # in Python. JSON numeric equality otherwise treats 1 and 1.0 alike.
    if isinstance(left, bool) or isinstance(right, bool):
        return type(left) is type(right) and left == right
    return left == right


def _resolve_ref(root, reference, path):
    if not reference.startswith("#/"):
        raise ValidationCaseSchemaError(
            f"{path}: only local JSON-Pointer $ref values are supported, got {reference!r}"
        )
    node = root
    for raw in reference[2:].split("/"):
        token = raw.replace("~1", "/").replace("~0", "~")
        if not isinstance(node, dict) or token not in node:
            raise ValidationCaseSchemaError(
                f"{path}: unresolved schema reference {reference!r}"
            )
        node = node[token]
    return node


def _validate_node(root, schema, value, path):
    if not isinstance(schema, dict):
        raise ValidationCaseSchemaError(f"{path}: schema node is not an object")

    unsupported = sorted(set(schema) - _SUPPORTED_SCHEMA_KEYS)
    if unsupported:
        raise ValidationCaseSchemaError(
            f"{path}: validator does not support schema keyword(s) {unsupported}"
        )

    if "$ref" in schema:
        _validate_node(root, _resolve_ref(root, schema["$ref"], path), value, path)

    branches = schema.get("oneOf")
    if branches is not None:
        if not isinstance(branches, list):
            raise ValidationCaseSchemaError(f"{path}: oneOf must be an array")
        matched = 0
        branch_errors = []
        for branch in branches:
            try:
                _validate_node(root, branch, value, path)
                matched += 1
            except ValidationCaseSchemaError as exc:
                branch_errors.append(str(exc))
        if matched != 1:
            detail = "; ".join(branch_errors[:3])
            raise ValidationCaseSchemaError(
                f"{path}: oneOf expected exactly one matching branch, got {matched}"
                + (f" ({detail})" if detail else "")
            )

    if "const" in schema and not _json_equal(schema["const"], value):
        raise ValidationCaseSchemaError(
            f"{path}: expected constant {schema['const']!r}, got {value!r}"
        )

    if "enum" in schema:
        allowed = schema["enum"]
        if not any(_json_equal(candidate, value) for candidate in allowed):
            raise ValidationCaseSchemaError(
                f"{path}: value {value!r} is not one of {allowed!r}"
            )

    expected_type = schema.get("type")
    if expected_type is not None and not _type_matches(expected_type, value):
        raise ValidationCaseSchemaError(
            f"{path}: expected {expected_type}, got {_json_type_name(value)}"
        )

    if isinstance(value, dict):
        required = schema.get("required", [])
        for key in required:
            if key not in value:
                raise ValidationCaseSchemaError(
                    f"{path}.{key}: missing required property"
                )
        properties = schema.get("properties", {})
        if not isinstance(properties, dict):
            raise ValidationCaseSchemaError(f"{path}: properties must be an object")
        if schema.get("additionalProperties") is False:
            unknown = sorted(set(value) - set(properties))
            if unknown:
                raise ValidationCaseSchemaError(
                    f"{path}.{unknown[0]}: unknown property"
                )
        for key, child in value.items():
            child_schema = properties.get(key)
            if child_schema is not None:
                _validate_node(root, child_schema, child, f"{path}.{key}")

    if isinstance(value, list):
        if "minItems" in schema and len(value) < schema["minItems"]:
            raise ValidationCaseSchemaError(
                f"{path}: expected at least {schema['minItems']} item(s), got {len(value)}"
            )
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            raise ValidationCaseSchemaError(
                f"{path}: expected at most {schema['maxItems']} item(s), got {len(value)}"
            )
        if "items" in schema:
            for index, child in enumerate(value):
                _validate_node(root, schema["items"], child, f"{path}[{index}]")

    if isinstance(value, str):
        if "minLength" in schema and len(value) < schema["minLength"]:
            raise ValidationCaseSchemaError(
                f"{path}: string shorter than minLength={schema['minLength']}"
            )
        if "maxLength" in schema and len(value) > schema["maxLength"]:
            raise ValidationCaseSchemaError(
                f"{path}: string longer than maxLength={schema['maxLength']}"
            )
        if "pattern" in schema and re.search(schema["pattern"], value) is None:
            raise ValidationCaseSchemaError(
                f"{path}: value {value!r} does not match pattern {schema['pattern']!r}"
            )

    is_number = isinstance(value, (int, float)) and not isinstance(value, bool)
    if is_number:
        if isinstance(value, float) and not math.isfinite(value):
            raise ValidationCaseSchemaError(f"{path}: number must be finite")
        for keyword, relation in (
            ("minimum", lambda x, bound: x >= bound),
            ("maximum", lambda x, bound: x <= bound),
            ("exclusiveMinimum", lambda x, bound: x > bound),
            ("exclusiveMaximum", lambda x, bound: x < bound),
        ):
            if keyword in schema and not relation(value, schema[keyword]):
                raise ValidationCaseSchemaError(
                    f"{path}: {value!r} violates {keyword}={schema[keyword]!r}"
                )
        if "multipleOf" in schema:
            multiple = schema["multipleOf"]
            if not isinstance(multiple, (int, float)) or isinstance(multiple, bool) or multiple <= 0:
                raise ValidationCaseSchemaError(
                    f"{path}: invalid schema multipleOf={multiple!r}"
                )
            quotient = value / multiple
            if not math.isclose(quotient, round(quotient), rel_tol=0.0, abs_tol=1e-12):
                raise ValidationCaseSchemaError(
                    f"{path}: {value!r} is not a multiple of {multiple!r}"
                )


def validate_case_document(case, *, source=None, schema_path=DEFAULT_SCHEMA):
    """Validate one parsed JSON case against the complete canonical v2 schema.

    Returns the input object unchanged. Raises ValidationCaseSchemaError with a
    field path on any mismatch.
    """
    schema_path = Path(schema_path)
    schema = _load_schema(str(schema_path.resolve()))
    try:
        _validate_node(schema, schema, case, "$")
    except ValidationCaseSchemaError as exc:
        if source is None:
            raise
        raise ValidationCaseSchemaError(f"{source}: {exc}") from None
    return case
