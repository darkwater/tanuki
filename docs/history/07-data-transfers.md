# Blob, offer, and stream sketches

Status: DEFERRED, 2026-10-05. The user rejected this complexity for the initial system. Initially support inline binary values for small data. Direct transfers, address/encryption negotiation, offers, streams, and managed blob storage below are historical proposals, not an implementation plan. Subscription sketches below remain unaccepted proposals. The Rust below is API pseudocode, not a compiled implementation or a selected wire format. Named IDs, timestamp types, and transport handles are placeholders.

## User direction

Think carefully about binary data and streams. Explore Wayland data sources as inspiration. Tanuki may facilitate direct connections instead of carrying the bytes through the core. A concrete use case is copying an image between machines. Suggestions should be expressed as Rust types and examples for review.

## Inspiration

[Wayland protocol specification: data offers and sources](https://wayland.freedesktop.org/docs/html/apa.html).

Wayland sources advertise MIME types; a receiver chooses a type and supplies a file descriptor, typically a pipe, through the protocol. The source writes the selected representation to that descriptor. This provides on-demand representation selection and transfer outside ordinary protocol message payloads. Network rendezvous, peer authentication, and reachability are additional Tanuki design problems; Unix file descriptors do not transfer between hosts.

## Proposed distinctions

- A blob identifies fixed, finite bytes with a defined retention lifetime.
- A finite offer advertises data a producer can generate or serve on demand, possibly in multiple representations. It depends on provider availability unless materialized.
- A live offer opens a stream. It has no immutable whole-object content identity and no necessary natural end.
- A transfer is one opened instance. The same offer might serve several readers, subject to policy.
- Route selection (local descriptor, direct network, relay) is independent of these meanings.

```rust
enum DataRef {
    Blob(BlobRef),
    Offer(OfferRef),
}

struct BlobRef {
    id: BlobId,
    media_type: String,
    size: u64,
}

struct OfferRef {
    id: OfferId,
    representations: Vec<Representation>,
    kind: OfferKind,
}

struct Representation {
    id: RepresentationId,
    media_type: String,
    size_hint: Option<u64>,
}

enum OfferKind {
    Finite,
    Live,
}

struct OpenRequest {
    offer: OfferId,
    representation: RepresentationId,
    route: RoutePreference,
}

enum RoutePreference {
    DirectOnly,
    PreferDirect, // may fall back to a relay
    RelayOnly,
}
```

These descriptors can be values in retained nodes. A retained descriptor does not mean the referenced bytes are stored or the producer is reachable. Changes in availability should be observable. IDs select resources and do not implicitly grant permission to fetch them.

## Clipboard example

```rust
// Producer: keep the offer registration alive to serve requests.
let offer = client.offer(finite_offer([
    representation("image/png"),
    representation("image/bmp"),
])).await?;

client.write("/desktop/clipboard", offer.reference()).await?;

while let Some(request) = offer.next_request().await {
    let mut writer = request.accept().await?;
    clipboard_image.encode_to(request.representation(), &mut writer).await?;
    writer.finish().await?;
}

// Consumer: choose a representation and open one transfer.
let mut reader = client.open(OpenRequest {
    offer: clipboard.id,
    representation: clipboard.find("image/png")?,
    route: RoutePreference::PreferDirect,
}).await?;
save_image(&mut reader).await?;
```

New clipboard content should create a new finite offer ID. An existing offer should not silently serve later clipboard contents; this proposal prevents a receiver requesting an old selection but getting a new image. Offer replacement, cancellation, and already-open transfers require explicit lifetime rules.

## Persistence example

```rust
// Proposal: explicitly copy one representation into managed blob storage.
let blob = client.materialize(
    clipboard.id,
    clipboard.find("image/png")?,
    KeepFor::Days(7),
).await?;
client.write("/clipboard/history/latest", blob).await?;
```

This can preserve content after the source goes offline. It is separate from retaining its topic descriptor. Proposal: finish the blob upload before publishing a reference that promises stored bytes. Atomic node writes atomically publish references, not all subsequent transfers. Decide whether references pin blobs or carry explicit expiry; do not assume node retention supplies storage retention automatically.

## Live streams

A microphone provider could advertise a live audio representation and open a stream on demand. Decide whether each open creates a new source, joins a shared live feed, or acquires an exclusive device. For fan-out, slow consumers require an explicit buffering/drop/disconnect policy.

An ordered byte stream is a useful first transport abstraction for files and some audio uses. Timed frames or datagrams may be needed later for real-time media. Do not equate an infinite blob with a complete media-stream protocol. Bidirectional sessions are a possible later extension; no requirement has been selected.

## Direct transfer negotiation — proposal

1. Consumer asks Tanuki to open a particular offer and representation.
2. Tanuki checks access and contacts the provider.
3. A transfer is created with short-lived, transfer-specific authorization and an agreed route. Peers authenticate the authorized counterpart, rather than trusting a resource ID or advertised address alone.
4. On the same host, a local agent could broker an FD. Across machines, negotiate a direct authenticated connection. A relay can handle cases without a usable direct route.
5. Bytes flow separately from state subscription traffic.

This does not require adopting full Internet NAT traversal initially. A reachable LAN/VPN route plus relay fallback is one proposed scope. Peer disappearance, cancellation, timeout, and integrity/completion must be surfaced as transfer outcomes. Cancellation cannot retract bytes already delivered.

## Subscription sketches for feedback

```rust
enum SubscriptionMessage {
    Snapshot { topics: Vec<TopicSnapshot> },
    Update { topics: Vec<TopicChange> },
    Lagged, // requires resubscription; missed instant events are not recovered
}

struct TopicChange {
    topic: String,
    change: Change,
}

enum Change {
    Present(TopicSnapshot),
    Removed { previous: TopicSnapshot },
    Event { value: Value, metadata: Metadata },
}

enum ShapeState<T> {
    Ready(T),
    Unavailable { problems: Vec<FieldProblem> },
}

enum FieldProblem {
    Missing { path: String },
    Invalid { path: String, message: String },
}
```

An initial snapshot establishes the complete retained view; a client applies an entire update before evaluating a shape. Optional Rust fields can represent expected absence; required missing fields make the shape unavailable. Event occurrences are delivered explicitly rather than folded into a latest-value cache. Subscription IDs/revisions can be added when deciding multiplexing/recovery; the sketches intentionally do not settle them.

## Next decisions

1. Is the distinction between stored blobs, finite offers, and live offers useful?
2. Must finite offers be repeatable and immutable per representation, or can some be single-use?
3. What outlives provider disconnect or offer replacement? What outlives node expiry?
4. Is managed storage optional, and who materializes data for persistence?
5. Should an open return only a readable stream initially, or support duplex channels/framed media?
6. Which direct routes and relay behaviour are needed in the first implementation?
