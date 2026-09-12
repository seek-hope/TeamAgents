#!/usr/bin/env node
/** teamagents CLI (TS) — milestone 1: talks to the Rust core. */
import { CoreClient } from "./core-client.ts";

const coreBin = process.env.TEAMAGENTS_CORE ?? "../core/target/debug/teamagents-core";

async function main() {
  const [cmd] = process.argv.slice(2);
  const client = new CoreClient(coreBin);
  try {
    if (cmd === "doctor" || cmd === undefined) {
      const info = await client.call("ping");
      console.log(`teamagents-core ${info.core} OK`);
    } else {
      console.error(`unknown command: ${cmd}`);
      process.exit(1);
    }
  } finally {
    client.close();
  }
}

main();
