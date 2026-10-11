# ADR 0001: `sef-canonical-value-v1`

Status: accepted and frozen for Phase 1A by explicit user authorization.

This decision fills the typed-value representation left open by Part III.B of
`docs/03_MODEL_AND_QUERY_IR.md`. It does not change `sef-dcbor-sha256-v1`, implement
SQIR or SEIR, or establish an external interchange/API format. References to their
domain identifiers reserve collision separation only.

## Decision and version boundaries

Canonicalization has two explicit steps: a semantic object projects to the SEF
typed value tree below; the tree encodes to deterministic CBOR. Neither Rust memory
layout, field declaration order, `Debug` output, Serde derives nor JSON defines
either contract. `Canonical` implementations explicitly choose their field names,
tags, optional fields and collection semantics. A Serde-capable CBOR writer may
encode the already constructed tree without defining that tree.

The canonical format identifier is `sef-canonical-value-v1`; the fingerprint
profile is `sef-dcbor-sha256-v1`; the semantic and CSMIR profiles are both `0.1`.
Changing the representation below requires a new canonical format version.
Changing a semantic projection, tag, field name or included meaning requires the
affected semantic/IR profile version to change. Rust-only refactors preserve bytes.
The representation and Phase 1A projections are frozen at completion of this block.

## Typed values and CBOR

Every value is a definite-length CBOR array. Numbers in the first position are
SEF discriminants, **not** CBOR semantic tags. Payloads shown as CBOR primitives
are unwrapped; `V` means a recursively encoded SEF typed value.

| SEF value | Exact CBOR data-model form |
| --- | --- |
| Null | `[0]` |
| Boolean | `[1, boolean]` |
| Signed integer | `[2, integer]` |
| Unsigned integer | `[3, integer]` |
| Finite float | `[4, float]` |
| Byte string | `[5, bytes]` |
| UTF-8 text | `[6, text]` |
| Ordered sequence | `[7, [V, ...]]` |
| Set | `[8, [V, ...]]` |
| Map | `[9, {text: V, ...}]` |
| Named record/union variant | `[10, text_tag, {text: V, ...}]` |
| Absent optional value | `[11]` |
| Present optional value | `[12, V]` |

Only these SEF discriminants are defined in v1. Encoding uses RFC 8949 section
4.2.1 core deterministic encoding: shortest argument lengths, definite lengths,
and the shortest IEEE 754 width that represents a float exactly. No CBOR bignum,
decimal-fraction, date/time or other semantic tags are emitted.

Signed integers range from `-2^63` through `2^63-1`; unsigned integers range from
`0` through `2^64-1`. A nonnegative signed integer still has SEF tag 2 and therefore
differs from tag 3. Negative CBOR integers use argument `-1-n`. Declared semantic
integer types may carry their own bit width; the literal integer variants in this
block are 64-bit. Floats reject NaN and either infinity, preserve every other
finite value exactly, and normalize both signs of zero to positive zero.

Text is valid UTF-8, with exact bytes preserved. There is no Unicode normalization,
case folding, trimming or locale conversion. Empty text and empty byte strings
are distinct. A code-point-equivalent composed/decomposed spelling remains distinct.

Map keys are raw CBOR text, not tag-6 values. Duplicate keys are errors. Encode
each key in preferred CBOR form, then sort by the unsigned lexicographic order of
those complete encoded bytes. This differs from sorting Rust strings: `"b"`
precedes `"aa"`. Values and record fields follow the same map rule. A missing key
differs from a present key containing `[11]`, which differs from `[0]` or
`[12, [0]]`. In a fixed semantic record, every declared optional field is emitted:
`None` is `[11]`, `Some(x)` is `[12, V(x)]`. Non-semantic fields are omitted entirely.

Variant tags match `[A-Za-z0-9_][A-Za-z0-9_-]*(\.[A-Za-z0-9_][A-Za-z0-9_-]*)*`.
They are exact, case-sensitive identifiers chosen explicitly by semantic projection
code, not Rust type names or numerical Rust enum discriminants. Examples include
`compiled-semantic-model`, `conformance.exact-domain` and `csmir.Decimal`.
Unit enums use a type tag and a `variant` text field (for example
`[10, "aggregation-class", {"variant": [6, "fully-additive"]}]`). Payload unions
use distinct case tags and explicitly named payload fields. Empty records have an
empty map; no enum/union case is inferred from payload shape. Domain separation
is supplied by the envelope below, so record tags need not carry a domain prefix.

## Exact decimals

A decimal value is the record `csmir.Decimal` with exactly these three fields:

