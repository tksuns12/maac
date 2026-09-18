# Native external ABI v1

This document publishes the calling contract for the MaaC native external-processor ABI implemented by `maac::external_native`.

The capability identifiers are exact:

- ABI: `maac.native-c-abi/1`
- adapter: `maac.native-dylib-adapter/1`

A descriptor using any other ABI or adapter is not executable through this adapter. The host does not infer this ABI from a filename, file extension, magic number, or platform loader behavior.

## Module bytes and loading

The implementation dependency bytes are already hash-verified by the Generic Lock external-discovery boundary before this adapter receives them. On Unix hosts, the adapter materializes those exact bytes to a private temporary file and opens that file with the platform dynamic loader. The temporary file is retained for the complete lifetime of the loaded instance.

Loading is attempted only after host ABI, adapter, determinism, and permission authorization succeeds.

## Required exported symbols

A conforming module exports these C ABI symbols:

```c
void *maac_external_v1_create(void);
void maac_external_v1_destroy(void *instance);

int32_t maac_external_v1_restore_state(
    void *instance,
    const uint8_t *bytes,
    size_t len);

int32_t maac_external_v1_apply_config(
    void *instance,
    const uint8_t *canonical_json,
    size_t len);

int32_t maac_external_v1_set_parameter(
    void *instance,
    const uint8_t *id_utf8,
    size_t id_len,
    const uint8_t *canonical_json,
    size_t value_len);

int32_t maac_external_v1_process_f64_planar(
    void *instance,
    size_t frames,
    size_t input_channels,
    const double *const *inputs,
    size_t output_channels,
    double *const *outputs);
```

`create` must return a non-null instance. Every successful instance is destroyed exactly once.

## Lifecycle

After instantiation, the host follows MaaC/1 §17 ordering exactly:

1. restore the complete pinned state when one exists;
2. apply the locked normalized configuration;
3. apply explicit parameter overrides in caller-supplied order;
4. process audio.

Configuration and parameter values are passed as §20 canonical JSON bytes. Parameter IDs are UTF-8 bytes and are descriptor-stable IDs, not display names.

## Audio layout

`process_f64_planar` operates on binary64 planar channel buffers. Every channel contains exactly `frames` samples. Input buffers are read-only. Output buffers are writable and pre-zeroed by the caller when zero initialization is required by the surrounding graph contract.

The function must not retain buffer pointers after it returns.

## Status codes

Every function returning `int32_t` uses:

| status | meaning | MaaC error |
|---:|---|---|
| 0 | success | — |
| 1 | malformed state/config/value | `E_SCHEMA` |
| 2 | missing or unknown reference | `E_REFERENCE` |
| 3 | unsupported capability/state combination | `E_CAPABILITY` |
| 4 | permission denied | `E_PERMISSION` |
| 5 | bounded resource failure | `E_RESOURCE_LIMIT` |
| 6 | processing failure | `E_RENDER_STATE` |

Any other status is mapped to `E_RENDER_STATE`.

## Scope

This ABI v1 is intentionally narrow. It defines native dynamic-library invocation and planar binary64 audio processing only. It does not define subprocess hosting, sandboxing, network access, device access, shared-memory transport, event ports, sample-accurate parameter streams, block-dependent scheduling, checkpoint state, or crash isolation. Those capabilities require separate explicit contracts.
