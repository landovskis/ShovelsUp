// IMP-REQ-009-11 / TC-REQ-009-6: queue list endpoint meets latency target
// under load, against a 5,000-row seeded review_candidates table.
//
// Run: k6 run -e BASE_URL=https://staging.shovelsup.example \
//        -e ADMIN_USER=admin -e ADMIN_PASSWORD=<pw> loadtest/review_queue.js
//
// Requires REVIEW_QUEUE_ENABLED=true on the target and review_candidates
// seeded to ~5,000 open rows first (this script only reads, it doesn't seed).
import http from 'k6/http';
import { check } from 'k6';
import encoding from 'k6/encoding';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:3000';
const ADMIN_USER = __ENV.ADMIN_USER || 'admin';
const ADMIN_PASSWORD = __ENV.ADMIN_PASSWORD || '';

export const options = {
  scenarios: {
    queue_list_read: {
      executor: 'constant-vus',
      vus: 20,
      duration: '1m',
    },
  },
  thresholds: {
    http_req_duration: ['p(95)<1000'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  const res = http.get(`${BASE_URL}/admin/review_candidates?status=open`, {
    headers: {
      Authorization: `Basic ${encoding.b64encode(`${ADMIN_USER}:${ADMIN_PASSWORD}`)}`,
    },
  });

  check(res, {
    'status is 200': (r) => r.status === 200,
  });
}
