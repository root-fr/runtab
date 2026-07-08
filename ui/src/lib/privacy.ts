import type { SyncedEvent } from "@/api/types";

// The consent copy. Per the spec guardrails this exact sentence is the consent
// moment and must appear wherever a user is about to upload. Both the "see what
// syncs" drawer and the pre-sync review screen import it from here so the two
// can never drift.
export const PRIVACY_SENTENCE =
  "Only these derived numbers ever leave your machine. No prompts, no code, no file paths, ever.";

// A whitelist-shaped fallback shown only when the ledger has no rows yet.
// Otherwise the drawer renders one real record from the user's own data
// (GET /api/sync/preview-record). The ids are 64-hex sha256 digests, the exact
// shape the server's own validator accepts, so even the sample could sync.
export const EXAMPLE_SYNCED_EVENT: SyncedEvent = {
  event_id: "3f9a1c7e".repeat(8),
  ts: "2026-07-06T10:00:00Z",
  agent: "claude-code",
  model: "claude-sonnet-4",
  project_label: "tkm",
  session_id: "9b2e5d1a".repeat(8),
  machine_id: "6d1f0a2c-1b2c-4d5e-8f90-a1b2c3d4e5f6",
  machine_name: "laptop",
  input_tokens: 1024,
  output_tokens: 512,
  cache_read_tokens: 8000,
  cache_creation_tokens: 256,
  reasoning_tokens: 0,
  est_cost_microusd: 41230,
  cost_basis: "estimated",
};
