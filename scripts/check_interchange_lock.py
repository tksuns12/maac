#!/usr/bin/env python3
"""Validate the fixed L4 generic-interchange corpus without running MaaC.

This stdlib-only checker validates canonical bytes, fixed hashes, closed wire
shapes, and the relationships declared by the synthetic fixture contracts. It
does not normalize source, discover runtime dependencies, execute processors,
render PCM, or establish generic runtime lock conformance.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import math
import re
import struct
from pathlib import Path, PurePosixPath
from typing import Any

from check_identity_edit import (
    CorpusError,
    canonical_bytes,
    fail,
    parse_json,
    require_exact_keys,
    validate_identifier,
    validate_path,
    validate_typed_document,
    validate_value,
)


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CORPUS = ROOT / "conformance" / "l4"
DIGEST = re.compile(r"sha256:[0-9a-f]{64}")
RAW_DIGEST = re.compile(r"[0-9a-f]{64}")
UNSIGNED = re.compile(r"0|[1-9][0-9]*")
POSITIVE_UNSIGNED = re.compile(r"[1-9][0-9]*")
FILE_ROLES = {
    "config",
    "render_input",
    "lock",
    "typed_document",
    "fixture_contract",
    "bytes",
    "mutation_or_invalid",
}
EXPECTED_INVALIDS = {
    "missing-version": "E_SCHEMA",
    "float-version": "E_SCHEMA",
    "true-version": "E_SCHEMA",
    "unknown-format": "E_SCHEMA",
    "version-two": "E_SCHEMA",
    "leading-zero-bytes": "E_SCHEMA",
    "negative-bytes": "E_SCHEMA",
    "numeric-bytes": "E_SCHEMA",
    "unknown-field": "E_SCHEMA",
    "whole-config-replaced-by-record": "E_SCHEMA",
    "bad-engine-role": "E_SCHEMA",
    "duplicate-engine-slot": "E_SCHEMA",
    "unsorted-dependencies": "E_ORDER",
    "unsorted-processors": "E_ORDER",
    "duplicate-processor-node": "E_CLOSURE",
    "stale-config-digest": "E_DIGEST",
    "stripped-config-label": "E_DIGEST",
    "stale-render-key": "E_DIGEST",
    "role-cross-pin": "E_CLOSURE",
    "missing-processor-slot": "E_CLOSURE",
    "missing-state-slot": "E_CLOSURE",
    "duplicate-processor-slot": "E_CLOSURE",
    "bad-crop": "E_TIMING",
    "bad-channel-order": "E_SCHEMA",
    "bad-block-total": "E_TIMING",
    "config-descriptor-mismatch": "E_DIGEST",
    "bad-evidence-length": "E_EVIDENCE",
    "noncanonical-short-escape": "E_CANONICAL",
}


def reject(code: str, message: str) -> None:
    fail(f"{code}: {message}")


def require_object(value: Any, context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        reject("E_SCHEMA", f"{context}: expected an object")
    return value


def require_array(value: Any, context: str) -> list[Any]:
    if not isinstance(value, list):
        reject("E_SCHEMA", f"{context}: expected an array")
    return value


def require_string(value: Any, context: str, *, nonempty: bool = False) -> str:
    if not isinstance(value, str) or (nonempty and not value):
        reject("E_SCHEMA", f"{context}: expected a{' nonempty' if nonempty else ''} string")
    return value


def require_version(value: Any, context: str) -> None:
    if type(value) is not int or value != 1:
        reject("E_SCHEMA", f"{context}: expected literal integer 1")


def require_digest(value: Any, context: str) -> str:
    if not isinstance(value, str) or DIGEST.fullmatch(value) is None:
        reject("E_SCHEMA", f"{context}: expected sha256 plus 64 lowercase hex digits")
    return value


def require_unsigned(value: Any, context: str, *, positive: bool = False) -> int:
    pattern = POSITIVE_UNSIGNED if positive else UNSIGNED
    if not isinstance(value, str) or pattern.fullmatch(value) is None:
        reject("E_SCHEMA", f"{context}: noncanonical unsigned decimal string")
    return int(value)


def check_exact_keys(value: dict[str, Any], expected: set[str], context: str) -> None:
    try:
        require_exact_keys(value, expected, context)
    except CorpusError as error:
        reject("E_SCHEMA", str(error))


def check_path(value: Any, context: str) -> tuple[str, ...]:
    try:
        validate_path(value, context)
    except CorpusError as error:
        reject("E_SCHEMA", str(error))
    return tuple(value)


def check_typed_value(value: Any, context: str) -> None:
    try:
        validate_value(value, context)
    except CorpusError as error:
        reject("E_SCHEMA", str(error))


def check_record(value: Any, context: str) -> None:
    check_typed_value(value, context)
    if value.get("t") != "record":
        reject("E_SCHEMA", f"{context}: expected a complete typed record")


def check_quantity(value: Any, unit: str, context: str, *, nonnegative: bool = False) -> None:
    check_typed_value(value, context)
    if value.get("t") != "quantity" or value.get("u") != unit:
        reject("E_SCHEMA", f"{context}: expected a reduced {unit} quantity")
    if nonnegative and int(value["n"]) < 0:
        reject("E_SCHEMA", f"{context}: quantity must be nonnegative")


def validate_config(value: Any, context: str) -> None:
    value = require_object(value, context)
    check_exact_keys(value, {"format", "version", "processor_type", "descriptor_hash", "config"}, context)
    if value["format"] != "maac.generic-config":
        reject("E_SCHEMA", f"{context}.format: expected maac.generic-config")
    require_version(value["version"], f"{context}.version")
    require_string(value["processor_type"], f"{context}.processor_type", nonempty=True)
    if value["descriptor_hash"] is not None:
        require_digest(value["descriptor_hash"], f"{context}.descriptor_hash")
    check_record(value["config"], f"{context}.config")


def validate_asset(value: Any, context: str) -> None:
    value = require_object(value, context)
    check_exact_keys(value, {"source", "kind", "sha256", "bytes"}, context)
    check_path(value["source"], f"{context}.source")
    if value["kind"] not in {"audio", "blob", "descriptor", "module"}:
        reject("E_SCHEMA", f"{context}.kind: invalid asset kind")
    require_digest(value["sha256"], f"{context}.sha256")
    require_unsigned(value["bytes"], f"{context}.bytes")


def validate_dependency(value: Any, context: str) -> None:
    value = require_object(value, context)
    check_exact_keys(value, {"owner", "role", "sha256", "bytes"}, context)
    owner = value["owner"]
    if owner is not None:
        check_path(owner, f"{context}.owner")
    role = require_array(value["role"], f"{context}.role")
    for index, part in enumerate(role):
        require_string(part, f"{context}.role[{index}]", nonempty=True)
    valid = False
    if owner is None and role == ["engine", "implementation"]:
        valid = True
    elif owner is not None and len(role) == 2 and role[0] == "processor" and role[1] in {
        "implementation", "descriptor", "adapter", "state"
    }:
        valid = True
    elif len(role) == 3 and role[0] in {"imported-source", "auxiliary"}:
        valid = True
    if not valid:
        reject("E_SCHEMA", f"{context}: invalid owner/role tuple")
    require_digest(value["sha256"], f"{context}.sha256")
    require_unsigned(value["bytes"], f"{context}.bytes")


def validate_processor(value: Any, context: str) -> None:
    value = require_object(value, context)
    check_exact_keys(
        value,
        {
            "node", "processor_type", "implementation_hash", "descriptor_hash", "adapter_id",
            "state_hash", "config", "config_digest", "latency_frames", "determinism",
        },
        context,
    )
    check_path(value["node"], f"{context}.node")
    require_string(value["processor_type"], f"{context}.processor_type", nonempty=True)
    identity = [value["implementation_hash"], value["descriptor_hash"], value["adapter_id"]]
    all_null = all(item is None for item in identity)
    all_present = all(item is not None for item in identity)
    if not (all_null or all_present):
        reject("E_SCHEMA", f"{context}: processor identity group must be all null or all nonnull")
    if all_present:
        require_digest(value["implementation_hash"], f"{context}.implementation_hash")
        require_digest(value["descriptor_hash"], f"{context}.descriptor_hash")
        require_string(value["adapter_id"], f"{context}.adapter_id", nonempty=True)
    if value["state_hash"] is not None:
        require_digest(value["state_hash"], f"{context}.state_hash")
    validate_config(value["config"], f"{context}.config")
    require_digest(value["config_digest"], f"{context}.config_digest")
    require_unsigned(value["latency_frames"], f"{context}.latency_frames")
    if value["determinism"] not in {"declared_deterministic", "nondeterministic"}:
        reject("E_SCHEMA", f"{context}.determinism: invalid value")


def validate_engine(value: Any, context: str) -> None:
    value = require_object(value, context)
    fields = {
        "implementation_id", "build_id", "platform_id", "architecture_id",
        "numerical_mode_id", "sample_rate", "block_schedule",
    }
    check_exact_keys(value, fields, context)
    for field in fields - {"sample_rate", "block_schedule"}:
        require_string(value[field], f"{context}.{field}", nonempty=True)
    require_unsigned(value["sample_rate"], f"{context}.sample_rate", positive=True)
    schedule = value["block_schedule"]
    if schedule is not None:
        schedule = require_array(schedule, f"{context}.block_schedule")
        if not schedule:
            reject("E_SCHEMA", f"{context}.block_schedule: expected a nonempty array")
        for index, block in enumerate(schedule):
            require_unsigned(block, f"{context}.block_schedule[{index}]", positive=True)


def validate_output(value: Any, context: str) -> None:
    value = require_object(value, context)
    check_exact_keys(
        value,
        {"port", "score", "tail", "render_frames", "crop", "channel_order", "encoding_id", "clipping_id", "dither_id"},
        context,
    )
    port = require_object(value["port"], f"{context}.port")
    check_exact_keys(port, {"t", "path", "port"}, f"{context}.port")
    if port["t"] != "ref":
        reject("E_SCHEMA", f"{context}.port.t: expected ref")
    check_path(port["path"], f"{context}.port.path")
    try:
        validate_identifier(port["port"], f"{context}.port.port")
    except CorpusError as error:
        reject("E_SCHEMA", str(error))
    score = require_array(value["score"], f"{context}.score")
    if len(score) != 2:
        reject("E_SCHEMA", f"{context}.score: expected two endpoints")
    for index, endpoint in enumerate(score):
        check_quantity(endpoint, "q", f"{context}.score[{index}]")
    check_quantity(value["tail"], "s", f"{context}.tail", nonnegative=True)
    require_unsigned(value["render_frames"], f"{context}.render_frames")
    crop = require_array(value["crop"], f"{context}.crop")
    if len(crop) != 2:
        reject("E_SCHEMA", f"{context}.crop: expected two endpoints")
    for index, endpoint in enumerate(crop):
        require_unsigned(endpoint, f"{context}.crop[{index}]")
    order = require_array(value["channel_order"], f"{context}.channel_order")
    if not order:
        reject("E_SCHEMA", f"{context}.channel_order: expected a nonempty array")
    for index, channel in enumerate(order):
        require_unsigned(channel, f"{context}.channel_order[{index}]")
    if len(set(order)) != len(order):
        reject("E_SCHEMA", f"{context}.channel_order: duplicate channel")
    if value["encoding_id"] != "pcm_f32le_interleaved/1" or value["clipping_id"] != "none" or value["dither_id"] != "none":
        reject("E_SCHEMA", f"{context}: unsupported fixed output encoding policy")


def validate_evidence(value: Any, context: str) -> None:
    value = require_object(value, context)
    check_exact_keys(value, {"pcm_sha256", "pcm_bytes", "file"}, context)
    require_digest(value["pcm_sha256"], f"{context}.pcm_sha256")
    require_unsigned(value["pcm_bytes"], f"{context}.pcm_bytes")
    if value["file"] is not None:
        file_value = require_object(value["file"], f"{context}.file")
        check_exact_keys(file_value, {"sha256", "bytes"}, f"{context}.file")
        require_digest(file_value["sha256"], f"{context}.file.sha256")
        require_unsigned(file_value["bytes"], f"{context}.file.bytes")


def validate_fixture_contract(value: Any, context: str) -> None:
    value = require_object(value, context)
    if value.get("format") == "maac.fixture.engine-contract/1":
        check_exact_keys(
            value,
            {
                "format", "implementation_id", "build_id", "platform_id",
                "architecture_id", "numerical_mode_id", "block_independent",
                "processor_contracts",
            },
            context,
        )
        for field in {"implementation_id", "build_id", "platform_id", "architecture_id", "numerical_mode_id"}:
            require_string(value[field], f"{context}.{field}", nonempty=True)
        if not isinstance(value["block_independent"], bool):
            reject("E_CORPUS", f"{context}.block_independent: expected boolean")
        contracts = require_object(value["processor_contracts"], f"{context}.processor_contracts")
        if set(contracts) != {"core.sine/1"}:
            reject("E_CORPUS", f"{context}: unexpected core fixture processor set")
        core = require_object(contracts["core.sine/1"], f"{context}.processor_contracts.core.sine/1")
        check_exact_keys(core, {"determinism", "latency_frames", "state"}, f"{context}.processor_contracts.core.sine/1")
        require_unsigned(core["latency_frames"], f"{context}.processor_contracts.core.sine/1.latency_frames")
        if core["determinism"] != "declared_deterministic" or core["state"] != "reset_only":
            reject("E_CORPUS", f"{context}: unexpected core fixture contract")
    elif value.get("format") == "maac.fixture.processor-contract/1":
        check_exact_keys(
            value,
            {
                "format", "processor_type", "capability_id", "adapter_id", "config",
                "latency_frames", "determinism", "output_channels", "state",
                "auxiliary_slots", "imported_source_slots",
            },
            context,
        )
        for field in {"processor_type", "capability_id", "adapter_id"}:
            require_string(value[field], f"{context}.{field}", nonempty=True)
        check_record(value["config"], f"{context}.config")
        require_unsigned(value["latency_frames"], f"{context}.latency_frames")
        require_unsigned(value["output_channels"], f"{context}.output_channels", positive=True)
        if value["determinism"] != "declared_deterministic":
            reject("E_CORPUS", f"{context}.determinism: unexpected fixture value")
        for field, expected in {
            "auxiliary_slots": ["alpha", "beta"],
            "imported_source_slots": ["alias_a", "alias_b"],
        }.items():
            if value[field] != expected:
                reject("E_CORPUS", f"{context}.{field}: unexpected fixture slots")
        state = require_object(value["state"], f"{context}.state")
        check_exact_keys(state, {"initial_state", "reset_frame", "serialization_id"}, f"{context}.state")
        if state != {
            "initial_state": "source_declared_optional",
            "reset_frame": "0",
            "serialization_id": "maac.fixture.state-f32le/1",
        }:
            reject("E_CORPUS", f"{context}.state: unexpected fixture serialization contract")
    else:
        reject("E_CORPUS", f"{context}: unknown fixture contract format")


def validate_render_input(value: Any, context: str, *, lock: bool = False) -> None:
    value = require_object(value, context)
    keys = {"format", "version", "execution_hash", "assets", "dependencies", "processors", "engine", "output"}
    if lock:
        keys |= {"render_key", "evidence"}
    check_exact_keys(value, keys, context)
    expected_format = "maac.generic-lock" if lock else "maac.generic-render-input"
    if value["format"] != expected_format:
        reject("E_SCHEMA", f"{context}.format: expected {expected_format}")
    require_version(value["version"], f"{context}.version")
    require_digest(value["execution_hash"], f"{context}.execution_hash")
    assets = require_array(value["assets"], f"{context}.assets")
    for index, item in enumerate(assets):
        validate_asset(item, f"{context}.assets[{index}]")
    dependencies = require_array(value["dependencies"], f"{context}.dependencies")
    for index, item in enumerate(dependencies):
        validate_dependency(item, f"{context}.dependencies[{index}]")
    engine_roles = [item for item in dependencies if item["owner"] is None and item["role"] == ["engine", "implementation"]]
    if len(engine_roles) != 1:
        reject("E_SCHEMA", f"{context}.dependencies: expected exactly one engine implementation role")
    processors = require_array(value["processors"], f"{context}.processors")
    for index, item in enumerate(processors):
        validate_processor(item, f"{context}.processors[{index}]")
    validate_engine(value["engine"], f"{context}.engine")
    validate_output(value["output"], f"{context}.output")
    if lock:
        require_digest(value["render_key"], f"{context}.render_key")
        if value["evidence"] is not None:
            validate_evidence(value["evidence"], f"{context}.evidence")


def derive_render_input(lock: dict[str, Any]) -> dict[str, Any]:
    value = {key: copy.deepcopy(item) for key, item in lock.items() if key not in {"render_key", "evidence"}}
    value["format"] = "maac.generic-render-input"
    return value


def sha256_uri(raw: bytes) -> str:
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def canonical_sort_key(value: Any) -> bytes:
    return canonical_bytes(value)


def dependency_identity(value: dict[str, Any]) -> dict[str, Any]:
    return {"owner": value["owner"], "role": value["role"]}


def validate_order_and_uniqueness(value: dict[str, Any], context: str) -> None:
    assets = value["assets"]
    asset_ids = [tuple(item["source"]) for item in assets]
    if len(set(asset_ids)) != len(asset_ids):
        reject("E_CLOSURE", f"{context}.assets: duplicate source identity")
    if [canonical_sort_key(item["source"]) for item in assets] != sorted(canonical_sort_key(item["source"]) for item in assets):
        reject("E_ORDER", f"{context}.assets: source identities are not sorted")

    dependencies = value["dependencies"]
    dep_ids = [canonical_bytes(dependency_identity(item)) for item in dependencies]
    if len(set(dep_ids)) != len(dep_ids):
        reject("E_CLOSURE", f"{context}.dependencies: duplicate dependency identity")
    if dep_ids != sorted(dep_ids):
        reject("E_ORDER", f"{context}.dependencies: dependency identities are not sorted")

    processors = value["processors"]
    processor_ids = [tuple(item["node"]) for item in processors]
    if len(set(processor_ids)) != len(processor_ids):
        reject("E_CLOSURE", f"{context}.processors: duplicate node identity")
    processor_sort = [canonical_bytes(item["node"]) for item in processors]
    if processor_sort != sorted(processor_sort):
        reject("E_ORDER", f"{context}.processors: node identities are not sorted")


def require_file(files: dict[str, bytes], values: dict[str, Any], relative: Any, context: str) -> tuple[bytes, Any]:
    if not isinstance(relative, str) or relative not in files:
        reject("E_CORPUS", f"{context}: unknown corpus file {relative!r}")
    return files[relative], values.get(relative)


def read_confined_file(path: Path, label: str) -> bytes:
    if path.is_symlink():
        reject("E_CORPUS", f"{label}: symlinked corpus metadata/artifacts are forbidden")
    if not path.is_file():
        reject("E_CORPUS", f"{label}: required regular file is absent")
    return path.read_bytes()


def validate_timing(value: dict[str, Any], timing: dict[str, Any], context: str) -> None:
    expected_keys = {"id", "execution", "score", "tail", "sample_rate", "render_frames", "output_channels", "tempo"}
    check_exact_keys(timing, expected_keys, f"{context}.timing")
    output = value["output"]
    if output["score"] != timing["score"] or output["tail"] != timing["tail"]:
        reject("E_TIMING", f"{context}: output score/tail differs from supplied timing vector")
    if value["engine"]["sample_rate"] != timing["sample_rate"] or output["render_frames"] != timing["render_frames"]:
        reject("E_TIMING", f"{context}: sample rate or render frame count differs from supplied timing vector")
    frames = require_unsigned(output["render_frames"], f"{context}.output.render_frames")
    start = require_unsigned(output["crop"][0], f"{context}.output.crop[0]")
    end = require_unsigned(output["crop"][1], f"{context}.output.crop[1]")
    if start > end or end > frames:
        reject("E_TIMING", f"{context}: crop is outside the half-open render interval")
    channels = require_unsigned(timing["output_channels"], f"{context}.timing.output_channels", positive=True)
    if sorted(int(item) for item in output["channel_order"]) != list(range(channels)):
        reject("E_SCHEMA", f"{context}: channel_order is not a permutation of all output channels")
    schedule = value["engine"]["block_schedule"]
    if schedule is not None and sum(int(item) for item in schedule) != frames:
        reject("E_TIMING", f"{context}: block schedule does not sum to render_frames")


def typed_scalar(value: Any, tag: str, context: str) -> Any:
    check_typed_value(value, context)
    if value.get("t") != tag:
        reject("E_CLOSURE", f"{context}: expected typed {tag}")
    return value.get("v")


def typed_ref_path(value: Any, context: str) -> tuple[str, ...]:
    check_typed_value(value, context)
    if value.get("t") != "ref" or value.get("port") is not None:
        reject("E_CLOSURE", f"{context}: expected an object reference")
    return tuple(value["path"])


def validate_source_bindings(
    value: dict[str, Any], execution: dict[str, Any], source: Any,
    files: dict[str, bytes], context: str,
) -> None:
    source = require_object(source, f"{context}.source_bindings")
    check_exact_keys(source, {"project", "requires", "processors", "assets"}, f"{context}.source_bindings")
    objects = require_object(execution.get("objects"), f"{context}.execution.objects")
    project_id = require_string(source["project"], f"{context}.source_bindings.project", nonempty=True)
    if project_id not in objects or objects[project_id].get("kind") != "project":
        reject("E_CLOSURE", f"{context}: supplied execution project is absent")
    project_fields = require_object(objects[project_id].get("fields"), f"{context}.execution project fields")
    requires = project_fields.get("requires")
    check_typed_value(requires, f"{context}.execution project requires")
    if requires.get("t") != "list":
        reject("E_CLOSURE", f"{context}: project.requires is not a list")
    actual_requires = [typed_scalar(item, "string", f"{context}.project.requires") for item in requires["items"]]
    if actual_requires != source["requires"]:
        reject("E_CLOSURE", f"{context}: project capability requirements mismatch")
    supported_requires = ["maac.fixture.external/1"] if source["processors"] else []
    if source["requires"] != supported_requires:
        reject("E_CAPABILITY", f"{context}: unsupported synthetic fixture capability")
    if project_fields.get("output") != value["output"]["port"]:
        reject("E_CLOSURE", f"{context}: project output reference differs from lock output")
    score = project_fields.get("score")
    if not isinstance(score, dict) or score.get("t") != "list" or score.get("items") != value["output"]["score"]:
        reject("E_TIMING", f"{context}: project score differs from lock output")
    if project_fields.get("tail") != value["output"]["tail"] or project_fields.get("rate", {}).get("n") != value["engine"]["sample_rate"]:
        reject("E_TIMING", f"{context}: project tail/rate differs from lock output")

    expected_assets = require_object(source["assets"], f"{context}.source_bindings.assets")
    actual_asset_objects = {name: item for name, item in objects.items() if isinstance(item, dict) and item.get("kind") == "asset"}
    if set(actual_asset_objects) != set(expected_assets):
        reject("E_CLOSURE", f"{context}: source asset declarations differ from supplied bindings")
    lock_assets = {tuple(item["source"]): item for item in value["assets"]}
    if set(lock_assets) != {(name,) for name in expected_assets}:
        reject("E_CLOSURE", f"{context}: locked source assets differ from supplied execution")
    for name, binding in expected_assets.items():
        binding = require_object(binding, f"{context}.source_bindings.assets.{name}")
        check_exact_keys(binding, {"kind", "path", "file"}, f"{context}.source_bindings.assets.{name}")
        raw, _ = require_file(files, {}, binding["file"], f"{context}.source_bindings.assets.{name}.file")
        fields = require_object(actual_asset_objects[name].get("fields"), f"{context}.execution asset {name}")
        if typed_scalar(fields.get("kind"), "symbol", f"{context}.asset.{name}.kind") != binding["kind"]:
            reject("E_CLOSURE", f"{context}: source asset {name} kind mismatch")
        if typed_scalar(fields.get("path"), "string", f"{context}.asset.{name}.path") != binding["path"]:
            reject("E_CLOSURE", f"{context}: source asset {name} path mismatch")
        if typed_scalar(fields.get("hash"), "string", f"{context}.asset.{name}.hash") != sha256_uri(raw):
            reject("E_CLOSURE", f"{context}: source asset {name} hash mismatch")
        locked = lock_assets[(name,)]
        if locked["kind"] != binding["kind"] or locked["sha256"] != sha256_uri(raw) or locked["bytes"] != str(len(raw)):
            reject("E_CLOSURE", f"{context}: lock asset {name} differs from source declaration")

    expected_processors = require_object(source["processors"], f"{context}.source_bindings.processors")
    for name, binding in expected_processors.items():
        binding = require_object(binding, f"{context}.source_bindings.processors.{name}")
        check_exact_keys(binding, {"implementation", "state"}, f"{context}.source_bindings.processors.{name}")
        node = objects.get(name)
        if not isinstance(node, dict) or node.get("kind") != "node":
            reject("E_CLOSURE", f"{context}: external source node {name} is absent")
        fields = require_object(node.get("fields"), f"{context}.execution node {name}")
        if typed_ref_path(fields.get("implementation"), f"{context}.node.{name}.implementation") != tuple(binding["implementation"]):
            reject("E_CLOSURE", f"{context}: node {name} implementation reference mismatch")
        if typed_ref_path(fields.get("state"), f"{context}.node.{name}.state") != tuple(binding["state"]):
            reject("E_CLOSURE", f"{context}: node {name} state reference mismatch")
        processor = next((item for item in value["processors"] if item["node"] == [name]), None)
        if processor is None:
            reject("E_CLOSURE", f"{context}: source processor {name} lacks lock entry")
        if processor["config"]["config"] != fields.get("config") or processor["processor_type"] != typed_scalar(fields.get("type"), "string", f"{context}.node.{name}.type"):
            reject("E_CLOSURE", f"{context}: node {name} type/config differs from lock")


def validate_static_render(
    value: dict[str, Any], relation: dict[str, Any], timings: dict[str, dict[str, Any]],
    files: dict[str, bytes], values: dict[str, Any], context: str,
) -> None:
    validate_order_and_uniqueness(value, context)
    _, execution = require_file(files, values, relation.get("execution"), f"{context}.execution")
    if execution is None:
        reject("E_CORPUS", f"{context}.execution: expected JSON")

    processor_configs = relation.get("processor_configs")
    if not isinstance(processor_configs, dict):
        reject("E_CORPUS", f"{context}.processor_configs: expected object")
    processors = {"/".join(item["node"]): item for item in value["processors"]}
    if set(processors) != set(processor_configs):
        reject("E_CLOSURE", f"{context}: processor set differs from declared config bindings")
    for name, relative in processor_configs.items():
        raw, config = require_file(files, values, relative, f"{context}.processor_configs.{name}")
        processor = processors[name]
        actual_config_digest = sha256_uri(canonical_bytes(processor["config"]))
        if processor["config_digest"] != actual_config_digest:
            reject("E_DIGEST", f"{context}: processor {name} config_digest is stale")
        if processor["config"] != config:
            reject("E_CLOSURE", f"{context}: processor {name} does not contain its bound Config")
        if processor["config_digest"] != sha256_uri(raw):
            reject("E_DIGEST", f"{context}: processor {name} config_digest is stale")
        config_value = processor["config"]
        if config_value["processor_type"] != processor["processor_type"] or config_value["descriptor_hash"] != processor["descriptor_hash"]:
            reject("E_CLOSURE", f"{context}: processor {name} Config context mismatch")

    validate_source_bindings(value, execution, relation.get("source_bindings"), files, context)
    if value["execution_hash"] != sha256_uri(canonical_bytes(execution)):
        reject("E_DIGEST", f"{context}: execution_hash does not match supplied preimage")

    binding_list = relation.get("dependency_bindings")
    if not isinstance(binding_list, list):
        reject("E_CORPUS", f"{context}.dependency_bindings: expected array")
    expected_deps: dict[bytes, tuple[str, bytes]] = {}
    for index, binding in enumerate(binding_list):
        binding = require_object(binding, f"{context}.dependency_bindings[{index}]")
        check_exact_keys(binding, {"owner", "role", "file"}, f"{context}.dependency_bindings[{index}]")
        raw, _ = require_file(files, values, binding["file"], f"{context}.dependency_bindings[{index}].file")
        identity = canonical_bytes({"owner": binding["owner"], "role": binding["role"]})
        if identity in expected_deps:
            reject("E_CORPUS", f"{context}: duplicate dependency binding")
        expected_deps[identity] = (binding["file"], raw)
    actual_deps = {canonical_bytes(dependency_identity(item)): item for item in value["dependencies"]}
    if set(actual_deps) != set(expected_deps):
        reject("E_CLOSURE", f"{context}: dependency closure differs from fixture contract")
    for identity, (relative, raw) in expected_deps.items():
        item = actual_deps[identity]
        if item["sha256"] != sha256_uri(raw) or item["bytes"] != str(len(raw)):
            reject("E_CLOSURE", f"{context}: dependency {relative} is cross-pinned or stale")

    asset_bindings = relation.get("asset_bindings")
    if not isinstance(asset_bindings, list):
        reject("E_CORPUS", f"{context}.asset_bindings: expected array")
    expected_assets: dict[tuple[str, ...], tuple[str, bytes]] = {}
    for index, binding in enumerate(asset_bindings):
        binding = require_object(binding, f"{context}.asset_bindings[{index}]")
        check_exact_keys(binding, {"source", "file"}, f"{context}.asset_bindings[{index}]")
        raw, _ = require_file(files, values, binding["file"], f"{context}.asset_bindings[{index}].file")
        expected_assets[tuple(binding["source"])] = (binding["file"], raw)
    actual_assets = {tuple(item["source"]): item for item in value["assets"]}
    if set(actual_assets) != set(expected_assets):
        reject("E_CLOSURE", f"{context}: asset closure differs from fixture inventory")
    for source, (relative, raw) in expected_assets.items():
        item = actual_assets[source]
        if item["sha256"] != sha256_uri(raw) or item["bytes"] != str(len(raw)):
            reject("E_CLOSURE", f"{context}: asset {relative} hash/length mismatch")

    timing_id = relation.get("timing")
    if not isinstance(timing_id, str) or timing_id not in timings:
        reject("E_CORPUS", f"{context}.timing: unknown vector")
    validate_timing(value, timings[timing_id], context)

    engine_raw, engine_contract = require_file(files, values, "contracts/fixture-engine.json", f"{context}.engine contract")
    engine_dep = next(item for item in value["dependencies"] if item["owner"] is None and item["role"] == ["engine", "implementation"])
    if engine_dep["sha256"] != sha256_uri(engine_raw) or engine_dep["bytes"] != str(len(engine_raw)):
        reject("E_CLOSURE", f"{context}: engine implementation pin mismatch")
    engine_fields = {key: engine_contract[key] for key in ("implementation_id", "build_id", "platform_id", "architecture_id", "numerical_mode_id")}
    if any(value["engine"][key] != expected for key, expected in engine_fields.items()):
        reject("E_CLOSURE", f"{context}: engine identity differs from fixture contract")
    if value["engine"]["block_schedule"] is None and not bool(engine_contract["block_independent"]):
        reject("E_CLOSURE", f"{context}: null block schedule lacks a block-independence contract")

    for name, processor in processors.items():
        owned = {item["role"][1]: item for item in value["dependencies"] if item["owner"] == processor["node"] and item["role"][0] == "processor"}
        if processor["implementation_hash"] is None:
            if owned or processor["descriptor_hash"] is not None or processor["adapter_id"] is not None or processor["state_hash"] is not None:
                reject("E_CLOSURE", f"{context}: null processor identity has owned slots")
            contract = engine_contract.get("processor_contracts", {}).get(processor["processor_type"])
            if not isinstance(contract, dict):
                reject("E_CLOSURE", f"{context}: no synthetic core processor contract")
        else:
            required = {"implementation", "descriptor", "adapter"}
            if processor["state_hash"] is not None:
                required.add("state")
            if set(owned) != required:
                reject("E_CLOSURE", f"{context}: external processor slot closure mismatch")
            if owned["implementation"]["sha256"] != processor["implementation_hash"] or owned["descriptor"]["sha256"] != processor["descriptor_hash"]:
                reject("E_CLOSURE", f"{context}: external processor byte hashes mismatch")
            if processor["state_hash"] is not None and owned["state"]["sha256"] != processor["state_hash"]:
                reject("E_CLOSURE", f"{context}: external processor state hash mismatch")
            descriptor_file = next(
                (binding["file"] for binding in binding_list if binding["owner"] == processor["node"] and binding["role"] == ["processor", "descriptor"]),
                None,
            )
            descriptor = values.get(descriptor_file) if descriptor_file is not None else None
            if not isinstance(descriptor, dict):
                reject("E_CORPUS", f"{context}: external descriptor is not JSON")
            contract = descriptor
            if descriptor["adapter_id"] != processor["adapter_id"] or descriptor["processor_type"] != processor["processor_type"]:
                reject("E_CLOSURE", f"{context}: external descriptor identity mismatch")
            if descriptor["config"] != processor["config"]["config"]:
                reject("E_CLOSURE", f"{context}: external descriptor Config mismatch")
        if processor["latency_frames"] != contract["latency_frames"] or processor["determinism"] != contract["determinism"]:
            reject("E_CLOSURE", f"{context}: latency/determinism differs from fixture contract")


def validate_pcm(lock: dict[str, Any], evidence_raw: bytes, pcm: dict[str, Any], context: str) -> None:
    evidence = lock["evidence"]
    if evidence is None:
        reject("E_EVIDENCE", f"{context}: expected evidence object")
    expected_digest = require_digest(pcm.get("sha256"), "manifest.pcm.sha256")
    expected_bytes = require_unsigned(pcm.get("bytes"), "manifest.pcm.bytes")
    if sha256_uri(evidence_raw) != expected_digest or len(evidence_raw) != expected_bytes:
        reject("E_EVIDENCE", f"{context}: supplied PCM bytes differ from fixed evidence vector")
    if evidence["pcm_sha256"] != expected_digest or evidence["pcm_bytes"] != str(expected_bytes):
        reject("E_EVIDENCE", f"{context}: evidence PCM hash/length mismatch")
    channels = len(lock["output"]["channel_order"])
    start, end = map(int, lock["output"]["crop"])
    if len(evidence_raw) != (end - start) * channels * 4:
        reject("E_EVIDENCE", f"{context}: PCM length does not match crop and channel count")
    samples = struct.unpack("<" + "f" * (len(evidence_raw) // 4), evidence_raw)
    if not all(math.isfinite(sample) for sample in samples):
        reject("E_EVIDENCE", f"{context}: PCM contains a nonfinite binary32 sample")
    actual_bits = [
        f"{struct.unpack('<I', evidence_raw[index:index + 4])[0]:08x}"
        for index in range(0, len(evidence_raw), 4)
    ]
    if actual_bits != pcm.get("sample_bits"):
        reject("E_EVIDENCE", f"{context}: PCM bit patterns differ from fixed rounding vector")
    file_value = evidence["file"]
    if file_value is not None and (file_value["sha256"] != expected_digest or file_value["bytes"] != str(expected_bytes)):
        reject("E_EVIDENCE", f"{context}: raw-profile file evidence differs from exact PCM bytes")


def validate_lock_candidate(
    lock: dict[str, Any], lock_relation: dict[str, Any], render_relation: dict[str, Any],
    timings: dict[str, dict[str, Any]], files: dict[str, bytes], values: dict[str, Any],
    pcm: dict[str, Any], context: str,
) -> None:
    validate_render_input(lock, context, lock=True)
    validate_static_render(lock, render_relation, timings, files, values, context)
    derived = derive_render_input(lock)
    expected_key = sha256_uri(canonical_bytes(derived))
    if lock["render_key"] != expected_key:
        reject("E_DIGEST", f"{context}: render_key does not hash the complete pre-render input")
    evidence_file = lock_relation.get("evidence_file")
    if evidence_file is None:
        if lock["evidence"] is not None:
            reject("E_EVIDENCE", f"{context}: unexpected evidence")
    else:
        raw, _ = require_file(files, values, evidence_file, f"{context}.evidence_file")
        validate_pcm(lock, raw, pcm, context)


def apply_mutations(base: Any, descriptor: dict[str, Any], context: str) -> Any:
    check_exact_keys(descriptor, {"base", "expected", "operations"}, context)
    result = copy.deepcopy(base)
    operations = require_array(descriptor["operations"], f"{context}.operations")
    if not operations:
        reject("E_CORPUS", f"{context}.operations: expected nonempty array")

    def locate(path: Any, op_context: str) -> tuple[Any, Any]:
        path = require_array(path, f"{op_context}.path")
        if not path:
            reject("E_CORPUS", f"{op_context}.path: empty mutation path")
        current = result
        for part in path[:-1]:
            try:
                current = current[part]
            except (KeyError, IndexError, TypeError):
                reject("E_CORPUS", f"{op_context}.path: invalid mutation path")
        return current, path[-1]

    for index, operation in enumerate(operations):
        op_context = f"{context}.operations[{index}]"
        operation = require_object(operation, op_context)
        op = operation.get("op")
        if op in {"replace", "add"}:
            check_exact_keys(operation, {"op", "path", "value"}, op_context)
            parent, key = locate(operation["path"], op_context)
            try:
                parent[key] = copy.deepcopy(operation["value"])
            except (IndexError, TypeError):
                reject("E_CORPUS", f"{op_context}.path: invalid mutation target")
        elif op == "delete":
            check_exact_keys(operation, {"op", "path"}, op_context)
            parent, key = locate(operation["path"], op_context)
            try:
                del parent[key]
            except (KeyError, IndexError, TypeError):
                reject("E_CORPUS", f"{op_context}.path: invalid delete target")
        elif op == "delete_index":
            check_exact_keys(operation, {"op", "path", "index"}, op_context)
            parent, key = locate(operation["path"], op_context)
            try:
                del parent[key][operation["index"]]
            except (KeyError, IndexError, TypeError):
                reject("E_CORPUS", f"{op_context}: invalid delete_index target")
        elif op == "append":
            check_exact_keys(operation, {"op", "path", "value"}, op_context)
            parent, key = locate(operation["path"], op_context)
            try:
                parent[key].append(copy.deepcopy(operation["value"]))
            except (KeyError, AttributeError, TypeError):
                reject("E_CORPUS", f"{op_context}: invalid append target")
        elif op == "swap":
            check_exact_keys(operation, {"op", "path", "indices"}, op_context)
            parent, key = locate(operation["path"], op_context)
            indices = operation["indices"]
            if not isinstance(indices, list) or len(indices) != 2 or not all(type(item) is int for item in indices):
                reject("E_CORPUS", f"{op_context}.indices: expected two integer indexes")
            try:
                parent[key][indices[0]], parent[key][indices[1]] = parent[key][indices[1]], parent[key][indices[0]]
            except (KeyError, IndexError, TypeError):
                reject("E_CORPUS", f"{op_context}: invalid swap target")
        elif op == "swap_fields":
            check_exact_keys(operation, {"op", "paths"}, op_context)
            paths = operation["paths"]
            if not isinstance(paths, list) or len(paths) != 2:
                reject("E_CORPUS", f"{op_context}.paths: expected two mutation paths")
            left_parent, left_key = locate(paths[0], op_context)
            right_parent, right_key = locate(paths[1], op_context)
            try:
                left_parent[left_key], right_parent[right_key] = right_parent[right_key], left_parent[left_key]
            except (KeyError, IndexError, TypeError):
                reject("E_CORPUS", f"{op_context}: invalid swap_fields target")
        else:
            reject("E_CORPUS", f"{op_context}.op: unsupported mutation operation")
    return result


def load_corpus(corpus: Path) -> tuple[dict[str, Any], dict[str, bytes], dict[str, Any]]:
    manifest_path = corpus / "manifest.json"
    manifest_raw = read_confined_file(manifest_path, "manifest.json")
    manifest = parse_json(manifest_raw, manifest_path)
    manifest = require_object(manifest, "manifest")
    check_exact_keys(manifest, {"schema", "evidence_boundary", "files", "relationships", "invalid_cases", "pcm"}, "manifest")
    if manifest["schema"] != "maac.conformance.generic-interchange/1":
        reject("E_CORPUS", "manifest.schema: unexpected corpus version")
    expected_boundary = {
        "schema_validation": "shape_only",
        "static_fixture_relationships": True,
        "runtime_lock_conformance": False,
        "normalizer_conformance": False,
        "dependency_discovery_conformance": False,
        "rendering_conformance": False,
    }
    if manifest["evidence_boundary"] != expected_boundary:
        reject("E_CORPUS", "manifest.evidence_boundary: overclaims the static corpus")
    specs = require_object(manifest["files"], "manifest.files")
    actual = {
        path.relative_to(corpus).as_posix()
        for path in corpus.rglob("*")
        if path.is_file() and path.name not in {"manifest.json", "SHA256SUMS", "README.md"}
    }
    if set(specs) != actual:
        reject("E_CORPUS", "manifest.files: exact corpus inventory mismatch")
    for relative in actual:
        if (corpus / relative).is_symlink():
            reject("E_CORPUS", f"{relative}: symlinked corpus artifacts are forbidden")
    files: dict[str, bytes] = {}
    values: dict[str, Any] = {}
    for relative, spec in specs.items():
        path = PurePosixPath(relative)
        if path.is_absolute() or ".." in path.parts or not path.parts:
            reject("E_CORPUS", f"manifest.files: unsafe path {relative!r}")
        spec = require_object(spec, f"manifest.files.{relative}")
        check_exact_keys(spec, {"bytes", "sha256", "role", "canonical"}, f"manifest.files.{relative}")
        if type(spec["bytes"]) is not int or spec["bytes"] < 0:
            reject("E_CORPUS", f"manifest.files.{relative}.bytes: invalid byte count")
        if not isinstance(spec["sha256"], str) or RAW_DIGEST.fullmatch(spec["sha256"]) is None:
            reject("E_CORPUS", f"manifest.files.{relative}.sha256: invalid digest")
        if spec["role"] not in FILE_ROLES:
            reject("E_CORPUS", f"manifest.files.{relative}: invalid file role")
        if spec["canonical"] is not None and type(spec["canonical"]) is not bool:
            reject("E_CORPUS", f"manifest.files.{relative}: canonical metadata must be boolean or null")
        raw = read_confined_file(corpus / relative, relative)
        if len(raw) != spec["bytes"] or hashlib.sha256(raw).hexdigest() != spec["sha256"]:
            reject("E_DIGEST", f"{relative}: transport bytes differ from manifest")
        files[relative] = raw
        if spec["role"] != "bytes":
            value = parse_json(raw, corpus / relative)
            values[relative] = value
            if spec["canonical"] is True and raw != canonical_bytes(value):
                reject("E_CANONICAL", f"{relative}: bytes are not canonical")
    sums: dict[str, str] = {}
    sums_path = corpus / "SHA256SUMS"
    sums_raw = read_confined_file(sums_path, "SHA256SUMS")
    try:
        sums_text = sums_raw.decode("ascii")
    except UnicodeDecodeError as error:
        reject("E_CORPUS", f"SHA256SUMS: non-ASCII content: {error}")
    for line_number, line in enumerate(sums_text.splitlines(), 1):
        if "  " not in line:
            reject("E_CORPUS", f"SHA256SUMS:{line_number}: malformed line")
        digest, relative = line.split("  ", 1)
        if relative in sums or RAW_DIGEST.fullmatch(digest) is None:
            reject("E_CORPUS", f"SHA256SUMS:{line_number}: duplicate path or invalid digest")
        sums[relative] = digest
    expected_sums = {relative: hashlib.sha256(raw).hexdigest() for relative, raw in files.items()}
    if sums != expected_sums:
        reject("E_DIGEST", "SHA256SUMS does not match exact corpus inventory")
    return manifest, files, values


def validate_manifest(corpus: Path) -> tuple[int, int, int]:
    manifest, files, values = load_corpus(corpus)
    relationships = require_object(manifest["relationships"], "manifest.relationships")
    check_exact_keys(relationships, {"configs", "render_inputs", "locks", "portable_pairs", "timing"}, "manifest.relationships")

    for relative, spec in manifest["files"].items():
        role = spec["role"]
        value = values.get(relative)
        if role == "config":
            validate_config(value, relative)
        elif role == "render_input":
            validate_render_input(value, relative)
        elif role == "lock":
            validate_render_input(value, relative, lock=True)
        elif role == "typed_document":
            try:
                validate_typed_document(value, relative)
            except CorpusError as error:
                reject("E_SCHEMA", str(error))
        elif role == "fixture_contract":
            validate_fixture_contract(value, relative)

    pcm = require_object(manifest["pcm"], "manifest.pcm")
    check_exact_keys(pcm, {"file", "sha256", "bytes", "sample_bits"}, "manifest.pcm")
    if pcm["file"] != "evidence/minimal.pcm" or not isinstance(pcm["sample_bits"], list):
        reject("E_CORPUS", "manifest.pcm: unexpected fixed evidence declaration")

    configs = require_array(relationships["configs"], "manifest.relationships.configs")
    if {item.get("id") for item in configs if isinstance(item, dict)} != {"core-sine", "fixture-external"}:
        reject("E_CORPUS", "config relationship IDs differ from fixed corpus")
    for index, item in enumerate(configs):
        item = require_object(item, f"manifest.relationships.configs[{index}]")
        check_exact_keys(item, {"id", "file"}, f"manifest.relationships.configs[{index}]")
        _, config = require_file(files, values, item["file"], f"manifest.relationships.configs[{index}].file")
        validate_config(config, item["file"])

    timing_items = require_array(relationships["timing"], "manifest.relationships.timing")
    timings: dict[str, dict[str, Any]] = {}
    for index, item in enumerate(timing_items):
        item = require_object(item, f"manifest.relationships.timing[{index}]")
        if not isinstance(item.get("id"), str) or item["id"] in timings:
            reject("E_CORPUS", "timing vector IDs must be unique strings")
        timings[item["id"]] = item
    if set(timings) != {"zero-step", "negative-step", "nonzero-ramp"}:
        reject("E_CORPUS", "timing vector IDs differ from fixed corpus")

    render_items = require_array(relationships["render_inputs"], "manifest.relationships.render_inputs")
    render_relations: dict[str, dict[str, Any]] = {}
    for index, item in enumerate(render_items):
        item = require_object(item, f"manifest.relationships.render_inputs[{index}]")
        check_exact_keys(item, {"id", "file", "execution", "processor_configs", "dependency_bindings", "asset_bindings", "source_bindings", "timing"}, f"manifest.relationships.render_inputs[{index}]")
        relative = item["file"]
        _, value = require_file(files, values, relative, f"manifest.relationships.render_inputs[{index}].file")
        validate_render_input(value, relative)
        validate_static_render(value, item, timings, files, values, relative)
        if relative in render_relations:
            reject("E_CORPUS", "duplicate render-input relationship")
        render_relations[relative] = item
    if set(item.get("id") for item in render_items) != {"minimal", "equal-config-negative", "external-ramp", "external-transitive"}:
        reject("E_CORPUS", "render-input IDs differ from fixed corpus")

    lock_items = require_array(relationships["locks"], "manifest.relationships.locks")
    lock_relations: dict[str, dict[str, Any]] = {}
    for index, item in enumerate(lock_items):
        item = require_object(item, f"manifest.relationships.locks[{index}]")
        check_exact_keys(item, {"id", "file", "render_input", "evidence_file"}, f"manifest.relationships.locks[{index}]")
        relative = item["file"]
        _, value = require_file(files, values, relative, f"manifest.relationships.locks[{index}].file")
        if item["render_input"] not in render_relations:
            reject("E_CORPUS", f"{relative}: unknown render-input relationship")
        validate_lock_candidate(value, item, render_relations[item["render_input"]], timings, files, values, manifest["pcm"], relative)
        if derive_render_input(value) != values[item["render_input"]]:
            reject("E_CORPUS", f"{relative}: lock pre-render fields differ from fixed render input")
        lock_relations[relative] = item
    if len(lock_relations) != 6:
        reject("E_CORPUS", "expected six fixed lock relationships")

    pairs = require_array(relationships["portable_pairs"], "manifest.relationships.portable_pairs")
    if len(pairs) != 3:
        reject("E_CORPUS", "expected three portable/canonical pairs")
    for index, pair in enumerate(pairs):
        pair = require_object(pair, f"manifest.relationships.portable_pairs[{index}]")
        check_exact_keys(pair, {"portable", "canonical"}, f"manifest.relationships.portable_pairs[{index}]")
        _, portable = require_file(files, values, pair["portable"], f"portable_pairs[{index}].portable")
        canonical_raw, canonical = require_file(files, values, pair["canonical"], f"portable_pairs[{index}].canonical")
        if portable != canonical or canonical_bytes(portable) != canonical_raw:
            reject("E_CANONICAL", f"portable pair {index}: semantic value/canonical bytes mismatch")

    invalids = require_array(manifest["invalid_cases"], "manifest.invalid_cases")
    observed = {item.get("id"): item.get("expected") for item in invalids if isinstance(item, dict)}
    if observed != EXPECTED_INVALIDS or len(invalids) != len(EXPECTED_INVALIDS):
        reject("E_CORPUS", "invalid case inventory/outcomes differ from fixed corpus")
    for index, item in enumerate(invalids):
        item = require_object(item, f"manifest.invalid_cases[{index}]")
        check_exact_keys(item, {"id", "file", "expected"}, f"manifest.invalid_cases[{index}]")
        raw, descriptor = require_file(files, values, item["file"], f"manifest.invalid_cases[{index}].file")
        try:
            if item["id"] == "noncanonical-short-escape":
                if raw == canonical_bytes(descriptor):
                    reject("E_CORPUS", "noncanonical escape fixture unexpectedly canonical")
                reject("E_CANONICAL", "short control-character escape is not canonical")
            descriptor = require_object(descriptor, item["file"])
            base_name = descriptor.get("base")
            if base_name not in lock_relations:
                reject("E_CORPUS", f"{item['file']}: unknown base lock")
            if descriptor.get("expected") != item["expected"]:
                reject("E_CORPUS", f"{item['file']}: expected code mismatch")
            candidate = apply_mutations(values[base_name], descriptor, item["file"])
            lock_relation = lock_relations[base_name]
            render_relation = render_relations[lock_relation["render_input"]]
            validate_lock_candidate(candidate, lock_relation, render_relation, timings, files, values, manifest["pcm"], item["file"])
        except CorpusError as error:
            code = str(error).split(":", 1)[0]
            if code != item["expected"]:
                reject("E_CORPUS", f"{item['id']}: expected {item['expected']}, observed {error}")
        else:
            reject("E_CORPUS", f"{item['id']}: invalid vector unexpectedly accepted")

    minimal_locks = [values[item["file"]] for item in lock_items if item["id"].startswith("minimal")]
    if len({lock["render_key"] for lock in minimal_locks}) != 1:
        reject("E_DIGEST", "adding/changing evidence changed the minimal render key")
    if len({hashlib.sha256(files[item["file"]]).hexdigest() for item in lock_items if item["id"].startswith("minimal")}) != 3:
        reject("E_CORPUS", "minimal evidence variants must have distinct lock transport hashes")
    external_keys = {item["id"]: values[item["file"]]["render_key"] for item in lock_items if item["id"].startswith("external")}
    if len(set(external_keys.values())) != 2:
        reject("E_DIGEST", "transitive dependency pin change did not change render key")
    ramp = copy.deepcopy(values["canonical/external-ramp-render-input.json"])
    transitive = copy.deepcopy(values["canonical/external-transitive-render-input.json"])
    ramp_beta = next(item for item in ramp["dependencies"] if item["role"] == ["auxiliary", "maac.fixture.external/1", "beta"])
    transitive_beta = next(item for item in transitive["dependencies"] if item["role"] == ["auxiliary", "maac.fixture.external/1", "beta"])
    transitive_beta["sha256"] = ramp_beta["sha256"]
    transitive_beta["bytes"] = ramp_beta["bytes"]
    if transitive != ramp:
        reject("E_CORPUS", "external transitive vectors differ by more than the beta auxiliary pin")

    return len(files), len(lock_items), len(invalids)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    args = parser.parse_args()
    try:
        files, locks, invalids = validate_manifest(args.corpus)
    except (CorpusError, OSError) as error:
        print(f"FAIL L4 interchange corpus: {error}")
        return 1
    print(
        "PASS L4 interchange corpus: "
        f"files={files}; locks={locks}; invalids={invalids}; "
        "schema-validation=shape-only; runtime-lock-conformance=false; "
        "normalizer-conformance=false; dependency-discovery-conformance=false; "
        "rendering-conformance=false"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
