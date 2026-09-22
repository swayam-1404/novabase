# Optional Recommendation Assistant (Phase 21)

Phase 21 adds a provider-neutral explanation boundary to the Nova Intelligence
Engine. It can operate with no provider, returning a deterministic evidence
summary and a valid NovaQL `create index` suggestion. Applications may inject
an `ExplanationProvider` to add plain-language narrative.

The provider receives only validated collection/path names, aggregate counts,
ratios, an optional aggregate impact evaluation, and the suggested command. It
never receives documents, query literals, credentials, tokens, or raw query
text. Input context and output sizes are bounded. Empty and oversized output,
unsafe schema identifiers, invalid limits, and provider failures return typed
errors.

Provider prose is untrusted, display-only text. NovaDB never parses or executes
it, and the assistant cannot change an index, recommendation state, plan,
transaction, recovery decision, authorization result, or stored data. A human
must review and explicitly run any suggested NovaQL command.
