# Values, schemas, and encoding

Updated 2026-10-05. This file supersedes earlier suggestions of a separate topic type declaration, implicit nullable types, and an initial blob-transfer subsystem.

## Decisions and current direction

- Each topic holds one complete value. A string-keyed map never creates or spills into other topics. Nested field projection, if supported, is not topic creation.
- There are exactly two enforcement modes: freeform and schema-enforced. There is no intermediate type-locking stage.
- Freeform values still have concrete broad runtime kinds, such as integer or map. Their kind can change on subsequent writes; a minor warning is a possibility, not a hard error.
- A schema specifies permitted types and constraints together: for example an unsigned 8-bit integer or an integer in 0..=100.
- Earlier requirement: schemas strictly prevent invalid consumer-visible states and invalid writes report errors. The latest discussion considers reject/warn/cast policies and invalidating linked views; see 09-links-and-schemas.md. Whether warn is advisory-only or permits invalid visible values is unresolved. Strict guarantees must not be claimed for warn-and-accept policies.
- Decision: use `Value::Null` at runtime. Schemas determine whether absence is permitted (for example through Option<T>); runtime values do not carry Some wrappers. Semantic types such as timestamps remain distinct runtime kinds.
- Support JSON and binary encoding. MessagePack is the current binary choice; Postcard was an earlier candidate.
- Initial transport direction is WebSocket: text messages carry JSON and binary messages carry MessagePack. Later transports may include raw TCP and MQTT. Parse complete WebSocket messages; do not assume each wire fragment is a complete packet.
- Initial binary-data scope is small inline byte arrays. Direct transfer negotiation is deferred. No initial offers, stream subsystem, or managed blob storage is required.
- Consider timestamps and durations as useful first-class types.
- Decision: use ISO 8601 duration strings, compatible with the discussed Temporal-style notation. Exact supported units and runtime representation remain implementation questions; this does not select a separate span type.

## Runtime value sketch — proposal

```rust
enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    Timestamp(Timestamp),
    Duration(Duration),
}

struct Timestamp {
    unix_seconds: i64,
    nanos: u32, // invariant: 0 <= nanos < 1_000_000_000
}

struct Duration {
    nanos: i64, // signed, exact; roughly +/-292 years
}
```

All fields are illustrative. A production constructor must enforce timestamp normalization. Duration representation, signedness, range, unsigned integer support, non-finite float handling, are unresolved. The earlier elapsed-time-only duration sketch is under reconsideration: Temporal-compatible strings may represent calendar spans as well. The supported units and runtime representation are open. A timestamp identifies an instant, not a timezone or a local calendar date.

## Option, omission, and removal

Earlier terminology mixed two things: an absent record key and a present key with an absent optional value. Neither should make every type implicitly nullable.

Proposal: a schema can explicitly declare `Option<T>`; `T` itself always requires a value of that type. Omitted record keys need a separately chosen rule (reject, or normalize to None for Option fields). A missing topic or removed topic is not the same as a present topic containing Null.

Decision: Null represents absence; a non-null value represents the ordinary present value. The earlier explicit runtime Option proposal is superseded. This cannot distinguish None from Some(None); lossless nested options are not an initial requirement. Missing map keys and removed topics remain distinct from present Null values.

## Schema activation — proposal needed to uphold the invariant

Installing or tightening a schema must check existing affected state as well as future writes. Proposed default: reject activation if any existing value conflicts, leave the old configuration intact, and report offending paths. A future atomic migration could change values and schema together. Validation/activation must serialize with writes so no invalid state slips through a race.

Already delivered values cannot be retracted. The invariant applies to state exposed under the active schema; snapshots and live updates must respect the activation boundary. Invalid multi-topic batches should be rejected as a whole (proposal consistent with required atomicity).

## Encoding details

