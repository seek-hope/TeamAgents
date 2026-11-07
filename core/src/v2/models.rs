//! R2 v2 schema identity and request-kind constants. Runtime facts are read
//! through explicit SQL/JSON projections, so this module deliberately keeps
//! no second set of mirror structs or status enums.

pub const V2_FORMAT_ID: &str = "teamagents-v2";
/// 1 → 2 (R22/A20): `context_entries.compressed_by` and `model_requests.kind`
/// (compression calls are billed requests that never advance an instance).
/// 2 → 3 (D-71): the runtime's own closing notes (`work-close-`, `work-block-`,
/// `turn-close-`) move from the member's `assistant` kind to `runtime`, so no
/// reader can mistake the runtime's word for the model's answer.
/// D-348/D-349: v4 records, per model request, the tool names it was offered and whether the surface check
/// authorized that set (`model_requests.offered_tools` / `.surface_authorized`, D-341's decision for D-143).
/// 4 -> 5 (D-451): append-only pending user-input revisions; existing envelopes stay valid.
/// 7 -> 8: goals become work scopes; identifiers, events and acceptance/budget state are migrated explicitly.
pub const V2_SCHEMA_VERSION: i64 = 8;

/// Request kinds (§4.1 ModelRequest): an ordinary turn request advances the
/// instance phase; a compression request only produces a summary (§7, A20).
pub const REQUEST_KIND_TURN: &str = "turn";
pub const REQUEST_KIND_COMPRESSION: &str = "compression";
