import { test } from "node:test";
import assert from "node:assert/strict";
import { CoreClient } from "../src/core-client.ts";

const coreBin = process.env.TEAMAGENTS_CORE ?? "../core/target/debug/teamagents-core";

test("ping and session lifecycle over stdio", async () => {
  const client = new CoreClient(coreBin);
  try {
    const pong = await client.call("ping");
    assert.ok(pong.core);

    await client.createSession("s1", "/tmp");
    const action = {
      action_id: "act_t1",
      session_id: "s1",
      actor_id: "user",
      kind: "user_message",
      payload: { text: "hi" },
    };
    const r1 = await client.submit(action);
    assert.equal(r1.ok, false); // reduce not yet ported — readable refusal
    assert.match(r1.error, /not yet ported/);
    const r2 = await client.submit(action); // idempotent replay
    assert.deepEqual(r2, r1);
  } finally {
    client.close();
  }
});
