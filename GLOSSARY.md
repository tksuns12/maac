# MaaC

MaaC describes a finite musical performance as typed, addressable source data
that tools edit and an engine renders. These terms name the parts of that
model; the [specification](MaaC-1-Specification.md) defines them normatively.

## Source identity

**Authored document (A)**:
A source exactly as written, as a canonical typed graph: IDs, labels, field
presence, units and constructors kept; comments and whitespace dropped.
_Avoid_: source tree, syntax JSON, raw document

**Semantic normalization (N(A))**:
The execution view of an authored document, with references resolved,
specified defaults expanded, execution units made canonical and pitch letters
and `bar` lowered. It never changes A.
_Avoid_: canonicalization, default expansion, normalized source

**Editing view**:
N(A) with display labels kept, the view in which a patch's expectations are
compared.
_Avoid_: editing normalization

**Revision hash**:
The SHA-256 of an authored document's canonical bytes. Writing a default out
changes it.
_Avoid_: source hash, document hash

**Execution hash**:
The SHA-256 of N(A)'s canonical bytes without display labels, and without the
fields the language gained after the hash was defined while they hold their
defaults. Writing a default out or leaving it implicit gives the same
execution hash.
_Avoid_: execution identity (the hash together with the N(A) bytes it covers),
normalized hash