Reference: [MessagePack specification](https://github.com/msgpack/msgpack/blob/master/spec.md).

The specification defines binary values, a timestamp extension (type -1), and application-defined extensions (0..127). Duration would require a Tanuki convention; Null can use MessagePack nil. IDs for custom extensions are not selected.

The user's raw-transport idea discriminates JSON versus MessagePack by the first byte: ASCII JSON versus >0x7f MessagePack. This works with a constrained packet envelope (e.g. map/array roots); MessagePack positive fixints occupy 0x00..0x7f. The receiver must still reject invalid root shapes; not every byte above 0x7f is valid. Raw TCP message framing remains a separate decision. Whitespace/BOM policy and whether selection occurs once per connection or per packet remain open.

The user is open to making MessagePack preferred and JSON an imperfect convenience transport. Exact JSON fidelity is not a requirement. Proposal: tagged/escaped JSON representations for bytes, timestamp, and duration. A timestamp-shaped string alone is still a string. Tags must not collide with ordinary user map values. Exact layout is open; tagged envelopes or an escape convention can solve this.

JSON round-trip policy also needs decisions for exact large integers, floating-point values that look integral, and NaN/infinity. Do not silently narrow values to JavaScript number precision. MessagePack encoding widths should not automatically become stricter topic types.

## Questions for the next discussion

1. Settled: runtime Null; optionality belongs to schemas.
2. Missing record keys: reject or normalize to None only for declared Option fields?
3. Signed nanosecond durations versus a wider seconds/nanos representation?
4. Are UInt and non-finite floats useful enough to include?
5. JSON tagging and map escaping; small-byte and total-message size limits.
6. Schema activation conflicts and atomic migration semantics.


## Earlier minimal JSON extension proposal — not selected

Ordinary values use ordinary JSON. Reserve `$tanuki` in value objects for an explicit tagged representation:

```json
{"$tanuki": ["bytes", "AQID"]}
{"$tanuki": ["timestamp", "2026-10-05T00:00:00Z"]}
{"$tanuki": ["duration_ns", "1500000000"]}
```

These are three separate example values, not one JSON document. Byte strings use base64; timestamps use a specified RFC3339 profile; duration nanoseconds use a decimal string to avoid number rounding. Exact profiles and numeric bounds remain open.

An ordinary map containing the reserved key is escaped:

```json
{"$tanuki": ["map", {"$tanuki": "ordinary user data"}]}
```

For the map tag, interpret the payload's keys literally and recursively decode its values. Do not reinterpret that payload map itself as another tag. All other maps recursively decode values normally. Unknown or malformed tags produce decoding errors, rather than silently becoming ordinary maps.

This tagging applies to value positions, not arbitrary protocol-envelope objects. Decode extended JSON into runtime Values before schema validation. Do not infer timestamps from strings or bytes from integer arrays. This keeps freeform writes predictable.

Suggested compromise: make MessagePack the preferred faithful transport; keep JSON convenient for ordinary values plus a small set of explicit extensions. Exact integer sign/float-kind preservation and non-finite floats can be limited or require later explicit tags. Unsupported cases should fail or have documented conversion, not silently evade schemas.


## Temporal duration strings and direct JSON tags — current discussion

User suggestions: use the duration string format supported by JavaScript Temporal, consider Jiff on the server, and simplify JSON tags to objects such as `{"$timestamp": "..."}`. Possibly support such tags for all value kinds, including integers. ISO 8601 duration strings have since been accepted. Direct-tag syntax, full-type tagging, and Jiff implementation details remain proposals.

References:

- [Temporal.Duration documentation](https://tc39.es/proposal-temporal/docs/duration.html)
- [Jiff temporal formatting](https://docs.rs/jiff/latest/jiff/fmt/temporal/index.html)
- [Jiff SignedDuration](https://docs.rs/jiff/latest/jiff/struct.SignedDuration.html)

Temporal uses ISO 8601 duration strings, e.g. PT1.5S, PT5M, P1D, P1M. Jiff Span can represent calendar units; SignedDuration's ISO parsing accepts time units of hours or smaller. Calendar spans cannot in general be normalized to nanoseconds without a reference date and potentially a timezone. Decide whether Tanuki accepts the full span model or a fixed-duration subset; do not silently interpret P1M as minutes or P1D as exactly 24 hours in calendar arithmetic.

Assistant recommendation for discussion: support direct explicit tags for every runtime kind but allow ordinary JSON shorthand for ordinary values. This preserves simple scripts while offering explicit numeric typing and exact large integers.

```json
{
  "percentage": 72,
  "sequence": {"$int": "9223372036854775807"},
  "gain": {"$float": 1},
  "observed_at": {"$timestamp": "2026-10-05T00:00:00Z"},
  "timeout": {"$duration": "PT1.5S"},
  "thumbnail": {"$bytes": "AQID"}
}
```

Possible full-tag forms also include $null, $bool, $string, $list, and $map. Their exact payload rules are not selected. A fully tagged mode could preserve all types but is more verbose.

Proposed collision rule: any object with a dollar-prefixed key is reserved and must be exactly one recognized tag with a valid payload. Literal maps with such keys use `$map`; its payload's keys are literal while values are recursively decoded. This avoids future tag collisions and still permits arbitrary user keys.

```json
{"$map": {"$timestamp": "this is literal user data"}}
```

Tags live only in value positions; protocol envelope objects are parsed according to the protocol. Reject unknown/malformed tags. Numeric tag payloads require documented ranges and accepted string/number forms. Native MessagePack types and any extension mappings must agree with the same logical Value model.
