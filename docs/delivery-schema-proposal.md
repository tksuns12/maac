# Built-in extension schemas: proposal

**Status:** accepted and implemented, 2026-10-08. The normative contract is
[§14.1](../MaaC-1-Specification.md#141-immutable-assets); this page keeps the
reasoning, the decisions and the results.

## Why

The [T2 production trial](ai-production-trial.md) logged F11: a delivery
needs a copied schema file and its hash pin. Every project that delivers
carries these lines:

```maac
asset schema { kind = descriptor; path = "production.schema.json"; hash = "sha256:1f9f3623515e645834fd97ecb300cdf00c5e11f4dd260f5bd85a92d3af91568d"; }
extension deliveries {
  namespace = "maac.production/1"; schema = &schema; render_affecting = true;
  data = { ... };
}
```

Each line of that boilerplate costs something:

- **The copy.** The project needs its own copy of
  [`production.schema.json`](../production.schema.json).
  [`examples/lofi/`](../examples/lofi/) keeps one next to the source, and so
  does every new song.
- **The pin.** The hash is 71 characters that the author must copy exactly. It
  carries no choice: the host accepts one exact byte sequence for
  `maac.production/1` and rejects every other.
- **The path base.** Asset paths resolve from the package root, while import,
  sample and wavetable paths resolve from the declaring file. A song in a
  subfolder must write the schema path from the root, unlike its imports.

The schema field adds no information for a namespace the host implements. The
host already embeds the schema and requires byte equality. The takes
extensions (`maac.takes/1` and `/2`) carry the same boilerplate.

## Existing position

- [§14.1](../MaaC-1-Specification.md#141-immutable-assets): an `asset`
  requires `kind`, `path`, and `hash`. The path is a package-relative
  retrieval hint, and the hash is the asset's identity.
- [§26](../MaaC-1-Specification.md#26-versioning-and-extensions): an
  `extension` requires `schema`, a descriptor-asset reference. A host must
  not trust an unknown schema's assertion.
- [production §1](production.md) and [grouped takes](grouped-takes.md): the
  schema pin and bytes must match the self-contained schema that the host
  embeds. The implementation compares bytes (`src/production_data.rs`,
  `src/takes.rs`) and records the schema hash in execution identity.
- Precedent: an `import` already selects one of two forms. One is a local
  `path` plus `hash`. The other is an exact `builtin` identity such as
  `import basic { builtin = "std/basic/1.0.0"; }`. The host embeds the
  built-in source, records it as `@builtin/std/basic/1.0.0.maac` with its
  SHA-256, and archives list it under `builtins` instead of as a member.

## Proposed contract

A descriptor asset may name a built-in schema instead of a file:

```maac
asset schema { kind = descriptor; builtin = "maac.production/1"; }
extension deliveries {
  namespace = "maac.production/1"; schema = &schema; render_affecting = true;
  data = { ... };
}
```

Rules:

1. An `asset` selects exactly one of two forms: `path` plus `hash`, which is
   the current form and unchanged, or `builtin` alone. Only
   `kind = descriptor` permits `builtin`. Mixing `builtin` with `path` or
   `hash` is an error, as is `builtin` on any other kind.
2. A `builtin` value is the exact namespace whose published schema the host
   embeds: `"maac.production/1"`, `"maac.takes/1"` or `"maac.takes/2"`. An
   unknown value is `E_REFERENCE` at the `builtin` field, as for an unknown
   built-in import. There are no aliases and no fallback.
3. A built-in descriptor may only be an extension's `schema`, and only for an
   extension whose `namespace` equals its `builtin` value. Any other use is
   `E_ASSET`.
4. The resolved identity is `@builtin/schema/<namespace>.json`, for example
   `@builtin/schema/maac.production/1.json`, with the SHA-256 of the embedded
   bytes. It sits in the reserved `@builtin/` namespace, so no source map,
   asset map or local path can replace it. Provenance, locks and archives
   record that identity the same way they record built-in imports.
5. The embedded bytes are the current schema files. A built-in schema is
   frozen by its namespace version, as library versions are. A changed schema
   needs a new namespace version.
6. Both forms are valid permanently. For the same extension they produce the
   same schema hash in execution and delivery identity. The source hash still
   differs because the source text differs.

No valid source changes meaning: every document that uses `builtin` on an
asset is rejected today. Under [§26](../MaaC-1-Specification.md#26-versioning-and-extensions)
this is an additive change within `maac 1`.

## Rejected alternatives

- **Omit `schema` for known namespaces.** This removes one more line, but
  §26 then has a required field that is sometimes optional, and the schema
  identity disappears from the source. Being able to read which contract
  validates `data` is worth one line.
- **A string `schema`** (`schema = "maac.production/1";`). This changes the
  type of an existing field from reference to reference-or-string, and repeats
  the namespace on the same object. The asset form keeps every schema an
  asset, so tools that list assets, locks and archives stay uniform.
- **Resolve asset paths from the declaring file.** This would fix the
  path-base mismatch for every asset, but it changes the meaning of valid
  nested sources. That needs a new language version.

## Acceptance

- [`examples/lofi/lofi.maac`](../examples/lofi/lofi.maac) switches to the
  built-in form and drops its copied schema. Its delivery WAV and
  `schema_hash` are byte-identical to the copied form.
- Existing sources, including
  [`examples/production.maac`](../examples/production.maac) and the takes
  examples, keep the copied form. Their renders, hashes, conformance vectors
  and saved artifacts are unchanged.
- `maac.takes/1` and `maac.takes/2` accept the built-in form. Their comp
  renders are byte-identical to the copied form.
- Negative tests, each with its code and location: `builtin` with `path`;
  `builtin` with `hash`; `builtin` on an audio or blob asset; an
  unknown built-in; a built-in that differs from the extension's namespace; a
  built-in descriptor referenced anywhere except an extension `schema`; and a
  source or asset map entry under `@builtin/schema/`.
- An editable archive of the lofi song round-trips. It lists the schema under
  `builtins`, never as a member, and rejects a tampered built-in hash.
- The editing kernel keeps the form stable, and a Protocol 2 transaction can
  rewrite the copied form into the built-in form.
- The [producer guide](ai-producer-guide.md) delivery step needs no copied
  file. [production.md](production.md), [grouped-takes.md](grouped-takes.md),
  §14.1, §26, [capabilities.md](capabilities.md), the CHANGELOG and the
  trial's F11 row describe the form.

## Not proposed

- Built-in descriptors for external processors (§17). Their descriptors
  describe a third-party module and stay pinned files.
- Changing how any path resolves.
- Upgrading existing sources automatically. One Protocol 2 transaction
  rewrites the copied form into the built-in form when an author wants it.

## Decisions

1. **Form.** Use `builtin` on a descriptor asset, mirroring built-in
   imports. *Recommended.* The alternatives are above.
2. **Identifier.** The `builtin` value is the extension namespace itself, such
   as `"maac.production/1"`, not a separate schema ID. *Recommended:* one name
   per contract, and rule 3 can check it directly.
3. **Scope.** Cover `maac.production/1`, `maac.takes/1` and `maac.takes/2`
   together. *Recommended:* the takes extensions run through the same
   descriptor code in `src/bundle.rs`, and leaving them out would make the
   same field behave in two ways.
4. **Examples.** Switch only the lofi song and the producer guide. Keep
   `examples/production.maac` and the takes examples in the copied form, so
   that both forms stay exercised. *Recommended.*
5. **Delivery.** One part on a topic branch: language, bundle, production,
   takes, archive, editing, docs. *Recommended:* the change is small, and a
   half-landed form would fail archive round-trips.

## Results

Implemented in one part, as decided.

- `src/extensions.rs` lists the three built-in schemas and checks the form.
  The bundle resolves it without reading a file, and records its identity in
  `ResolvedBundle::builtin_schemas`. Production, takes, semantic validation
  and archives accept it; the Python reference checker
  (`check_production.py`) does too.
- "Late Window" delivers the same WAV in both forms
  (`sha256:1bf87bbc…89d1cc`), with the same `production_schema` hash and
  `check_status` `pass`. Only hashes of the source text differ.
- `examples/take-comp.maac` and `examples/grouped-takes.maac` build
  byte-identical WAVs in both forms.
- The "Late Window" archive lists `@builtin/schema/maac.production/1.json`
  under `builtins` and has one member, the source. It verifies and unpacks offline.
- Tests: `tests/builtin_schemas.rs`, the located cases in
  `tests/extension_diagnostics.rs`, an archive unit test, and
  `tests/editing_bundle.rs`.

Two acceptance items changed during implementation. There is no `maac fmt`
command, so the editing kernel stands in for it. Module artifacts package
libraries only, and a library cannot declare an extension, so they never meet
a built-in descriptor.