| Field | SEF value |
| --- | --- |
| `coefficient` | Text containing a normalized signed base-10 integer |
| `precision` | Unsigned integer `1..76` |
| `scale` | Unsigned integer `0..precision` |

Its mathematical value is `coefficient * 10^(-scale)`. The coefficient is `0` or
`-?[1-9][0-9]*`; the minus sign is ASCII `-`. There is no leading `+`, leading zero,
negative zero, exponent, decimal point, whitespace or locale punctuation. Its
absolute value fits the declared precision. Construction may accept a leading
plus/minus and leading zeros, but removes them before canonical projection.

Precision and scale are part of the semantic type and are preserved. Thus typed
`1.20` at `(3,2)` differs from typed `1.2` at `(2,1)`; normalization never removes
trailing coefficient zeros or changes the declared type. Up to 76 digits are
encoded as text without conversion through binary floats or machine integers.
The exact golden for coefficient `1234`, precision 4, scale 2 is below.

## Sets and sequences

For each set member, obtain its canonical typed-value CBOR bytes `B` and raw
`SHA-256(B)`. Sort ascending by the unsigned hash bytes, then by `B` as a collision
tie-break. Remove only adjacent **byte-identical** members; never equate members
merely because hashes match. This child ordering hash has no domain envelope and
is not an object content fingerprint. The set's enclosing value and eventual
artifact fingerprint provide the format and domain separation.

The model's object collections, policy/dependency reference collections, grain
atoms, axis-rule collections, conformance bindings and relationship key-pair
conjunctions are set-like. Semantic validation separately rejects duplicate object
identities and other forbidden duplicates before a compiled model is accepted.
Set canonicalization does not excuse invalid model definitions.

Sequence order is preserved, including duplicate members. Expression operands,
function arguments, case branches, struct fields, composite-key component fields,
temporal ordering terms and window ordering terms are sequences. Changing their
order can change meaning and must change their encoding. No commutative algebra
rewrite is implied by canonicalization.

The executable projection catalog is `src/canonical_projection.rs` alongside the
explicit domain implementations in `model.rs`, `relationships.rs` and `policy.rs`
within `crates/sef-model-ir`. Numeric metric casts use the distinct
`metric-expr.explicit-cast` tag with `operand` and optional `rounding` fields.
They preserve units; the surrounding metric expression includes its resolved
`value_type` and optional `unit`. Neither casts nor ordered role qualifiers imply
query binding or relationship-path analysis.

## Hash envelope and metadata

An artifact fingerprint hashes precisely this definite-length CBOR array:

```text
[
  "sef-canonical-value-v1",
  "sef-dcbor-sha256-v1",
  DOMAIN,
  "0.1",                     # semantic profile
  "0.1",                     # domain IR profile
  V                          # typed semantic value, embedded, not byte-string wrapped
]
```

The first five entries are raw CBOR text. `DOMAIN` is exactly `CSMIR`, `SQIR` or
`SEIR`. Phase 1A creates CSMIR artifacts only. The digest is SHA-256 of the full
envelope, displayed as `sef-sha256-v1:` followed by 64 lowercase hexadecimal
characters. Parsing rejects alternate prefixes, uppercase hex and wrong lengths.
The unwrapped value bytes and their raw digest in the first table below are
encoding test vectors, not artifact content hashes.

Semantic projections include stable IDs, semantic versions, logical source and
field handles, types, units, nullability, expressions, declared grains,
relationships/cardinality, temporal rules, policy definitions and context-slot
declarations, explicit conversions and semantic dependencies. The `metadata`
field of an object projects only its identity. Changing a governed semantic
version changes its content fingerprint. Computed content hashes are not inserted
into their own input.

Descriptions, labels, source spans, opaque non-semantic extension payloads,
trace/request IDs, telemetry, runtime context **values**, runtime verification
evidence and operational provenance are excluded. The declared cardinality or
uniqueness contract remains included; whether the same contract is currently
`DeclaredUnverified` or `Verified` does not change its semantic definition.
`Invalid` evidence is subject to structural validation where applicable; exclusion
from a hash is not permission to compile an invalid contract. Semantic extensions
must have an explicit future contract rather than hide meaning in excluded data.

## Golden vectors

Hex is lowercase and has no whitespace. Hashes were checked independently using
.NET SHA-256 over the listed byte literals. The executable companion is
`crates/sef-model-ir/tests/canonical_vectors.rs`; it also covers integer boundaries,
preferred float widths, exact Unicode, malformed inputs, resource bounds and
ordering properties.

