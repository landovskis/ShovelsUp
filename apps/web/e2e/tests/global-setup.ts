import { execFileSync } from 'node:child_process';

// TC-011-2/-3 (IMP-REQ-011-14/-15) need a real project row to navigate
// `/projects/{id}` against — this harness has no HTTP-reachable "create a
// project" endpoint (the pipeline populates `projects` from ingested
// council documents, not a public API), so this seeds one directly via
// `psql` against the same local dev Postgres instance the Rust test suite
// itself targets (`postgres://shovelsup:change-me@localhost:5434/shovelsup`
// — the local dev connection string documented for this task, not a
// production secret). Requires `psql` on PATH and the dev Postgres
// container running with migrations applied (both already required to run
// `cargo run -p shovelsup-web` in the first place).
//
// Runs once, in Playwright's main process, before any worker spins up;
// `process.env` set here is inherited by every worker process Playwright
// spawns, so tests read the seeded id via `process.env.SHOVELSUP_E2E_PROJECT_ID`.
const DEV_DATABASE_URL =
  process.env.SHOVELSUP_DATABASE_URL ||
  'postgres://shovelsup:change-me@localhost:5434/shovelsup';

export default function globalSetup() {
  const output = execFileSync(
    'psql',
    [
      DEV_DATABASE_URL,
      '-t',
      '-A',
      '-c',
      // Idempotent: re-running the harness against the same database (e.g.
      // a second local `npm test` invocation) must not fail on the unique
      // `(civic_address_normalized, project_type)` index — delete any
      // previous harness-seeded row first, then insert fresh, rather than
      // relying on `ON CONFLICT` column inference (the unique index here
      // isn't in a shape Postgres will infer a conflict target from).
      "DELETE FROM projects WHERE civic_address_normalized = '1 e2e harness lane' AND project_type = 'residential'; " +
        "INSERT INTO projects (civic_address_normalized, project_type) VALUES ('1 e2e harness lane', 'residential') RETURNING id;",
    ],
    { encoding: 'utf-8' },
  );

  // `-t` suppresses column headers/footers but NOT each statement's
  // command-completion tag (e.g. "DELETE 1", "INSERT 0 1") when multiple
  // statements run in one `-c`, so the raw output is several lines — pull
  // out just the UUID line rather than assuming the whole trimmed output
  // is the id.
  const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
  const projectId = output
    .split('\n')
    .map((line) => line.trim())
    .find((line) => uuidPattern.test(line));
  if (!projectId) {
    throw new Error(
      `global-setup: failed to seed an e2e project row — psql returned no id. ` +
        `Is Postgres running and reachable at ${DEV_DATABASE_URL}, with migrations applied?`,
    );
  }

  process.env.SHOVELSUP_E2E_PROJECT_ID = projectId;
}
