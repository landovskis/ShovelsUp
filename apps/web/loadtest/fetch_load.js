// IMP-REQ-001-09 / TC-REQ-001-5: post-meeting fetch load stays within SLA.
//
// Drives 30 concurrent fixture fetches against a running instance's
// municipal-fetch code path. Point BASE_URL at a staging deployment with a
// reachable fixture HTTP server standing in for a municipal site.
//
// LIMITATION: exercises Fetcher indirectly via the admin reprocess endpoint,
// the only HTTP-reachable trigger currently wired to it — there is no
// fetch_jobs worker yet (see IMPLEMENTATION_CHECKLIST.md REQ-001 risks), so
// this measures the reprocess handler's latency under load, not a true
// end-to-end scheduled-fetch burst. Revisit once the worker lands.
//
// Run: k6 run -e BASE_URL=https://staging.shovelsup.example loadtest/fetch_load.js
import http from 'k6/http';
import { check } from 'k6';
import encoding from 'k6/encoding';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:3000';
const ADMIN_USER = __ENV.ADMIN_USER || 'admin';
const ADMIN_PASSWORD = __ENV.ADMIN_PASSWORD || '';
const FETCH_JOB_ID = __ENV.FETCH_JOB_ID;

export const options = {
  scenarios: {
    post_meeting_burst: {
      executor: 'shared-iterations',
      vus: 30,
      iterations: 30,
      maxDuration: '2m',
    },
  },
  thresholds: {
    http_req_duration: ['p(95)<2000'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  if (!FETCH_JOB_ID) {
    throw new Error('FETCH_JOB_ID env var is required (seed a fetch_jobs row first)');
  }

  const res = http.post(
    `${BASE_URL}/admin/fetch_jobs/${FETCH_JOB_ID}/reprocess`,
    null,
    {
      headers: {
        Authorization: `Basic ${encoding.b64encode(`${ADMIN_USER}:${ADMIN_PASSWORD}`)}`,
      },
    },
  );

  check(res, {
    'status is 200 or 409 (already reprocessing)': (r) => r.status === 200 || r.status === 409,
  });
}
