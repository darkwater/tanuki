# System namespace and enums

Updated 2026-10-05.

## Reserved namespace — user decisions

Reserve `/$*` for Tanuki's built-in system facilities. The discussed interpretation is any path whose first segment begins with `$`, including descendants. Example paths:

- `/$system/connections`: connection count, an integer.
- `/$schemas/my_schema/...`: schema configuration.
- `/$clients/client_name/...`: earlier client-information example; exact role remains unspecified.
- `/$connections/conn_name/...`: latest chosen namespace for named managed connections/sessions (see 11-operational-decisions.md).

User schemas do not run for system paths at all. Schemas that mention/target system paths are invalid. This is a separate validation mechanism, not a built-in schema that user rules can override.

The system validator can return operation-specific errors, such as 'you cannot set properties of another connection', rather than a generic value/schema violation. Rules may depend on caller identity, operation, and current/candidate system state.

Assistant sketches (API and error names not selected):

```rust
fn validate_builtin_operation(
    caller: &Caller,
    operation: &Operation,
    candidate: &CandidateState,
) -> Result<(), SystemError>;

enum SystemError {
    CannotModifyOtherConnection,
    ReadOnlyProperty,
    InvalidSchemaDefinition,
    SchemaTargetsSystemNamespace,
}
```

Reserved does not necessarily mean read-only: core-owned statistics can be read-only, while designated schema/configuration properties can accept authorized writes. Exact permissions and namespace layout are not specified.

## Structural boundary questions still open

- Wildcard schema bindings such as `/**` can match system paths. Decide whether to reject such selectors or define schema selectors as operating only on the user namespace. Do not allow their expansion to invoke user schemas on system nodes.
- Links to system nodes must not make user-schema rules apply indirectly to core state, nor bypass built-in operation permissions. Whether such links are permitted and how this restriction is enforced remain open.
- 'Mentioning system paths' has not been formalized for future cross-node schema references. At minimum, bindings targeting them are forbidden; unrelated literal strings in ordinary data should not automatically be interpreted as references.
- Managed connection names have shared duplicate handling; internal session identity distinguishes replacement instances. Sessionless read-only requests need not appear under /$connections.

## Enums — assistant recommendation, not yet accepted

Keep enums in schemas, represented using existing runtime kinds. No `Value::Enum` is initially necessary.

Simple enums are constrained strings:

```rust
enum AssistantState {
    Idle,
    Listening,
    Thinking,
    Speaking,
}
```

An illustrative runtime value is the ordinary string `"listening"`. A schema restricts accepted strings to the declared variants. Without a schema, it remains a normal freeform string. Generic UIs can inspect the schema for choices.

Enums with payloads can be tagged records using ordinary maps:

```rust
enum PlaybackState {
    Stopped,
    Playing { position: Duration },
    Failed { message: String },
}
```

Illustrative values:

```json
{"kind": "stopped"}
{"kind": "playing", "position": {"$duration": "PT12.5S"}}
{"kind": "failed", "message": "Device unavailable"}
```

Here `kind` is an ordinary field, not a reserved wire-format type marker. The duration tag follows the existing unaccepted extended-JSON sketch. A schema validates both the discriminator and the fields permitted/required for that variant. The discriminator name, unknown-field policy, and enum tagging style are not selected.

Unknown variants follow normal validation policy (rejected under strict enforcement). Consumers can map validated values into language-native enums. A dedicated enum runtime kind would only be needed if variant identity must exist independently of schemas; this is not currently a requirement.