| Typed value | CBOR bytes (hex) | Raw SHA-256 of listed bytes |
| --- | --- | --- |
| Null | `8100` | `9037f92bc91fdcc764a89d9144c341d887ffc83351cc8cf0530555c89920a47d` |
| True | `8201f5` | `18f2627553e5578fa1d8ae10bbb73bee3353bf1287c5072bd95cb32e87dea928` |
| Signed 0 | `820200` | `95397da2338a6e41e75d2a34be802255e05cb6e7d79088fcd6a0d9f99b344681` |
| Unsigned 0 | `820300` | `c1446d769d6d013454d8705a7e9fee7a3102f17248e22fa611b6ac49baf00df0` |
| Signed -24 | `820237` | `23ac1804f253cf5325a174ff777429343aaa19fe6245b528b154090e5f75df4a` |
| Bytes 00 01 | `8205420001` | `0c2e0bbcbb8f07f1f9b9c25d6de59f5f94d18ad93e8afc21a4c00f7d44f2dd18` |
| Text U+00E9 | `820662c3a9` | `6ddcc935b353f6db0eae874b43f9d92acff124713da2da3b8453ec98679b9c8a` |
| Sequence signed [0,1] | `820782820200820201` | `2c881897b5391bb0411d48121893360732492c0f257117a22068083e2c6f7936` |
| Set signed {0,1,0} | `820882820201820200` | `6b3647c2343afa7c95c6e2360a6951c2df9141c23fad3625216950261c14b1e1` |
| Map aa: signed 2, b: signed 1 | `8209a26162820201626161820202` | `5162ad841658fcda07b2405003c23035e98f1eeff6ef2c04d9872a3682f3f5ec` |
| Empty csmir.Empty variant | `830a6b63736d69722e456d707479a0` | `dc570052840e7123aa6def574579e13061a78fafed865b1b8fda8565255a05e3` |
| Absent | `810b` | `e92f5db43e501345e6b4b255bc299df49d038356f6244f5f51ecf58697f89de4` |
| Present Null | `820c8100` | `a573f6611f38841de51c63e58cb9f90d6865d310a1a5f0523521d8db7408c96a` |
| Float 1.5 | `8204f93e00` | `dd82f0824a7dcf6140fa9b7a9089a0f68deaa7a462c737802ed8a70fef3e23cb` |
| Decimal 1234/(4,2) | `830a6d63736d69722e446563696d616ca3657363616c6582030269707265636973696f6e8203046b636f656666696369656e7482066431323334` | `a37b7f3e31290afc81791bf9fb5c75cc614865a40956e25d1d293372a87f6b15` |

The following are complete envelope bytes and actual artifact fingerprints for
the Null typed value. The SQIR and SEIR rows test only reserved domain separation.

The identity projection also has independently assembled preferred CBOR bytes:
`830a6f6f626a6563742d6964656e74697479a269737461626c655f6964820673746573742f6d65747269632f726576656e75657073656d616e7469635f76657273696f6e820665312e302e30`
for stable ID `test/metric/revenue`, semantic version `1.0.0`. Its raw SHA-256 is
`ddddc912e07b7c853f93720b2e3f1c5e10e31bf0ea2e74b9242ebef49901ec97`.
`tests/csmir-golden-hashes.txt` freezes all 21 corpus model artifact fingerprints;
these regression baselines are generated from the compiled definitions, while the
primitive/identity byte vectors are independently derived. These model hashes
are not query-result certification.

| Domain | Complete hashed bytes (hex) | SHA-256 (append to `sef-sha256-v1:`) |
| --- | --- | --- |
| CSMIR | `86767365662d63616e6f6e6963616c2d76616c75652d7631737365662d6463626f722d7368613235362d76316543534d495263302e3163302e318100` | `ba2e3af3f16f5cd246769aaa2e05f89234f6db6cea414cb110c2dfd0a2260e33` |
| SQIR | `86767365662d63616e6f6e6963616c2d76616c75652d7631737365662d6463626f722d7368613235362d7631645351495263302e3163302e318100` | `fa8825bcc4f502bf9c7cf6c1736cd72026ba111627a1ce120eeb57327ac2ef57` |
| SEIR | `86767365662d63616e6f6e6963616c2d76616c75652d7631737365662d6463626f722d7368613235362d7631645345495263302e3163302e318100` | `3ff60aa698adbceca20b5c3c8d186a7e8bf9c3d53b636817c02ef42067dcc510` |

## Implementation limits

The Phase 1A encoder rejects typed trees deeper than 128 edges, more than 100,000
typed nodes, or more than 16 MiB of cumulative text/byte payload (including map
keys and variant tags). Checks apply before set deduplication. These are defensive
resource limits, not alternative encodings. Rejected inputs return structured
errors. This ADR defines no external parser, persistence transport or automatic
compatibility fallback.
