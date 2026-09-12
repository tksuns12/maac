#!/usr/bin/env python3
"""Check fixed L1 identity/edit vectors without normalizing or editing MaaC.

The stdlib-only checker proves corpus byte/digest self-consistency, validates
supplied expected normalization vectors, checks protocol wire shapes, and
checks declared snapshot/request relationships. It does not parse MaaC source,
run a normalizer, apply patches, or prove production normalizer/editor
conformance.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import re
from pathlib import Path, PurePosixPath
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CORPUS = ROOT / "conformance" / "l1"
IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
INTEGER = re.compile(r"^(0|-?[1-9][0-9]*)$")
POSITIVE_INTEGER = re.compile(r"^[1-9][0-9]*$")
REVISION = re.compile(r"^sha256:[0-9a-f]{64}$")
UNITS = {"q", "s", "ms", "frame", "Hz", "kHz", "bpm", "ct", "dB"}
REVISION_AUTHORITY = "maac.revision.authored.sha256/1"


class CorpusError(ValueError):
    """A deterministic corpus contract failure."""


def fail(message: str) -> None:
    raise CorpusError(message)


def _json_string(value: str) -> str:
    pieces = ['"']
    for character in value:
        codepoint = ord(character)
        if 0xD800 <= codepoint <= 0xDFFF:
            fail("canonical JSON forbids surrogate code points")
        if codepoint <= 0x1F:
            pieces.append(f"\\u{codepoint:04x}")
        elif character == '"':
            pieces.append('\\"')
        elif character == "\\":
            pieces.append("\\\\")
        else:
            pieces.append(character)
    pieces.append('"')
    return "".join(pieces)


def canonical_bytes(value: Any) -> bytes:
    """Encode the canonical JSON data model used by the fixed L1 vectors."""

    def encode(current: Any) -> str:
        if current is None:
            return "null"
        if current is True:
            return "true"
        if current is False:
            return "false"
        if isinstance(current, int):
            return str(current)
        if isinstance(current, float):
            fail("canonical identity fixtures forbid JSON floating-point numbers")
        if isinstance(current, str):
            return _json_string(current)
        if isinstance(current, list):
            return "[" + ",".join(encode(item) for item in current) + "]"
        if isinstance(current, dict):
            if not all(isinstance(key, str) for key in current):
                fail("canonical JSON object keys must be strings")
            return "{" + ",".join(
                _json_string(key) + ":" + encode(current[key])
                for key in sorted(current)
            ) + "}"
        fail(f"unsupported canonical JSON value: {type(current).__name__}")

    return encode(value).encode("utf-8")


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def parse_json(raw: bytes, path: Path) -> Any:
    if raw.startswith(b"\xef\xbb\xbf"):
        fail(f"{path}: UTF-8 BOM is forbidden")
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"{path}: invalid UTF-8: {error}")
    try:
        return json.loads(text, object_pairs_hook=_unique_object)
    except (json.JSONDecodeError, CorpusError) as error:
        fail(f"{path}: malformed JSON: {error}")


def require_exact_keys(value: dict[str, Any], expected: set[str], context: str) -> None:
    if set(value) != expected:
        fail(f"{context}: expected keys {sorted(expected)}, received {sorted(value)}")


def require_string(value: Any, context: str) -> str:
    if not isinstance(value, str):
        fail(f"{context}: expected a string")
    return value


def validate_identifier(value: Any, context: str) -> None:
    if not isinstance(value, str) or len(value) > 128 or IDENTIFIER.fullmatch(value) is None:
        fail(f"{context}: invalid identifier")


def validate_path(value: Any, context: str, *, allow_empty: bool = False) -> None:
    if not isinstance(value, list) or (not allow_empty and not value):
        fail(f"{context}: invalid path array")
    for index, component in enumerate(value):
        validate_identifier(component, f"{context}[{index}]")


def validate_rational(value: dict[str, Any], context: str) -> None:
    numerator = value.get("n")
    denominator = value.get("d")
    if not isinstance(numerator, str) or INTEGER.fullmatch(numerator) is None:
        fail(f"{context}.n: noncanonical integer")
    if not isinstance(denominator, str) or POSITIVE_INTEGER.fullmatch(denominator) is None:
        fail(f"{context}.d: noncanonical positive integer")
    if math.gcd(abs(int(numerator)), int(denominator)) != 1:
        fail(f"{context}: rational is not reduced")


def validate_value(value: Any, context: str) -> None:
    if not isinstance(value, dict) or not isinstance(value.get("t"), str):
        fail(f"{context}: typed value must be an object with tag t")
    tag = value["t"]
    if tag == "number":
        require_exact_keys(value, {"t", "n", "d"}, context)
        validate_rational(value, context)
    elif tag == "quantity":
        require_exact_keys(value, {"t", "n", "d", "u"}, context)
        validate_rational(value, context)
        if not isinstance(value["u"], str) or value["u"] not in UNITS:
            fail(f"{context}.u: unsupported typed-tree unit")
    elif tag in {"string", "symbol"}:
        require_exact_keys(value, {"t", "v"}, context)
        if not isinstance(value["v"], str):
            fail(f"{context}.v: value must be a string")
    elif tag == "boolean":
        require_exact_keys(value, {"t", "v"}, context)
        if not isinstance(value["v"], bool):
            fail(f"{context}.v: value must be a boolean")
    elif tag == "ref":
        require_exact_keys(value, {"t", "path", "port"}, context)
        validate_path(value["path"], f"{context}.path")
        if value["port"] is not None:
            validate_identifier(value["port"], f"{context}.port")
    elif tag == "call":
        require_exact_keys(value, {"t", "fn", "args"}, context)
        validate_identifier(value["fn"], f"{context}.fn")
        if not isinstance(value["args"], list):
            fail(f"{context}.args: call arguments must be a list")
        for index, item in enumerate(value["args"]):
            validate_value(item, f"{context}.args[{index}]")
    elif tag in {"list", "tuple"}:
        require_exact_keys(value, {"t", "items"}, context)
        if not isinstance(value["items"], list):
            fail(f"{context}.items: items must be a list")
        if tag == "tuple" and len(value["items"]) < 2:
            fail(f"{context}.items: tuple must have at least two items")
        for index, item in enumerate(value["items"]):
            validate_value(item, f"{context}.items[{index}]")
    elif tag == "record":
        require_exact_keys(value, {"t", "fields"}, context)
        if not isinstance(value["fields"], dict):
            fail(f"{context}.fields: record fields must be an object")
        for name, item in value["fields"].items():
            validate_identifier(name, f"{context}.fields key")
            validate_value(item, f"{context}.fields.{name}")
    else:
        fail(f"{context}: unsupported typed value tag {tag!r}")


def validate_typed_object(value: Any, context: str) -> None:
    if not isinstance(value, dict):
        fail(f"{context}: typed object must be an object")
    require_exact_keys(value, {"kind", "fields", "children"}, context)
    validate_identifier(value["kind"], f"{context}.kind")
    if not isinstance(value["fields"], dict) or not isinstance(value["children"], dict):
        fail(f"{context}: fields and children must be objects")
    for name, item in value["fields"].items():
        validate_identifier(name, f"{context}.fields key")
        validate_value(item, f"{context}.fields.{name}")
    for name, child in value["children"].items():
        validate_identifier(name, f"{context}.children key")
        validate_typed_object(child, f"{context}.children.{name}")


def validate_typed_document(value: Any, context: str) -> None:
    if not isinstance(value, dict):
        fail(f"{context}: typed document must be an object")
    require_exact_keys(value, {"version", "objects"}, context)
    if value["version"] != 1 or isinstance(value["version"], bool):
        fail(f"{context}.version: expected integer 1")
    if not isinstance(value["objects"], dict):
        fail(f"{context}.objects: expected an object")
    for name, item in value["objects"].items():
        validate_identifier(name, f"{context}.objects key")
        validate_typed_object(item, f"{context}.objects.{name}")


def validate_patch_request(value: Any, context: str) -> None:
    if not isinstance(value, dict):
        fail(f"{context}: patch request must be an object")
    require_exact_keys(value, {"version", "base_revision", "operations"}, context)
    if not isinstance(value["version"], int) or isinstance(value["version"], bool) or value["version"] not in {1, 2}:
        fail(f"{context}.version: expected integer 1 or 2")
    if not isinstance(value["base_revision"], str) or REVISION.fullmatch(value["base_revision"]) is None:
        fail(f"{context}.base_revision: invalid revision")
    operations = value["operations"]
    if not isinstance(operations, list) or not operations:
        fail(f"{context}.operations: expected a nonempty array")
    for index, operation in enumerate(operations):
        op_context = f"{context}.operations[{index}]"
        if not isinstance(operation, dict) or not isinstance(operation.get("op"), str):
            fail(f"{op_context}: operation must be an object with op")
        op = operation["op"]
        if op == "set":
            allowed = {"op", "object", "field", "value", "expect", "expect_absent"}
            required = {"op", "object", "field", "value"}
            if not required <= set(operation) or not set(operation) <= allowed:
                fail(f"{op_context}: invalid set fields")
            if "expect" in operation and "expect_absent" in operation:
                fail(f"{op_context}: expect and expect_absent are mutually exclusive")
            if "expect_absent" in operation and operation["expect_absent"] is not True:
                fail(f"{op_context}.expect_absent: expected true")
            validate_path(operation["object"], f"{op_context}.object")
            validate_path(operation["field"], f"{op_context}.field")
            validate_value(operation["value"], f"{op_context}.value")
            if "expect" in operation:
                validate_value(operation["expect"], f"{op_context}.expect")
        elif op == "unset":
            allowed = {"op", "object", "field", "expect"}
            required = {"op", "object", "field"}
            if not required <= set(operation) or not set(operation) <= allowed:
                fail(f"{op_context}: invalid unset fields")
            validate_path(operation["object"], f"{op_context}.object")
            validate_path(operation["field"], f"{op_context}.field")
            if "expect" in operation:
                validate_value(operation["expect"], f"{op_context}.expect")
        elif op == "insert_object":
            require_exact_keys(operation, {"op", "parent", "id", "object_value"}, op_context)
            validate_path(operation["parent"], f"{op_context}.parent", allow_empty=True)
            validate_identifier(operation["id"], f"{op_context}.id")
            validate_typed_object(operation["object_value"], f"{op_context}.object_value")
        elif op == "delete_object":
            allowed = {"op", "object", "expect_object"}
            required = {"op", "object"}
            if not required <= set(operation) or not set(operation) <= allowed:
                fail(f"{op_context}: invalid delete_object fields")
            validate_path(operation["object"], f"{op_context}.object")
            if "expect_object" in operation:
                validate_typed_object(operation["expect_object"], f"{op_context}.expect_object")
        elif op == "rename_id":
            require_exact_keys(operation, {"op", "object", "new_id"}, op_context)
            validate_path(operation["object"], f"{op_context}.object")
            validate_identifier(operation["new_id"], f"{op_context}.new_id")
        else:
            fail(f"{op_context}: unsupported operation {op!r}")


def execution_object_projection(value: dict[str, Any]) -> dict[str, Any]:
    """Remove only typed Object.fields.label and recurse through children."""
    validate_typed_object(value, "projection input")
    return {
        "kind": value["kind"],
        "fields": {name: copy.deepcopy(field) for name, field in value["fields"].items() if name != "label"},
        "children": {name: execution_object_projection(child) for name, child in value["children"].items()},
    }


def execution_document_projection(value: dict[str, Any]) -> dict[str, Any]:
    validate_typed_document(value, "projection input")
    return {"version": value["version"], "objects": {name: execution_object_projection(item) for name, item in value["objects"].items()}}


def sha256(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def revision(raw: bytes) -> str:
    return "sha256:" + sha256(raw)


def _at(value: Any, path: list[Any], context: str) -> Any:
    current = value
    for component in path:
        if isinstance(component, str) and isinstance(current, dict) and component in current:
            current = current[component]
        elif isinstance(component, int) and isinstance(current, list) and 0 <= component < len(current):
            current = current[component]
        else:
            fail(f"{context}: missing path component {component!r}")
    return current


def _check_expected_normalized_documents(loaded: dict[str, Any]) -> None:
    omit = loaded["authored/minimal-tail-omitted.typed.json"]
    minimal = loaded["normalized/minimal-default.execution.json"]
    if set(minimal["objects"]) != {"clock", "metre", "p"}:
        fail("minimal expected N(A): unexpected object set")
    if minimal["objects"].get("clock") != omit["objects"].get("clock") or minimal["objects"].get("metre") != omit["objects"].get("metre"):
        fail("minimal expected N(A): tempo or meter changed")
    project_fields = _at(minimal, ["objects", "p", "fields"], "minimal expected N(A)")
    if set(project_fields) != {"meter", "rate", "requires", "score", "seed", "tail", "tempo"}:
        fail("minimal expected N(A): project defaults or fields are incomplete")
    for field in {"meter", "rate", "score", "tempo"}:
        if project_fields[field] != _at(omit, ["objects", "p", "fields", field], "minimal authored document"):
            fail(f"minimal expected N(A): authored project field {field} changed")
    expected_defaults = {
        "requires": {"items": [], "t": "list"},
        "seed": {"d": "1", "n": "0", "t": "number"},
        "tail": {"d": "1", "n": "0", "t": "quantity", "u": "s"},
    }
    if {field: project_fields[field] for field in expected_defaults} != expected_defaults:
        fail("minimal expected N(A): project default values mismatch")
    if "output" in project_fields:
        fail("minimal expected N(A): Document-only output must remain absent")

    for suffix in ("a", "b"):
        authored = loaded[f"authored/minimal-label-{suffix}.typed.json"]
        normalized = loaded[f"normalized/minimal-label-{suffix}.normalized.json"]
        if execution_document_projection(normalized) != minimal:
            fail(f"label {suffix} expected N(A): execution projection mismatch")
        for object_id in ("clock", "p"):
            path = ["objects", object_id, "fields", "label"]
            if _at(normalized, path, f"label {suffix} expected N(A)") != _at(authored, path, f"label {suffix} authored"):
                fail(f"label {suffix} expected N(A): authored label changed")

    unit_authored = loaded["authored/minimal-tail-1-50s.typed.json"]
    unit_expected = loaded["normalized/minimal-tail-1-50.execution.json"]
    tail_path = ["objects", "p", "fields", "tail"]
    if _at(unit_expected, tail_path, "unit expected N(A)") != _at(unit_authored, tail_path, "unit authored document"):
        fail("unit expected N(A): canonical 1/50s value mismatch")
    for object_id in ("clock", "metre"):
        if unit_expected["objects"].get(object_id) != unit_authored["objects"].get(object_id):
            fail("unit expected N(A): non-project object changed")

    constant_authored = loaded["authored/constant-omitted.typed.json"]
    constant_expected = loaded["normalized/constant-default.execution.json"]
    if set(constant_expected["objects"]) != {"clock", "constant", "metre", "p"}:
        fail("constant expected N(A): unexpected object set")
    expected_constant_fields = {
        "config": {"fields": {}, "t": "record"},
        "params": {"fields": {"value": {"d": "1", "n": "0", "t": "number"}}, "t": "record"},
        "type": {"t": "string", "v": "core.constant/1"},
    }
    if _at(constant_expected, ["objects", "constant", "fields"], "constant expected N(A)") != expected_constant_fields:
        fail("constant expected N(A): config/params defaults mismatch")
    if _at(constant_expected, ["objects", "constant", "children"], "constant expected N(A)") != _at(constant_authored, ["objects", "constant", "children"], "constant authored document"):
        fail("constant expected N(A): children changed")


def _check_position_resolution_vectors(manifest: dict[str, Any], loaded: dict[str, Any]) -> int:
    vectors = manifest.get("position_resolution_vectors")
    if not isinstance(vectors, list) or len(vectors) != 2:
        fail("manifest.position_resolution_vectors: expected the two fixed meter contexts")
    expected_ids = {"bar_positions_in_4_4", "bar_positions_in_3_4"}
    seen: set[str] = set()
    for vector in vectors:
        if not isinstance(vector, dict):
            fail("manifest.position_resolution_vectors: malformed vector")
        require_exact_keys(vector, {"authored", "claim", "expected_normalized", "id", "resolved_bar_span", "resolved_explicit_span"}, "position resolution vector")
        vector_id = require_string(vector["id"], "position resolution vector id")
        if vector_id in seen or vector_id not in expected_ids:
            fail(f"position resolution vector {vector_id}: duplicate or unsupported id")
        seen.add(vector_id)
        if vector["claim"] != "supplied expected full N(A), not output from this checker":
            fail(f"position resolution vector {vector_id}: evidence claim mismatch")
        authored_name = require_string(vector["authored"], f"position resolution vector {vector_id}.authored")
        normalized_name = require_string(vector["expected_normalized"], f"position resolution vector {vector_id}.expected_normalized")
        if authored_name not in loaded or normalized_name not in loaded:
            fail(f"position resolution vector {vector_id}: unknown file")
        authored, normalized = loaded[authored_name], loaded[normalized_name]
        bar_authored = _at(authored, ["objects", "bar_region", "fields", "span"], vector_id)
        explicit_authored = _at(authored, ["objects", "explicit_region", "fields", "span"], vector_id)
        bar_normalized = _at(normalized, ["objects", "bar_region", "fields", "span"], vector_id)
        explicit_normalized = _at(normalized, ["objects", "explicit_region", "fields", "span"], vector_id)
        if bar_normalized != vector["resolved_bar_span"] or explicit_normalized != vector["resolved_explicit_span"]:
            fail(f"position resolution vector {vector_id}: supplied resolved span mismatch")
        if explicit_normalized != explicit_authored:
            fail(f"position resolution vector {vector_id}: explicit q span changed")
        if _at(authored, ["objects", "bar_region", "kind"], vector_id) != "region" or _at(authored, ["objects", "explicit_region", "kind"], vector_id) != "region":
            fail(f"position resolution vector {vector_id}: bar calls must use top-level regions")
        if not all(isinstance(item, dict) and item.get("t") == "call" and item.get("fn") == "bar" for item in _at(bar_authored, ["items"], vector_id)):
            fail(f"position resolution vector {vector_id}: authored bar constructors missing")
        if set(normalized["objects"]) != set(authored["objects"]):
            fail(f"position resolution vector {vector_id}: object set changed")
        for object_id in ("bar_region", "clock", "explicit_region", "metre"):
            if object_id != "bar_region" and normalized["objects"].get(object_id) != authored["objects"].get(object_id):
                fail(f"position resolution vector {vector_id}: unrelated object changed")
        authored_project = _at(authored, ["objects", "p", "fields"], vector_id)
        normalized_project = _at(normalized, ["objects", "p", "fields"], vector_id)
        if set(normalized_project) != {"meter", "rate", "requires", "score", "seed", "tail", "tempo"}:
            fail(f"position resolution vector {vector_id}: project fields/defaults mismatch")
        for field in ("meter", "rate", "score", "tempo"):
            if normalized_project.get(field) != authored_project.get(field):
                fail(f"position resolution vector {vector_id}: project field {field} changed")
        if {name: normalized_project.get(name) for name in ("requires", "seed", "tail")} != {
            "requires": {"items": [], "t": "list"},
            "seed": {"d": "1", "n": "0", "t": "number"},
            "tail": {"d": "1", "n": "0", "t": "quantity", "u": "s"},
        }:
            fail(f"position resolution vector {vector_id}: project defaults mismatch")
    if seen != expected_ids:
        fail("manifest.position_resolution_vectors: fixed vector inventory mismatch")
    base = next(item for item in vectors if item["id"] == "bar_positions_in_4_4")
    after = next(item for item in vectors if item["id"] == "bar_positions_in_3_4")
    if base["resolved_bar_span"] == after["resolved_bar_span"] or base["resolved_explicit_span"] != after["resolved_explicit_span"]:
        fail("position resolution vectors: meter-relative and explicit controls mismatch")
    return len(vectors)


CASE_CONTRACTS: dict[str, dict[str, Any]] = {
    "set_tail_zero_and_inverse": {"status": "success", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/set-tail-zero.request.json", "after": "authored/minimal-tail-zero.typed.json", "inverse": "patches/unset-tail-zero.inverse.request.json", "restored": "authored/minimal-tail-omitted.typed.json", "error": None, "relation": "matches_base", "checks": ["authored_presence", "inverse_exact"]},
    "expect_absent_conflicts_with_authored_zero": {"status": "failure", "base": "authored/minimal-tail-zero.typed.json", "current": "authored/minimal-tail-zero.typed.json", "request": "patches/expect-absent-conflict.request.json", "error": "E_CONFLICT", "relation": "matches_base", "checks": ["authored_presence", "atomic_failure"]},
    "stale_revision_conflicts_despite_equal_execution": {"status": "failure", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-zero.typed.json", "request": "patches/stale-base-equal-execution.request.json", "error": "E_CONFLICT", "relation": "stale_against_current", "checks": ["equal_execution_distinct_revision", "atomic_failure"]},
    "expect_effective_default_on_omitted_tail": {"status": "success", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/expect-default-tail.request.json", "after": "authored/minimal-tail-one.typed.json", "inverse": "patches/unset-tail-one.inverse.request.json", "restored": "authored/minimal-tail-omitted.typed.json", "error": None, "relation": "matches_base", "checks": ["effective_default_expect", "inverse_exact"]},
    "expect_absent_field_without_default_conflicts": {"status": "failure", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/expect-missing-no-default-label.request.json", "error": "E_CONFLICT", "relation": "matches_base", "checks": ["absent_without_default", "atomic_failure"]},
    "unset_missing_authored_tail_references_nothing": {"status": "failure", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/unset-missing-tail.request.json", "error": "E_REFERENCE", "relation": "matches_base", "checks": ["unset_requires_authored_field", "atomic_failure"]},
    "successive_writes_compare_against_fixed_base": {"status": "success", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/fixed-base-successive-tail.request.json", "after": "authored/minimal-tail-two.typed.json", "inverse": "patches/unset-tail-two.inverse.request.json", "restored": "authored/minimal-tail-omitted.typed.json", "error": None, "relation": "matches_base", "checks": ["fixed_base_expectations", "inverse_exact"]},
    "equivalent_unit_expect_preserves_authored_unit": {"status": "success", "base": "authored/minimal-tail-20ms.typed.json", "current": "authored/minimal-tail-20ms.typed.json", "request": "patches/unit-equivalent-expect.request.json", "after": "authored/minimal-tail-one.typed.json", "inverse": "patches/restore-tail-20ms.inverse.request.json", "restored": "authored/minimal-tail-20ms.typed.json", "error": None, "relation": "matches_base", "checks": ["semantic_unit_expect", "inverse_exact_unit"]},
    "rename_then_edit_uses_base_expect_and_candidate_path": {"status": "success", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/rename-tempo-and-set.request.json", "after": "authored/tempo-renamed-90.typed.json", "inverse": "patches/restore-renamed-tempo.inverse.request.json", "restored": "authored/minimal-tail-omitted.typed.json", "error": None, "relation": "matches_base", "checks": ["rename_reference_rewrite", "fixed_base_expectations", "candidate_target_paths", "inverse_exact"]},
    "inserted_object_has_no_base_correspondence": {"status": "failure", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/insert-new-object-precondition.request.json", "error": "E_CONFLICT", "relation": "matches_base", "checks": ["insert_loses_base_correspondence", "atomic_failure"]},
    "delete_reinsert_same_id_loses_base_correspondence": {"status": "failure", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/delete-reinsert-tempo-precondition.request.json", "error": "E_CONFLICT", "relation": "matches_base", "checks": ["reinsert_loses_base_correspondence", "atomic_failure"]},
    "missing_record_descendant_write_does_not_synthesize": {"status": "failure", "base": "authored/constant-omitted.typed.json", "current": "authored/constant-omitted.typed.json", "request": "patches/set-missing-params-descendant.request.json", "error": "E_REFERENCE", "relation": "matches_base", "checks": ["missing_candidate_intermediate", "atomic_failure"]},
    "nonrecord_intermediate_is_invalid_path": {"status": "failure", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/set-nonrecord-intermediate.request.json", "error": "E_REFERENCE", "relation": "matches_base", "checks": ["nonrecord_candidate_intermediate", "atomic_failure"]},
    "explicit_record_creation_allows_descendant_write": {"status": "success", "base": "authored/constant-omitted.typed.json", "current": "authored/constant-omitted.typed.json", "request": "patches/create-params-record.request.json", "after": "authored/constant-params-zero.typed.json", "inverse": "patches/unset-params-record.inverse.request.json", "restored": "authored/constant-omitted.typed.json", "error": None, "relation": "matches_base", "checks": ["explicit_record_creation", "fixed_base_expectations", "inverse_exact"]},
    "meter_edit_preserves_bar_constructor_and_explicit_q": {"status": "success", "base": "authored/bar-meter-base.typed.json", "current": "authored/bar-meter-base.typed.json", "request": "patches/edit-meter-preserve-bar.request.json", "after": "authored/bar-meter-after.typed.json", "inverse": "patches/restore-meter.inverse.request.json", "restored": "authored/bar-meter-base.typed.json", "error": None, "relation": "matches_base", "checks": ["bar_constructor_preserved", "explicit_q_preserved", "inverse_exact"]},
    "negative_tail_semantic_failure_is_atomic": {"status": "failure", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/negative-tail-atomic.request.json", "error": "E_RANGE", "relation": "matches_base", "checks": ["semantic_validation", "atomic_failure"]},
    "unsupported_v1_precedes_stale_base": {"status": "failure", "base": "authored/minimal-tail-omitted.typed.json", "current": "authored/minimal-tail-omitted.typed.json", "request": "patches/unsupported-v1.request.json", "error": "E_CAPABILITY", "relation": "invalid_for_capability_precedence", "checks": ["version_precedes_base", "atomic_failure"]},
    "delete_unreferenced_labelled_constant_with_normalized_expect_object": {"status": "success", "base": "authored/constant-labelled.typed.json", "current": "authored/constant-labelled.typed.json", "request": "patches/delete-labelled-constant.request.json", "after": "authored/minimal-tail-omitted.typed.json", "inverse": "patches/restore-labelled-constant.inverse.request.json", "restored": "authored/constant-labelled.typed.json", "error": None, "relation": "matches_base", "checks": ["expect_object_normalized_subtree_includes_label", "inverse_exact_authored_object"]},
    "delete_expect_object_label_mismatch_conflicts": {"status": "failure", "base": "authored/constant-labelled.typed.json", "current": "authored/constant-labelled.typed.json", "request": "patches/delete-labelled-constant-label-conflict.request.json", "error": "E_CONFLICT", "relation": "matches_base", "checks": ["expect_object_includes_label", "atomic_failure"]},
    "pitch_semantic_expect_writes_key_constructor": {"status": "success", "base": "authored/pitch-symbol.typed.json", "current": "authored/pitch-symbol.typed.json", "request": "patches/set-pitch-key-60.request.json", "after": "authored/pitch-key-60.typed.json", "inverse": "patches/restore-pitch-symbol.inverse.request.json", "restored": "authored/pitch-symbol.typed.json", "error": None, "relation": "matches_base", "checks": ["semantic_pitch_expect", "authored_constructor_preserved", "inverse_exact"]},
}

REQUEST_DIGESTS = {
    "patches/create-params-record.request.json": "6e0e2dd104566bab6965f9e9c3742e7631ee5731bb99d8f7f757e12ae7c99251",
    "patches/delete-labelled-constant-label-conflict.request.json": "3d3b130e3c60fb6e0c52b376ec12b8130169da92bb3b71fed6900dbf050d293b",
    "patches/delete-labelled-constant.request.json": "61e9727b26322be3d7afee2fec4b7b55ee166017655a0fc0177f17bbf68b5a1b",
    "patches/delete-reinsert-tempo-precondition.request.json": "f7a2754d3dd62e88d3d564ab5fe3e774cb71e5c8a2f419182ed7a437d1bd84aa",
    "patches/edit-meter-preserve-bar.request.json": "2c6adca3955d6d689b65cfb10941da420d43efbaac793c7f4f53abe737b3b93c",
    "patches/expect-absent-conflict.request.json": "c2056f757123e507488a5b55ab34543be993c8a4cbdcabdab60caeb905d48898",
    "patches/expect-default-tail.request.json": "41305c8afd0da46353cdcce0d6cf76f31229988c1eed3c6974677179d9f8a099",
    "patches/expect-missing-no-default-label.request.json": "a84807d250d772993e3f92e82e8e08063140e78581062d57bf8c30c69c9126df",
    "patches/fixed-base-successive-tail.request.json": "398e9c0af65463be06c4c0033bbcb0a7ef03a455300a833c5c2b50a17df065e2",
    "patches/insert-new-object-precondition.request.json": "b6016d680c57951a325348cab9810c6f61355ec58672fca830d888b583c03056",
    "patches/negative-tail-atomic.request.json": "0a259e2dc23918d8bbf620f7153aae391552f51a155e3a18215306dd4a400162",
    "patches/rename-tempo-and-set.request.json": "5e4b6f9814beb60fe2df78807d2f0275f4db626ebffbf668c249bf0d520968a3",
    "patches/restore-labelled-constant.inverse.request.json": "59d88fc0beded68245dacf10f209c9d67e3ccae55bb08886b2d0ebc85fb1b83b",
    "patches/restore-meter.inverse.request.json": "645de51cd13f35da68dc1f7532ab6c47054f27decd0ae14a26aa7a04dfca2f4d",
    "patches/restore-pitch-symbol.inverse.request.json": "74fad49660576a8ed56bfe8c27da16384770bf38a65c57b382eebcb163f10a28",
    "patches/restore-renamed-tempo.inverse.request.json": "02b8e596bf49a00d9fcd9b12afaea071ad2433e7115d13c8b6a005900e2b97e5",
    "patches/restore-tail-20ms.inverse.request.json": "75c329ac70c1e34fa7d0cba437079eb3f72eb6fb912d77f70e091bef7be1bc3e",
    "patches/set-missing-params-descendant.request.json": "d75d4d2a75d66dbb583a1fe5070b245a22dedd83bfc1c5d8ba0d73976eb7617f",
    "patches/set-nonrecord-intermediate.request.json": "4088c5cff47e274f5cb76efc6825b8ecd1a1f64dc48d415b14185a971c5110c5",
    "patches/set-pitch-key-60.request.json": "e40c9e77d26ae09b8ccb6d35acc0c0a376cb5b503b3662f7731feba3100cb027",
    "patches/set-tail-zero.request.json": "f02be244504062b32bb1bee19ad1c6aee3a421f0d1db9ab114b912c8cfe4cbe6",
    "patches/stale-base-equal-execution.request.json": "3e170afb426d880a9c120fa3871285d1e1022a0b6cd335c8b2b3334711849505",
    "patches/unit-equivalent-expect.request.json": "2f0d33f0fdc08c74e113780610963c6ab6192e888a9d7ff4ce7912cad1d9746e",
    "patches/unset-missing-tail.request.json": "c94a10227c5f6863aa548ddda0f779ff39d60dd966b2ef78c51cac1614f24fec",
    "patches/unset-params-record.inverse.request.json": "2fae3a83cded557ab317a2f39cd3524473c7d09d0d87e3d70888bcfb64bafe15",
    "patches/unset-tail-one.inverse.request.json": "cbd26c1051501ebc00868196a9cd28c64e02c92e63a9644f509581f73ea3f7e9",
    "patches/unset-tail-two.inverse.request.json": "3aaeea53300a2f28b3e13c404d49043f944024e9e87c5c293eafb92933f2ac29",
    "patches/unset-tail-zero.inverse.request.json": "f24e557a684913499cd9d9f35b5e4513b475814da2f5e307cbca030f417d6212",
    "patches/unsupported-v1.request.json": "1b0bccc273c0b2036c22c876cbb5a8706e4216c974c81f3a45d6707cc5e79f66",
}


def _require_operation_kinds(operations: list[Any], expected: list[str], case_id: str) -> None:
    actual = [operation.get("op") if isinstance(operation, dict) else None for operation in operations]
    if actual != expected:
        fail(f"patch case {case_id}: expected operation sequence {expected}, received {actual}")


def _check_case_trace(case_id: str, loaded: dict[str, Any]) -> None:
    contract = CASE_CONTRACTS[case_id]
    request = loaded[contract["request"]]
    operations = request["operations"]
    base = loaded[contract["base"]]
    if case_id == "successive_writes_compare_against_fixed_base":
        _require_operation_kinds(operations, ["set", "set"], case_id)
        if operations[0].get("expect_absent") is not True or operations[1].get("expect") != {"d": "1", "n": "0", "t": "quantity", "u": "s"}:
            fail(f"patch case {case_id}: fixed-base expectation trace mismatch")
    elif case_id == "equivalent_unit_expect_preserves_authored_unit":
        _require_operation_kinds(operations, ["set"], case_id)
        unit = _at(loaded["authored/minimal-tail-1-50s.typed.json"], ["objects", "p", "fields", "tail"], case_id)
        if operations[0].get("expect") != unit:
            fail(f"patch case {case_id}: semantic unit expectation mismatch")
        inverse_operations = loaded[contract["inverse"]]["operations"]
        _require_operation_kinds(inverse_operations, ["set"], case_id)
        if inverse_operations[0]["value"] != _at(base, ["objects", "p", "fields", "tail"], case_id):
            fail(f"patch case {case_id}: inverse does not restore authored unit")
    elif case_id == "rename_then_edit_uses_base_expect_and_candidate_path":
        after = loaded[contract["after"]]
        _require_operation_kinds(operations, ["rename_id", "set", "set"], case_id)
        if operations[0]["object"] != ["clock"] or operations[0]["new_id"] != "beat":
            fail(f"patch case {case_id}: rename trace mismatch")
        if operations[1].get("expect") != _at(base, ["objects", "p", "fields", "tempo"], case_id):
            fail(f"patch case {case_id}: reference expectation must retain base name")
        if operations[1]["value"] != _at(after, ["objects", "p", "fields", "tempo"], case_id):
            fail(f"patch case {case_id}: candidate reference value mismatch")
        if operations[2]["object"] != ["beat"] or operations[2].get("expect") != _at(base, ["objects", "clock", "fields", "points"], case_id):
            fail(f"patch case {case_id}: candidate path or base expectation mismatch")
        if operations[2]["value"] != _at(after, ["objects", "beat", "fields", "points"], case_id):
            fail(f"patch case {case_id}: renamed tempo value mismatch")
    elif case_id == "inserted_object_has_no_base_correspondence":
        _require_operation_kinds(operations, ["insert_object", "set"], case_id)
        if operations[0]["op"] != "insert_object" or operations[0]["id"] != "spare" or operations[1].get("expect") is None or operations[1]["object"] != ["spare"]:
            fail(f"patch case {case_id}: inserted-object trace mismatch")
    elif case_id == "delete_reinsert_same_id_loses_base_correspondence":
        _require_operation_kinds(operations, ["delete_object", "insert_object", "set"], case_id)
        if operations[2].get("expect") is None:
            fail(f"patch case {case_id}: reinserted-object trace mismatch")
    elif case_id == "explicit_record_creation_allows_descendant_write":
        _require_operation_kinds(operations, ["set", "set"], case_id)
        first, second = operations
        if first["field"] != ["params"] or first.get("expect_absent") is not True or first["value"] != {"fields": {}, "t": "record"} or second["field"] != ["params", "value"] or second.get("expect_absent") is not True:
            fail(f"patch case {case_id}: record creation/descendant trace mismatch")
    elif case_id == "meter_edit_preserves_bar_constructor_and_explicit_q":
        _require_operation_kinds(operations, ["set"], case_id)
        after = loaded[contract["after"]]
        bar_path = ["objects", "bar_region", "fields", "span"]
        explicit_path = ["objects", "explicit_region", "fields", "span"]
        bar_span = _at(base, bar_path, case_id)
        explicit_span = _at(base, explicit_path, case_id)
        if bar_span != _at(after, bar_path, case_id) or explicit_span != _at(after, explicit_path, case_id):
            fail(f"patch case {case_id}: authored region positions changed")
        if not all(item.get("t") == "call" and item.get("fn") == "bar" for item in bar_span["items"]) or explicit_span != {"items": [{"d": "1", "n": "4", "t": "quantity", "u": "q"}, {"d": "1", "n": "8", "t": "quantity", "u": "q"}], "t": "list"}:
            fail(f"patch case {case_id}: constructor controls mismatch")
    elif case_id == "unsupported_v1_precedes_stale_base":
        _require_operation_kinds(operations, ["set"], case_id)
        if request["version"] != 1 or request["base_revision"] == revision(canonical_bytes(base)):
            fail(f"patch case {case_id}: v1/stale-base precedence control mismatch")
    elif case_id in {
        "delete_unreferenced_labelled_constant_with_normalized_expect_object",
        "delete_expect_object_label_mismatch_conflicts",
    }:
        _require_operation_kinds(operations, ["delete_object"], case_id)
        expected_object = operations[0].get("expect_object")
        if operations[0].get("object") != ["constant"] or not isinstance(expected_object, dict):
            fail(f"patch case {case_id}: delete expectation trace mismatch")
        expected_fields = expected_object.get("fields")
        if not isinstance(expected_fields, dict) or set(expected_fields) != {"config", "label", "params", "type"}:
            fail(f"patch case {case_id}: normalized object expectation is incomplete")
        expected_label = "Reference constant" if case_id.startswith("delete_unreferenced") else "Changed label"
        if expected_fields["label"] != {"t": "string", "v": expected_label}:
            fail(f"patch case {case_id}: label expectation trace mismatch")
        if case_id.startswith("delete_unreferenced"):
            inverse_operations = loaded[contract["inverse"]]["operations"]
            _require_operation_kinds(inverse_operations, ["insert_object"], case_id)
            inverse_object = inverse_operations[0]["object_value"]
            authored_object = _at(base, ["objects", "constant"], case_id)
            if inverse_object != authored_object or set(inverse_object["fields"]) != {"label", "type"}:
                fail(f"patch case {case_id}: inverse must restore exact authored object")
    elif case_id == "pitch_semantic_expect_writes_key_constructor":
        _require_operation_kinds(operations, ["set"], case_id)
        after = loaded[contract["after"]]
        pitch_path = ["objects", "riff", "children", "n", "fields", "pitch"]
        key_60 = {"args": [{"d": "1", "n": "60", "t": "number"}], "fn": "key", "t": "call"}
        if operations[0].get("expect") != key_60 or operations[0].get("value") != key_60 or operations[0].get("object") != ["riff", "n"]:
            fail(f"patch case {case_id}: semantic pitch expectation trace mismatch")
        if _at(base, pitch_path, case_id) != {"t": "symbol", "v": "C4"} or _at(after, pitch_path, case_id) != key_60:
            fail(f"patch case {case_id}: authored pitch constructor snapshots mismatch")
        inverse_operations = loaded[contract["inverse"]]["operations"]
        _require_operation_kinds(inverse_operations, ["set"], case_id)
        inverse_value = inverse_operations[0]["value"]
        if inverse_value != {"t": "symbol", "v": "C4"}:
            fail(f"patch case {case_id}: inverse pitch constructor mismatch")


def _load_files(corpus: Path, manifest_path: Path, file_specs: Any) -> tuple[dict[str, Any], dict[str, bytes]]:
    if not isinstance(file_specs, dict) or not file_specs:
        fail("manifest.files: expected a nonempty object")
    actual = {path.relative_to(corpus).as_posix() for path in corpus.rglob("*.json") if path != manifest_path}
    if actual != set(file_specs):
        fail("manifest.files: listed files do not match the corpus JSON files")
    loaded: dict[str, Any] = {}
    raw_by_name: dict[str, bytes] = {}
    for relative, spec in file_specs.items():
        if not isinstance(relative, str) or not isinstance(spec, dict):
            fail("manifest.files: malformed entry")
        relative_path = PurePosixPath(relative)
        if relative_path.is_absolute() or ".." in relative_path.parts or relative_path.as_posix() != relative:
            fail(f"manifest.files: nonportable path {relative!r}")
        require_exact_keys(spec, {"bytes", "coverage", "role", "sha256", "shape"}, relative)
        if not isinstance(spec["bytes"], int) or isinstance(spec["bytes"], bool) or spec["bytes"] < 0:
            fail(f"{relative}: invalid byte length")
        if not isinstance(spec["sha256"], str) or re.fullmatch(r"[0-9a-f]{64}", spec["sha256"]) is None:
            fail(f"{relative}: invalid SHA-256 manifest value")
        require_string(spec["coverage"], f"{relative}.coverage")
        require_string(spec["role"], f"{relative}.role")
        path = corpus.joinpath(*relative_path.parts)
        raw = path.read_bytes()
        value = parse_json(raw, path)
        if raw != canonical_bytes(value):
            fail(f"{relative}: bytes are not canonical JSON with no final newline")
        if len(raw) != spec["bytes"]:
            fail(f"{relative}: byte length mismatch")
        if sha256(raw) != spec["sha256"]:
            fail(f"{relative}: SHA-256 mismatch")
        shape = spec.get("shape")
        if shape == "document":
            validate_typed_document(value, relative)
        elif shape == "object_fragment":
            validate_typed_object(value, relative)
            if spec["coverage"] != "typed_object_fragment_context":
                fail(f"{relative}: fragment coverage must be explicit")
        elif shape == "patch_request":
            validate_patch_request(value, relative)
        elif shape != "canonical_value":
            fail(f"{relative}: unsupported fixture shape {shape!r}")
        loaded[relative] = value
        raw_by_name[relative] = raw
    patch_files = {name for name, spec in file_specs.items() if isinstance(spec, dict) and spec.get("shape") == "patch_request"}
    if patch_files != set(REQUEST_DIGESTS):
        fail("fixed request inventory mismatch")
    for relative, expected_digest in REQUEST_DIGESTS.items():
        if sha256(raw_by_name[relative]) != expected_digest:
            fail(f"{relative}: fixed request SHA-256 mismatch")
    return loaded, raw_by_name


def _check_vectors(manifest: dict[str, Any], loaded: dict[str, Any], raw_by_name: dict[str, bytes]) -> int:
    _check_expected_normalized_documents(loaded)
    vectors = manifest.get("normalization_vectors")
    if not isinstance(vectors, list) or not vectors:
        fail("manifest.normalization_vectors: expected a nonempty list")
    vector_ids: set[str] = set()
    for vector in vectors:
        if not isinstance(vector, dict):
            fail("manifest.normalization_vectors: malformed vector")
        require_exact_keys(vector, {"claim", "expected_execution", "id", "members", "revision_relation"}, "normalization vector")
        vector_id = require_string(vector["id"], "normalization vector id")
        if vector_id in vector_ids:
            fail(f"normalization vector {vector_id}: duplicate id")
        vector_ids.add(vector_id)
        if vector["claim"] != "supplied expected vectors, not output from this checker" or vector["revision_relation"] != "all_distinct":
            fail(f"normalization vector {vector_id}: evidence claim/revision relation mismatch")
        expected_execution = require_string(vector["expected_execution"], f"normalization vector {vector_id}.expected_execution")
        if expected_execution not in loaded:
            fail(f"normalization vector {vector_id}: unknown execution vector")
        members = vector["members"]
        if not isinstance(members, list) or len(members) < 2:
            fail(f"normalization vector {vector_id}: expected at least two members")
        revisions: set[str] = set()
        for member in members:
            if not isinstance(member, dict):
                fail(f"normalization vector {vector_id}: malformed member")
            require_exact_keys(member, {"authored", "expected_normalized"}, f"normalization vector {vector_id} member")
            authored = require_string(member["authored"], f"normalization vector {vector_id} authored")
            normalized = require_string(member["expected_normalized"], f"normalization vector {vector_id} expected_normalized")
            if authored not in raw_by_name or normalized not in loaded:
                fail(f"normalization vector {vector_id}: unknown member file")
            revisions.add(revision(raw_by_name[authored]))
            if canonical_bytes(execution_document_projection(loaded[normalized])) != raw_by_name[expected_execution]:
                fail(f"normalization vector {vector_id}: supplied execution projection mismatch")
        if len(revisions) != len(members):
            fail(f"normalization vector {vector_id}: authored revisions must be distinct")
    return len(vectors)


def _check_projections(manifest: dict[str, Any], loaded: dict[str, Any], raw_by_name: dict[str, bytes]) -> int:
    relationships = manifest.get("projection_relationships")
    if not isinstance(relationships, list) or not relationships:
        fail("manifest.projection_relationships: expected a nonempty list")
    ids: set[str] = set()
    for relationship in relationships:
        if not isinstance(relationship, dict):
            fail("manifest.projection_relationships: malformed relationship")
        require_exact_keys(relationship, {"expected", "id", "source"}, "projection relationship")
        relationship_id = require_string(relationship["id"], "projection relationship id")
        source = require_string(relationship["source"], f"projection relationship {relationship_id}.source")
        expected = require_string(relationship["expected"], f"projection relationship {relationship_id}.expected")
        if relationship_id in ids:
            fail(f"projection relationship {relationship_id}: duplicate id")
        ids.add(relationship_id)
        if source not in loaded or expected not in raw_by_name:
            fail(f"projection relationship {relationship_id}: unknown file")
        if canonical_bytes(execution_object_projection(loaded[source])) != raw_by_name[expected]:
            fail(f"projection relationship {relationship_id}: projection mismatch")
    return len(relationships)


def _check_cases(manifest: dict[str, Any], loaded: dict[str, Any], raw_by_name: dict[str, bytes]) -> tuple[int, int, int]:
    cases = manifest.get("patch_cases")
    if not isinstance(cases, list) or not cases:
        fail("manifest.patch_cases: expected a nonempty list")
    ids: set[str] = set()
    successes = 0
    failures = 0
    case_keys = {"atomic", "base", "base_revision_relation", "checks", "current", "error", "expected_after", "expected_committed_revision", "expected_new_revision", "expected_restored", "expected_restored_revision", "expected_unchanged", "id", "inverse", "request", "status"}
    for case in cases:
        if not isinstance(case, dict):
            fail("manifest.patch_cases: malformed case")
        require_exact_keys(case, case_keys, "patch case")
        case_id = require_string(case["id"], "patch case id")
        if case_id in ids or case_id not in CASE_CONTRACTS:
            fail(f"patch case {case_id}: duplicate or unsupported id")
        ids.add(case_id)
        contract = CASE_CONTRACTS[case_id]
        expected_fields = {"status": contract["status"], "base": contract["base"], "current": contract["current"], "request": contract["request"], "error": contract["error"], "base_revision_relation": contract["relation"], "checks": contract["checks"]}
        for name, expected in expected_fields.items():
            if case[name] != expected:
                fail(f"patch case {case_id}: {name} does not match fixed contract")
        if case["atomic"] is not True:
            fail(f"patch case {case_id}: atomic result must be true")
        for name in ("base", "current", "request"):
            if case[name] not in loaded:
                fail(f"patch case {case_id}: unknown {name} file")
        request = loaded[case["request"]]
        base_revision = revision(raw_by_name[case["base"]])
        current_revision = revision(raw_by_name[case["current"]])
        relation = case["base_revision_relation"]
        if relation == "matches_base":
            if request["version"] != 2 or request["base_revision"] != base_revision or case["base"] != case["current"]:
                fail(f"patch case {case_id}: matching base revision relationship failed")
        elif relation == "stale_against_current":
            if request["version"] != 2 or request["base_revision"] != base_revision or base_revision == current_revision:
                fail(f"patch case {case_id}: stale base relationship failed")
        elif relation == "invalid_for_capability_precedence":
            if request["version"] != 1 or request["base_revision"] == current_revision:
                fail(f"patch case {case_id}: capability precedence control failed")
        else:
            fail(f"patch case {case_id}: unsupported base revision relationship")

        if case["status"] == "success":
            successes += 1
            after, inverse, restored = contract["after"], contract["inverse"], contract["restored"]
            if case["expected_after"] != after or case["inverse"] != inverse or case["expected_restored"] != restored or case["expected_unchanged"] is not None:
                fail(f"patch case {case_id}: success snapshot relationships mismatch")
            if any(reference not in loaded for reference in (after, inverse, restored)):
                fail(f"patch case {case_id}: unknown success relationship file")
            new_revision, restored_revision = revision(raw_by_name[after]), revision(raw_by_name[restored])
            if case["expected_new_revision"] != new_revision or case["expected_committed_revision"] != new_revision or case["expected_restored_revision"] != restored_revision:
                fail(f"patch case {case_id}: literal success revision mismatch")
            inverse_request = loaded[inverse]
            if inverse_request["version"] != 2 or inverse_request["base_revision"] != new_revision:
                fail(f"patch case {case_id}: inverse final-base revision mismatch")
            for operation in inverse_request["operations"]:
                if {"expect", "expect_absent", "expect_object"} & set(operation):
                    fail(f"patch case {case_id}: inverse contains a copied guard")
        elif case["status"] == "failure":
            failures += 1
            nullable = ("expected_after", "expected_new_revision", "inverse", "expected_restored", "expected_restored_revision")
            if any(case[name] is not None for name in nullable):
                fail(f"patch case {case_id}: failure cannot declare after/inverse state")
            if case["expected_unchanged"] != case["current"] or case["expected_committed_revision"] != current_revision:
                fail(f"patch case {case_id}: atomic unchanged relationship mismatch")
            if case["error"] not in {"E_CAPABILITY", "E_CONFLICT", "E_RANGE", "E_REFERENCE"}:
                fail(f"patch case {case_id}: unsupported stable error code")
        else:
            fail(f"patch case {case_id}: unsupported status")
        _check_case_trace(case_id, loaded)
    if ids != set(CASE_CONTRACTS):
        fail("manifest.patch_cases: fixed case inventory mismatch")
    return len(cases), successes, failures


def _require_fixed_files(loaded: dict[str, Any]) -> None:
    required = {
        "authored/constant-omitted.typed.json",
        "authored/minimal-label-a.typed.json",
        "authored/minimal-label-b.typed.json",
        "authored/minimal-tail-1-50s.typed.json",
        "authored/minimal-tail-omitted.typed.json",
        "normalized/constant-default.execution.json",
        "normalized/minimal-default.execution.json",
        "normalized/minimal-label-a.normalized.json",
        "normalized/minimal-label-b.normalized.json",
        "normalized/minimal-tail-1-50.execution.json",
    }
    for contract in CASE_CONTRACTS.values():
        for name in ("base", "current", "request", "after", "inverse", "restored"):
            reference = contract.get(name)
            if isinstance(reference, str):
                required.add(reference)
    missing = sorted(required - set(loaded))
    if missing:
        fail(f"missing required fixed files: {missing}")


def check_corpus(corpus: Path) -> dict[str, int | str]:
    manifest_path = corpus / "manifest.json"
    manifest = parse_json(manifest_path.read_bytes(), manifest_path)
    if not isinstance(manifest, dict) or manifest.get("schema") != "maac.conformance.identity-edit/2":
        fail("manifest: unsupported schema")
    require_exact_keys(manifest, {"schema", "authority", "evidence_boundary", "files", "fragment_context", "normalization_vectors", "patch_cases", "position_resolution_vectors", "projection_relationships"}, "manifest")

    authority = manifest.get("authority")
    if not isinstance(authority, dict):
        fail("manifest.authority: expected an object")
    expected_authority = {"language_version": 1, "patch_protocol": 2, "revision_algorithm": REVISION_AUTHORITY, "revision_preimage": "canonical authored typed-document bytes with no prefix bytes", "syntax_tree_version": 1}
    require_exact_keys(authority, set(expected_authority), "manifest.authority")
    for name in ("language_version", "patch_protocol", "syntax_tree_version"):
        if type(authority[name]) is not int:
            fail(f"manifest.authority.{name}: expected an integer")
    if authority != expected_authority:
        fail("manifest.authority: adopted authored revision/protocol contract mismatch")

    boundary = manifest.get("evidence_boundary")
    if not isinstance(boundary, dict):
        fail("manifest.evidence_boundary: expected an object")
    require_exact_keys(boundary, {"editor_conformance", "establishes", "normalizer_conformance", "scope"}, "manifest.evidence_boundary")
    if boundary["editor_conformance"] is not False or boundary["normalizer_conformance"] is not False:
        fail("manifest.evidence_boundary: checker cannot claim editor/normalizer conformance")
    if not isinstance(boundary["establishes"], list) or not boundary["establishes"] or not all(isinstance(item, str) for item in boundary["establishes"]):
        fail("manifest.evidence_boundary.establishes: expected nonempty string list")
    require_string(boundary["scope"], "manifest.evidence_boundary.scope")

    fragment = manifest.get("fragment_context")
    if not isinstance(fragment, dict):
        fail("manifest.fragment_context: expected an object")
    require_exact_keys(fragment, {"claim", "display_metadata", "execution_data_to_preserve"}, "manifest.fragment_context")
    if not isinstance(fragment["execution_data_to_preserve"], list) or not all(isinstance(item, str) for item in fragment["execution_data_to_preserve"]):
        fail("manifest.fragment_context.execution_data_to_preserve: expected string list")
    require_string(fragment["claim"], "manifest.fragment_context.claim")
    require_string(fragment["display_metadata"], "manifest.fragment_context.display_metadata")

    loaded, raw_by_name = _load_files(corpus, manifest_path, manifest.get("files"))
    _require_fixed_files(loaded)
    vector_count = _check_vectors(manifest, loaded, raw_by_name)
    position_vector_count = _check_position_resolution_vectors(manifest, loaded)
    projection_count = _check_projections(manifest, loaded, raw_by_name)
    case_count, success_count, failure_count = _check_cases(manifest, loaded, raw_by_name)
    return {"cases": case_count, "digests": len(loaded), "failures": failure_count, "files": len(loaded), "normalization_vectors": vector_count, "position_vectors": position_vector_count, "projections": projection_count, "protocol": 2, "revision_authority": REVISION_AUTHORITY, "successes": success_count}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    arguments = parser.parse_args()
    try:
        counts = check_corpus(arguments.corpus.resolve())
    except (CorpusError, OSError) as error:
        print(f"FAIL L1 identity corpus: {error}")
        return 1
    print("PASS L1 identity corpus: " + " ".join(f"{key}={value}" for key, value in counts.items()))
    print("scope=corpus-self-consistency-and-static-vectors; normalizer-conformance=false; editor-conformance=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
