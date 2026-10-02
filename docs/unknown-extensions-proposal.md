# Unknown extensions: D16 decision

**Status:** accepted and implemented 2026-10-02 as P20. The owner chose
option **A** with `E_REFERENCE` for a namespace missing from
`project.requires`; see [Decision](#decision). The analysis below is kept as
written, with one correction marked.

**Question:** the [evidence map](document-performance-evidence.md#document-obligations)
marks D16 **U** for "unknown-extension semantic normalization". Should MaaC
add a general way to validate or normalize extensions it does not understand,
or is refusal the intended boundary?

## What the specification requires

- [§1.2](../MaaC-1-Specification.md#12-conformance-profiles): Document
  implementations parse, preserve, validate, normalize and edit core document
  types. "Unsupported required extensions must be reported." An implementation
  states its profiles and its supported extension identifiers.
- [§26](../MaaC-1-Specification.md#26-versioning-and-extensions): an
  `extension` has `namespace`, `schema` (a descriptor-asset reference),
  `render_affecting` and `data`, and its namespace must appear in
  `project.requires`. A host that does not understand it "may preserve it for
  Document-only inspection but must not claim semantic normalization or
  faithful rendering of that document", and must not trust an unknown schema's
  claim that data is non-rendering in order to omit it from a hash.
- [§20.2](../MaaC-1-Specification.md#202-semantic-normalization):
  the execution hash may exclude extension data only when an *understood*
  schema explicitly marks it nonexecuting.

So the specification already answers the main question: unknown extensions
are preserved and reported, never normalized or rendered. D16's **U** records
that the implementation has no general extension validator. That is a
capability the specification does not require, not a conformance gap.

## What the implementation does today

Three namespaces are recognized: `maac.production/1`, `maac.takes/1` and
`maac.takes/2`. Each pins exact schema bytes and has hand-written semantics.
Probing an `extension e { namespace = "org.example.notes/1"; ... }` gave:

| Boundary | Result | Assessment |
| --- | --- | --- |
| `parse`, `AuthoredDocument`, revision hash | Accepted and preserved | Meets §26 preservation |
| `semantic::validate_source` | `E_CAPABILITY` at `p`/`requires` and at `e` | Meets §1.2 reporting |
| Foundation and bundle edit contexts | `E_CAPABILITY` | Meets §26 (no normalization) |
| `maac check` / bundle compilation | `E_CAPABILITY` "unknown required extension" at path `production.namespace` | **Defect:** the location names the production module, not object `e` and field `namespace` (§23) |
| Namespace missing from `project.requires` | Unknown namespace: the same message as above. `maac.production/1`: `E_REFERENCE` "missing required field" at `requires` | **Defect:** the §26 rule is not reported as its own violation or at the extension |
| `production_identity::execution_identity` | Hashes unknown extension data verbatim if called directly | Not reachable through compilation, but the public function does not refuse |
| Supported extension identifiers (§1.2) | Stated in prose in [capabilities](capabilities.md) | No single source shared by the validator and the documentation |

## Options

### A — Close D16 at the specified boundary (recommended)

Keep refusal as the boundary and prove it. Concretely:

1. Report a non-recognized extension at its own object and `namespace` field
   with `E_CAPABILITY` in every boundary, including bundle compilation.
2. Report an extension whose namespace is missing from `project.requires` at
   the extension's `namespace` field, for every namespace. This proposal uses
   `E_REFERENCE`, which production already returns, so no existing code
   changes. *Correction:* production returned `E_REFERENCE` only when
   `project.requires` was absent; when the list existed without the namespace,
   production and takes both returned `E_CAPABILITY`. Either code therefore
   changes an existing diagnostic.
3. Make `execution_identity` refuse documents with non-recognized extension
   namespaces (`E_CAPABILITY`) instead of hashing them, so no public function
   claims a normalization §26 forbids. The unit test that uses an
   `unknown/1` extension to check payload preservation would use the
   production extension instead.
4. Keep one public list of supported extension identifiers, used by the
   validator, and check in a test that [capabilities](capabilities.md) states
   the same list.
5. Add a test target covering preservation (exact syntax tree and revision
   hash, which changes when `data` changes) and refusal at each boundary in the
   table, with §23 locations.

D16 would then be **E**: preservation, reporting and refusal are evidenced,
and the absent general validator is recorded as outside the specification.
Cost: small, about the size of P19. No grammar, wire format or normative text
changes, apart from noting the requires-check code if the specification should
name it.

### B — Structural validation against the pinned schema

Validate unknown extension `data` against its pinned `schema` asset and report
"well-formed, semantics not understood", while still refusing editing,
normalization and rendering.

- **Gain:** authors learn that their extension data is shaped correctly.
- **Needs:** a normative schema language for descriptor assets, which §26 does
  not define today. The existing production and takes schemas use JSON Schema
  2020-12 with `pattern`, `if`/`then`, `oneOf` and `allOf`. A conforming
  validator needs a regular-expression engine, and the build is offline with no
  regex crate. Alternatively, the language could be restricted to a subset
  without `pattern`.
- **Risk:** a "valid" result is easy to mistake for support.

Cost: medium to large, plus a specification amendment, for limited value.

### C — Opaque preservation during editing

Let editing and normalization proceed for documents that carry unknown
extensions, keeping their data verbatim:

- refuse edits inside the extension data;
- refuse renames or deletions of objects that its data references; the
  references are syntactically visible;
- never render.

- **Gain:** a tool can edit the notes of a document that also carries another
  tool's extension, for example notation. Today such a document cannot be
  edited at all.
- **Needs:** a specification amendment to §26. A host that does not understand
  the extension would hash its data verbatim, while one that does would apply
  its defaults. The same document would then have two execution hashes unless
  the opaque form is a separate algorithm context, for example
  `maac.execution.opaque.sha256/1`, with its own place in locks and Protocol 2
  preconditions.

Cost: large, with several design decisions. This is worth doing when a
concrete cross-tool extension exists; nothing in the repository needs it yet.

### D — Schema-declared normalization rules (not recommended)

Let an unknown extension's schema declare defaults and nonexecuting fields for
a generic host to apply. This directly contradicts §26: a host would be trusting
an unknown schema to decide what leaves the hash.

## Recommendation

Choose **A** now. Record **C** as the candidate if a cross-tool extension use
case appears, with the separate opaque algorithm context as its first
decision. Do not pursue **B** without a demonstrated need for generic
validation, or **D** at all.

## Decision

The owner chose **A**, with `E_REFERENCE` for a namespace missing from
`project.requires`. As implemented:

- [`maac::extensions`](../src/extensions.rs) holds the supported identifiers
  (`SUPPORTED`) and the §26 checks. The single-document validator, every
  bundle path (before takes and production preparation), and the foundation
  and bundle edit contexts use it. [Capabilities](capabilities.md) states the
  same list, and a test compares them.
- An unsupported namespace is `E_CAPABILITY`, and a namespace missing from
  `project.requires` is `E_REFERENCE`, both at the extension's `namespace`
  field. A recognized namespace that is not required now fails with
  `E_REFERENCE` there, before the production or takes checks run.
- `execution_identity`, and the label-retaining normalization used for edit
  preconditions, refuse documents with an unsupported extension namespace.
- [`tests/unknown_extensions.rs`](../tests/unknown_extensions.rs) covers
  preservation (authored tree, revision hash) and refusal at validation,
  bundle check and compilation, both edit contexts and execution identity,
  with §23 locations.
