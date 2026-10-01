#!/usr/bin/env python3
"""Grammar-driven MaaC/1 surface-syntax corpus (evidence row D01, spec §2).

Cases are generated from the token classes and productions of
``grammar.lark``: every alternative of every terminal, at and just past its
boundaries, in each value position. The oracle is independent of the Rust
parser: Lark parses each case with ``grammar.lark`` and the ``check_spec.py``
transformer builds the typed syntax tree (§20.1). Rules that §2 states outside
the context-free grammar are applied explicitly by ``spec_rules`` below.

Run from the repository root:

    python3 scripts/lexical_corpus.py --write    # regenerate the corpus
    python3 scripts/lexical_corpus.py --verify   # fail on any drift

``tests/lexical_corpus.rs`` checks that ``maac::parse`` agrees with every
recorded expectation.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Iterator

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from check_spec import ToSyntax  # noqa: E402  (reference transformer)
from lark import Lark  # noqa: E402
from lark.exceptions import LarkError, VisitError  # noqa: E402

CORPUS = ROOT / "tests" / "fixtures" / "lexical" / "corpus.json"
SCHEMA = "maac.lexical-corpus/1"
MAX_IDENTIFIER_BYTES = 128

PARSER = Lark((ROOT / "grammar.lark").read_text(), parser="lalr", maybe_placeholders=False)


def identifiers(value: Any) -> Iterator[str]:
    """Every identifier token in a typed syntax value."""
    tag = value["t"]
    if tag == "symbol":
        yield value["v"]
    elif tag == "ref":
        yield from value["path"]
        if value["port"] is not None:
            yield value["port"]
    elif tag == "call":
        yield value["fn"]
        for item in value["args"]:
            yield from identifiers(item)
    elif tag in ("list", "tuple"):
        for item in value["items"]:
            yield from identifiers(item)
    elif tag == "record":
        for name, item in value["fields"].items():
            yield name
            yield from identifiers(item)


def object_identifiers(object_id: str, body: dict[str, Any]) -> Iterator[str]:
    yield body["kind"]
    yield object_id
    for name, value in body["fields"].items():
        yield name
        yield from identifiers(value)
    for child_id, child in body["children"].items():
        yield from object_identifiers(child_id, child)


def spec_rules(document: dict[str, Any]) -> str | None:
    """§2 rules outside the context-free grammar, as error codes.

    - Identifier length is at most 128 ASCII bytes. §2 names no code; the
      bound is a published hard limit, so it is E_RESOURCE_LIMIT (§23).
    """
    for object_id, body in document["objects"].items():
        for name in object_identifiers(object_id, body):
            if len(name.encode()) > MAX_IDENTIFIER_BYTES:
                return "E_RESOURCE_LIMIT"
    return None


def oracle(source: str) -> dict[str, Any]:
    try:
        tree = PARSER.parse(source)
    except LarkError:
        return {"error": "E_SYNTAX"}
    try:
        document = ToSyntax().transform(tree)
    except VisitError as error:
        message = str(error.orig_exc)
        if message.startswith("Unsupported language version"):
            return {"error": "E_VERSION"}
        for code in ("E_DUPLICATE_FIELD", "E_DUPLICATE_ID", "E_SYNTAX"):
            if message.startswith(code):
                return {"error": code}
        raise
    code = spec_rules(document)
    if code is not None:
        return {"error": code}
    return {"syntax": document}


def value_case(name: str, value: str) -> tuple[str, str]:
    """A value in field position of one object."""
    return name, f"maac 1;\nthing t {{ v = {value}; }}\n"


def cases() -> Iterator[tuple[str, str]]:
    # Header: the literal `maac`, an INT version, and `;`.
    for name, source in [
        ("header/minimal", "maac 1;"),
        ("header/leading-whitespace-and-comments", " \t\n// c\n/* b */ maac 1;"),
        ("header/leading-zero-version", "maac 01;"),
        ("header/version-2", "maac 2;"),
        ("header/version-0", "maac 0;"),
        ("header/signed-version", "maac +1;"),
        ("header/fraction-version", "maac 1/1;"),
        ("header/missing-semicolon", "maac 1 thing t {}"),
        ("header/joined-version", "maac1;"),
        ("header/tab-separator", "maac\t1;"),
        ("header/comment-separator", "maac/* c */1;"),
        ("header/keyword-prefix", "maacx 1;"),
        ("header/uppercase-keyword", "MAAC 1;"),
        ("header/missing", "thing t {}"),
        ("header/empty-source", ""),
        ("header/twice", "maac 1; maac 1;"),
    ]:
        yield name, source

    # Identifiers: [A-Za-z_][A-Za-z0-9_]* and at most 128 bytes.
    for name, ident in [
        ("letter", "a"),
        ("underscore", "_"),
        ("underscore-digits", "_9"),
        ("mixed-case", "AbC_z9"),
        ("keyword-maac", "maac"),
        ("keyword-true", "true"),
        ("keyword-false", "false"),
        ("max-length", "x" * MAX_IDENTIFIER_BYTES),
        ("over-max-length", "x" * (MAX_IDENTIFIER_BYTES + 1)),
    ]:
        yield f"identifier/object-id/{name}", f"maac 1; thing {ident} {{}}"
        yield f"identifier/kind/{name}", f"maac 1; {ident} t {{}}"
        yield f"identifier/field/{name}", f"maac 1; thing t {{ {ident} = 1; }}"
    for name, ident in [
        ("leading-digit", "9a"),
        ("hyphen", "a-b"),
        ("dot", "a.b"),
        ("non-ascii", "café"),
        ("dollar", "a$"),
    ]:
        yield f"identifier/object-id/{name}", f"maac 1; thing {ident} {{}}"
        yield f"identifier/field/{name}", f"maac 1; thing t {{ {ident} = 1; }}"
    yield "identifier/case-sensitive-ids", "maac 1; thing a {} thing A {}"
    # Pitch, unit and boolean spellings in identifier positions.
    for name, ident in [
        ("pitch", "C4"),
        ("flat-pitch", "Bb3"),
        ("unit-q", "q"),
        ("unit-ms", "ms"),
        ("unit-frame", "frame"),
    ]:
        yield f"identifier/object-id/{name}", f"maac 1; thing {ident} {{}}"
        yield f"identifier/field/{name}", f"maac 1; thing t {{ {ident} = 1; }}"
        yield f"identifier/record-field/{name}", f"maac 1; thing t {{ v = {{ {ident} = 1; }}; }}"
        yield f"identifier/reference/{name}", f"maac 1; thing t {{ v = &{ident}; }}"
    for name, ident in [("sharp-pitch", "C#4"), ("signed-pitch", "C-1")]:
        yield f"identifier/object-id/{name}", f"maac 1; thing {ident} {{}}"
        yield f"identifier/field/{name}", f"maac 1; thing t {{ {ident} = 1; }}"
        yield f"identifier/reference/{name}", f"maac 1; thing t {{ v = &{ident}; }}"
    for name, ident in [("true", "true"), ("maac", "maac")]:
        yield f"identifier/record-field/{name}", f"maac 1; thing t {{ v = {{ {ident} = 1; }}; }}"
        yield f"identifier/reference/{name}", f"maac 1; thing t {{ v = &{ident}; }}"
        yield f"identifier/call/{name}", f"maac 1; thing t {{ v = {ident}(1); }}"

    # NUMBER: optional sign, then fraction | decimal | integer.
    for sign_name, sign in [("unsigned", ""), ("plus", "+"), ("minus", "-")]:
        for name, number in [
            ("integer", "7"),
            ("zero", "0"),
            ("leading-zeros", "007"),
            ("decimal", "0.10"),
            ("decimal-trailing-zeros", "2.500"),
            ("fraction", "3/4"),
            ("fraction-unreduced", "6/8"),
            ("fraction-zero-numerator", "0/5"),
            ("large-integer", "123456789012345678901234567890"),
        ]:
            yield value_case(f"number/{sign_name}/{name}", sign + number)
    for name, number in [
        ("leading-dot", ".5"),
        ("trailing-dot", "5."),
        ("zero-denominator", "1/0"),
        ("leading-zero-denominator", "1/01"),
        ("signed-denominator", "1/-2"),
        ("decimal-fraction", "1.5/2"),
        ("fraction-decimal-denominator", "1/2.5"),
        ("exponent", "1e3"),
        ("exponent-decimal", "1.0e3"),
        ("underscore-separator", "1_000"),
        ("hex", "0x10"),
        ("double-sign", "--1"),
        ("sign-space", "- 1"),
        ("fraction-spaces", "1 / 2"),
        ("double-decimal-point", "1.2.3"),
    ]:
        yield value_case(f"number/invalid/{name}", number)

    # Words that name non-finite numbers are ordinary symbols, not numbers.
    for name, value in [("nan", "NaN"), ("infinity", "inf"), ("negative-infinity", "-inf")]:
        yield value_case(f"number/word/{name}", value)

    # Quantities: NUMBER immediately followed by a UNIT alternative.
    units = ["frame", "bpm", "kHz", "Hz", "ms", "ct", "dB", "q", "s"]
    for unit in units:
        for name, number in [("integer", "4"), ("decimal", "1.5"), ("fraction", "-1/3")]:
            yield value_case(f"quantity/{unit}/{name}", number + unit)
        yield value_case(f"quantity/{unit}/space-before-unit", "4 " + unit)
        yield value_case(f"quantity/{unit}/uppercase", "4" + unit.upper())
        yield value_case(f"quantity/{unit}/doubled", "4" + unit + unit)
        yield value_case(f"quantity/{unit}/bare-unit", unit)
    for name, value in [
        ("lowercase-hz", "4hz"),
        ("lowercase-khz", "4khz"),
        ("uppercase-db", "4DB"),
        ("unknown-unit", "4px"),
        ("unit-then-digit", "4q2"),
        ("ms-prefix-of-identifier", "4msx"),
        ("s-prefix-of-identifier", "4sec"),
        ("q-prefix-of-identifier", "4quarter"),
    ]:
        yield value_case(f"quantity/invalid/{name}", value)

    # PITCH: [A-G], optional #|##|b|bb, optional sign, octave digits.
    for letter in "ABCDEFG":
        yield value_case(f"pitch/letter/{letter}", f"{letter}4")
    for name, accidental in [("sharp", "#"), ("double-sharp", "##"), ("flat", "b"), ("double-flat", "bb")]:
        yield value_case(f"pitch/accidental/{name}", f"C{accidental}4")
    for name, pitch in [
        ("negative-octave", "C-1"),
        ("plus-octave", "C+4"),
        ("multi-digit-octave", "C10"),
        ("sharp-negative-octave", "B#-1"),
        ("flat-negative-octave", "Cb-1"),
    ]:
        yield value_case(f"pitch/{name}", pitch)
    for name, pitch in [
        ("triple-sharp", "C###4"),
        ("triple-flat", "Cbbb4"),
        ("mixed-accidentals", "C#b4"),
        ("missing-octave", "C#"),
        ("letter-h", "H4"),
        ("lowercase", "c4"),
        ("trailing-letter", "C4x"),
        ("trailing-underscore", "C4_"),
        ("flat-trailing-letter", "Bb3y"),
        ("sharp-trailing-letter", "C#4x"),
        ("signed-trailing-letter", "C-1x"),
        ("sign-without-octave", "C-"),
        ("bare-letter", "C"),
    ]:
        yield value_case(f"pitch/other/{name}", pitch)

    # STRING: JSON strings without raw control characters or lone surrogates.
    for name, literal in [
        ("empty", '""'),
        ("plain", '"hello"'),
        ("unicode", '"café ♪"'),
        ("escape-quote", r'"a\"b"'),
        ("escape-backslash", r'"a\\b"'),
        ("escape-slash", r'"a\/b"'),
        ("escape-b", r'"\b"'),
        ("escape-f", r'"\f"'),
        ("escape-n", r'"\n"'),
        ("escape-r", r'"\r"'),
        ("escape-t", r'"\t"'),
        ("escape-unicode-lower", r'"é"'),
        ("escape-unicode-upper", r'"é"'),
        ("escape-null", r'"\u0000"'),
        ("surrogate-pair", r'"🎵"'),
        ("delete-character", '"\x7f"'),
        ("comment-markers", '"// /* */"'),
    ]:
        yield value_case(f"string/{name}", literal)
    for name, literal in [
        ("unterminated", '"abc'),
        ("escape-x", r'"\x41"'),
        ("escape-single-quote", r"'\''"),
        ("short-unicode-escape", r'"\u12"'),
        ("non-hex-unicode-escape", r'"\u12g4"'),
        ("lone-high-surrogate", r'"\ud800"'),
        ("lone-low-surrogate", r'"\udc00"'),
        ("reversed-surrogates", r'"\udfb5\ud83c"'),
        ("raw-tab", '"a\tb"'),
        ("raw-newline", '"a\nb"'),
        ("raw-nul", '"a\x00b"'),
        ("single-quotes", "'abc'"),
        ("trailing-backslash", '"abc\\"'),
    ]:
        yield value_case(f"string/invalid/{name}", literal)

    # Booleans are reserved words; similar identifiers are symbols.
    for name, value in [
        ("true", "true"),
        ("false", "false"),
        ("capitalized", "True"),
        ("prefix", "trueish"),
        ("underscore-suffix", "false_"),
    ]:
        yield value_case(f"boolean/{name}", value)

    # References: & path (. IDENT)* [: IDENT].
    for name, value in [
        ("single", "&a"),
        ("child", "&a.b"),
        ("deep", "&a.b.c.d"),
        ("port", "&a:out"),
        ("child-port", "&a.b:out"),
        ("params", "&a.params.level"),
        ("space-after-ampersand", "& a"),
        ("space-around-dot", "&a . b"),
        ("space-around-colon", "&a : out"),
    ]:
        yield value_case(f"reference/{name}", value)
    for name, value in [
        ("bare-ampersand", "&"),
        ("trailing-dot", "&a."),
        ("empty-port", "&a:"),
        ("two-ports", "&a:b:c"),
        ("leading-digit", "&1a"),
        ("dot-after-port", "&a:b.c"),
        ("double-ampersand", "&&a"),
        ("string-path", '&"a"'),
    ]:
        yield value_case(f"reference/invalid/{name}", value)

    # Calls, lists, tuples and records, with their trailing-comma rules.
    for name, value in [
        ("call/empty", "f()"),
        ("call/one", "key(60)"),
        ("call/trailing-comma", "key(60,)"),
        ("call/two", "degree(7, &t)"),
        ("call/nested", "ratio(3/2, 440Hz)"),
        ("call/unknown-name", "random()"),
        ("call/space-before-paren", "key (60)"),
        ("list/empty", "[]"),
        ("list/one", "[1]"),
        ("list/trailing-comma", "[1,]"),
        ("list/mixed", '[1, 2q, "s", &r, C4, (1, 2), [3], { a = 1; }]'),
        ("list/nested", "[[[]]]"),
        ("tuple/two", "(1, 2)"),
        ("tuple/trailing-comma", "(1, 2,)"),
        ("tuple/three", "(0q, 120bpm, step)"),
        ("record/empty", "{}"),
        ("record/one", "{ a = 1; }"),
        ("record/nested", "{ a = { b = [1]; }; }"),
    ]:
        yield value_case(f"compound/{name}", value)
    for name, value in [
        ("call/only-comma", "f(,)"),
        ("call/double-comma", "f(1,,2)"),
        ("call/leading-comma", "f(,1)"),
        ("call/unclosed", "f(1"),
        ("call/reference-name", "&f(1)"),
        ("list/only-comma", "[,]"),
        ("list/double-comma", "[1,,2]"),
        ("list/unclosed", "[1, 2"),
        ("list/semicolon-separator", "[1; 2]"),
        ("tuple/single", "(1)"),
        ("tuple/single-trailing-comma", "(1,)"),
        ("tuple/empty", "()"),
        ("record/missing-semicolon", "{ a = 1 }"),
        ("record/comma-separator", "{ a = 1, b = 2 }"),
        ("record/duplicate-field", "{ a = 1; a = 2; }"),
        ("record/bare-value", "{ 1; }"),
    ]:
        yield value_case(f"compound/invalid/{name}", value)

    # Comments and whitespace.
    for name, source in [
        ("line-comment-at-end-without-newline", "maac 1; thing t {} // end"),
        ("line-comment-between-tokens", "maac 1; thing t { v // c\n = 1; }"),
        ("block-comment-between-tokens", "maac 1; thing t { v /* c */ = /* d */ 1; }"),
        ("block-comment-multiline", "maac 1; /* a\nb\n*/ thing t {}"),
        ("block-comment-with-stars", "maac 1; /** a * b **/ thing t {}"),
        ("comment-markers-in-string", 'maac 1; thing t { v = "/* not a comment"; }'),
        ("tabs-and-crlf", "maac 1;\r\n\tthing t {\r\n\tv = 1;\r\n}\r\n"),
        ("form-feed", "maac 1;\fthing t {}"),
        ("no-whitespace", "maac 1;thing t{v=1;w=[1,2];}"),
    ]:
        yield f"trivia/{name}", source
    for name, source in [
        ("nested-block-comment", "maac 1; /* a /* b */ c */ thing t {}"),
        ("unterminated-block-comment", "maac 1; thing t {} /* open"),
        ("lone-slash", "maac 1; / thing t {}"),
        ("hash-comment", "maac 1; # c\nthing t {}"),
        ("vertical-tab", "maac 1;\vthing t {}"),
        ("non-breaking-space", "maac 1; thing t {}"),
    ]:
        yield f"trivia/invalid/{name}", source

    # Object and field structure.
    for name, source in [
        ("empty-object", "maac 1; thing t {}"),
        ("nested-objects", "maac 1; thing t { child c { grand g { v = 1; } } }"),
        ("fields-and-children", "maac 1; thing t { v = 1; child c {} w = 2; }"),
        ("many-top-level", "maac 1; a x {} b y {} c z {}"),
        ("same-child-id-under-different-parents", "maac 1; thing t { c k {} } thing u { c k {} }"),
    ]:
        yield f"structure/{name}", source
    for name, source in [
        ("semicolon-after-object", "maac 1; thing t {};"),
        ("missing-object-id", "maac 1; thing {}"),
        ("missing-field-semicolon", "maac 1; thing t { v = 1 }"),
        ("missing-field-value", "maac 1; thing t { v = ; }"),
        ("field-at-top-level", "maac 1; v = 1;"),
        ("unclosed-object", "maac 1; thing t {"),
        ("extra-close-brace", "maac 1; thing t {} }"),
        ("colon-field", "maac 1; thing t { v: 1; }"),
        ("duplicate-field", "maac 1; thing t { v = 1; v = 2; }"),
        ("duplicate-top-level-id", "maac 1; a x {} b x {}"),
        ("duplicate-child-id", "maac 1; thing t { c k {} d k {} }"),
        ("field-child-collision", "maac 1; thing t { k = 1; c k {} }"),
        ("child-field-collision", "maac 1; thing t { c k {} k = 1; }"),
    ]:
        yield f"structure/invalid/{name}", source


def build() -> dict[str, Any]:
    seen: set[str] = set()
    records = []
    for case_id, source in cases():
        if case_id in seen:
            raise SystemExit(f"duplicate case id {case_id}")
        seen.add(case_id)
        records.append({"id": case_id, "source": source, "expected": oracle(source)})
    return {
        "schema": SCHEMA,
        "oracle": "grammar.lark (Lark LALR) with the check_spec.py syntax transformer; "
        "identifier bytes > 128 are E_RESOURCE_LIMIT",
        "cases": records,
    }


def encode(corpus: dict[str, Any]) -> str:
    return json.dumps(corpus, indent=1, ensure_ascii=False) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true", help="regenerate the corpus")
    mode.add_argument("--verify", action="store_true", help="fail if the corpus drifted")
    args = parser.parse_args()
    text = encode(build())
    if args.write:
        CORPUS.parent.mkdir(parents=True, exist_ok=True)
        CORPUS.write_text(text)
        print(f"wrote {CORPUS.relative_to(ROOT)}")
    elif not CORPUS.exists() or CORPUS.read_text() != text:
        raise SystemExit(f"{CORPUS.relative_to(ROOT)} is stale; run --write and review the diff")
    else:
        print(f"{CORPUS.relative_to(ROOT)} matches grammar.lark")


if __name__ == "__main__":
    main()
